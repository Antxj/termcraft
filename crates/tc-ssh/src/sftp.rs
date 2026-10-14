//! SFTP over the same SSH connection (the "SSH-browser"). Each request runs
//! in its own task so a long transfer never blocks the terminal.

use crate::Events;
use crate::SshEvent;
use russh_sftp::client::SftpSession;
use std::sync::Arc;
use tc_core::remote_fs::EolConverter;
pub use tc_core::remote_fs::{
    FsEvent as SftpEvent, FsRequest as SftpRequest, RemoteEntry, join, parent,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) async fn open(
    session: &russh::client::Handle<crate::Handler>,
) -> Result<(Arc<SftpSession>, String), String> {
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| format!("cannot open SFTP channel: {e}"))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("SFTP subsystem refused: {e}"))?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| format!("SFTP init failed: {e}"))?;
    let home = sftp
        .canonicalize(".")
        .await
        .unwrap_or_else(|_| "/".to_string());
    Ok((Arc::new(sftp), home))
}

/// Printed by the elevated shell right before it becomes `sftp-server`, so
/// the SFTP protocol only starts once sudo has finished with stdin.
const SUDO_READY: &str = "TC_SUDO_READY";

/// The remote command for "Use SUDO for SSH-browser". The password (if
/// any) is written to stdin, never put on the command line.
pub(crate) fn sudo_command(with_password: bool) -> String {
    let servers = [
        "/usr/lib/openssh/sftp-server",
        "/usr/libexec/openssh/sftp-server",
        "/usr/lib/ssh/sftp-server",
        "/usr/libexec/sftp-server",
        "/usr/lib/sftp-server",
        "/usr/local/libexec/sftp-server",
    ]
    .join(" ");
    let script = format!(
        "for p in {servers}; do if [ -x \"$p\" ]; then echo {SUDO_READY}; exec \"$p\"; fi; done; \
         echo 'sftp-server not found' >&2; exit 127"
    );
    let mode = if with_password { "-S -p ''" } else { "-n" };
    format!("sudo {mode} -- sh -c '{}'", script.replace('\'', "'\\''"))
}

pub(crate) enum SudoError {
    PasswordNeeded,
    Failed(String),
}

/// Open the browser through `sudo sftp-server`. Without a password, only
/// `sudo -n` is tried (no prompt, no stdin).
pub(crate) async fn open_sudo(
    session: &russh::client::Handle<crate::Handler>,
    password: Option<&str>,
) -> Result<(Arc<SftpSession>, String), SudoError> {
    use russh::ChannelMsg;
    let fail = |e: String| SudoError::Failed(e);
    let mut channel = session
        .channel_open_session()
        .await
        .map_err(|e| fail(format!("cannot open SFTP channel: {e}")))?;
    channel
        .exec(true, sudo_command(password.is_some()))
        .await
        .map_err(|e| fail(format!("sudo: {e}")))?;
    if let Some(pw) = password {
        let line = zeroize::Zeroizing::new(format!("{pw}\n"));
        channel
            .data(line.as_bytes())
            .await
            .map_err(|e| fail(format!("sudo: {e}")))?;
    }
    let mut stdout = Vec::new();
    let mut stderr = String::new();
    let wait = async {
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    stdout.extend_from_slice(&data);
                    if String::from_utf8_lossy(&stdout).contains(SUDO_READY) {
                        return Ok(());
                    }
                }
                ChannelMsg::ExtendedData { data, .. } => {
                    stderr.push_str(&String::from_utf8_lossy(&data));
                    // A wrong password: sudo asks again on stdin; stop here.
                    if stderr.contains("Sorry, try again") || stderr.contains("incorrect password")
                    {
                        return Err(fail("sudo: wrong password".into()));
                    }
                }
                ChannelMsg::ExitStatus { .. } | ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
        let e = stderr.trim();
        if password.is_none() && (e.contains("password") || e.contains("terminal")) {
            Err(SudoError::PasswordNeeded)
        } else if e.is_empty() {
            Err(fail("sudo is not available on this server".into()))
        } else {
            Err(fail(format!("sudo: {}", e.lines().last().unwrap_or(e))))
        }
    };
    match tokio::time::timeout(std::time::Duration::from_secs(15), wait).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = channel.close().await;
            return Err(e);
        }
        Err(_) => {
            let _ = channel.close().await;
            return Err(fail("sudo did not answer".into()));
        }
    }
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| fail(format!("SFTP init failed: {e}")))?;
    let home = sftp
        .canonicalize(".")
        .await
        .unwrap_or_else(|_| "/".to_string());
    Ok((Arc::new(sftp), home))
}

const CHUNK: usize = 64 * 1024;

pub(crate) async fn execute(sftp: Arc<SftpSession>, id: u64, request: SftpRequest, events: Events) {
    let send = |e: SftpEvent| events.send(SshEvent::Sftp(e));
    let result: Result<Option<SftpEvent>, String> = async {
        match request {
            SftpRequest::List(path) => {
                let mut entries: Vec<RemoteEntry> = sftp
                    .read_dir(path.clone())
                    .await
                    .map_err(|e| e.to_string())?
                    .filter(|e| e.file_name() != "." && e.file_name() != "..")
                    .map(|e| {
                        let m = e.metadata();
                        RemoteEntry {
                            name: e.file_name(),
                            is_dir: m.is_dir(),
                            size: m.size.unwrap_or(0),
                            modified: m.mtime.map(u64::from),
                            permissions: m.permissions.map(|p| p & 0o7777),
                            owner: m.user.clone().or_else(|| m.uid.map(|u| u.to_string())),
                            group: m.group.clone().or_else(|| m.gid.map(|g| g.to_string())),
                        }
                    })
                    .collect();
                entries.sort_by(|a, b| {
                    b.is_dir
                        .cmp(&a.is_dir)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                Ok(Some(SftpEvent::Listing { id, path, entries }))
            }
            SftpRequest::Download {
                remote,
                local,
                ascii,
            } => {
                let mut eol = ascii.then(EolConverter::to_local).flatten();
                let mut converted = Vec::new();
                let total = sftp
                    .metadata(remote.clone())
                    .await
                    .ok()
                    .and_then(|m| m.size);
                let mut src = sftp.open(remote).await.map_err(|e| e.to_string())?;
                let mut dst = tokio::fs::File::create(&local)
                    .await
                    .map_err(|e| format!("{}: {e}", local.display()))?;
                let mut buf = vec![0u8; CHUNK];
                let mut done = 0u64;
                loop {
                    let n = src.read(&mut buf).await.map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    let chunk = match &mut eol {
                        Some(c) => {
                            converted.clear();
                            c.convert(&buf[..n], &mut converted);
                            &converted[..]
                        }
                        None => &buf[..n],
                    };
                    dst.write_all(chunk).await.map_err(|e| e.to_string())?;
                    done += n as u64;
                    send(SftpEvent::Progress { id, done, total });
                }
                if let Some(c) = &mut eol {
                    converted.clear();
                    c.finish(&mut converted);
                    dst.write_all(&converted).await.map_err(|e| e.to_string())?;
                }
                dst.flush().await.map_err(|e| e.to_string())?;
                Ok(None)
            }
            SftpRequest::Upload {
                local,
                remote,
                ascii,
            } => {
                let mut eol = ascii.then(EolConverter::to_lf);
                let mut converted = Vec::new();
                let mut src = tokio::fs::File::open(&local)
                    .await
                    .map_err(|e| format!("{}: {e}", local.display()))?;
                let total = src.metadata().await.ok().map(|m| m.len());
                let mut dst = sftp.create(remote).await.map_err(|e| e.to_string())?;
                let mut buf = vec![0u8; CHUNK];
                let mut done = 0u64;
                loop {
                    let n = src.read(&mut buf).await.map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    let chunk = match &mut eol {
                        Some(c) => {
                            converted.clear();
                            c.convert(&buf[..n], &mut converted);
                            &converted[..]
                        }
                        None => &buf[..n],
                    };
                    dst.write_all(chunk).await.map_err(|e| e.to_string())?;
                    done += n as u64;
                    send(SftpEvent::Progress { id, done, total });
                }
                if let Some(c) = &mut eol {
                    converted.clear();
                    c.finish(&mut converted);
                    dst.write_all(&converted).await.map_err(|e| e.to_string())?;
                }
                dst.shutdown().await.map_err(|e| e.to_string())?;
                Ok(None)
            }
            SftpRequest::Mkdir(path) => {
                sftp.create_dir(path).await.map_err(|e| e.to_string())?;
                Ok(None)
            }
            SftpRequest::CreateFile(path) => {
                if sftp.try_exists(path.clone()).await.unwrap_or(false) {
                    return Err(format!("{path} already exists"));
                }
                let mut file = sftp.create(path).await.map_err(|e| e.to_string())?;
                file.shutdown().await.map_err(|e| e.to_string())?;
                Ok(None)
            }
            SftpRequest::Remove { path, is_dir } => {
                if is_dir {
                    sftp.remove_dir(path).await
                } else {
                    sftp.remove_file(path).await
                }
                .map_err(|e| e.to_string())?;
                Ok(None)
            }
            SftpRequest::Rename { from, to } => {
                sftp.rename(from, to).await.map_err(|e| e.to_string())?;
                Ok(None)
            }
            SftpRequest::SetPermissions { path, mode } => {
                let mut attrs = russh_sftp::protocol::FileAttributes::empty();
                attrs.permissions = Some(mode & 0o7777);
                sftp.set_metadata(path, attrs)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(None)
            }
        }
    }
    .await;
    match result {
        Ok(Some(event)) => send(event),
        Ok(None) => send(SftpEvent::Done { id }),
        Err(error) => send(SftpEvent::Failed { id, error }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sudo_command_never_contains_a_password_and_is_quoted() {
        let cmd = sudo_command(true);
        assert!(cmd.starts_with("sudo -S -p '' -- sh -c '"));
        assert!(cmd.ends_with('\''));
        assert!(cmd.contains(SUDO_READY));
        // The inner single quotes are escaped for the outer sh -c '...'.
        assert!(cmd.contains("'\\''sftp-server not found'\\''"));
        assert!(sudo_command(false).starts_with("sudo -n -- sh -c '"));
    }
}
