//! SFTP over the same SSH connection (the "SSH-browser"). Each request runs
//! in its own task so a long transfer never blocks the terminal.

use crate::Events;
use crate::SshEvent;
use russh_sftp::client::SftpSession;
use std::sync::Arc;
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
            SftpRequest::Download { remote, local } => {
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
                    dst.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
                    done += n as u64;
                    send(SftpEvent::Progress { id, done, total });
                }
                dst.flush().await.map_err(|e| e.to_string())?;
                Ok(None)
            }
            SftpRequest::Upload { local, remote } => {
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
                    dst.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
                    done += n as u64;
                    send(SftpEvent::Progress { id, done, total });
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
        }
    }
    .await;
    match result {
        Ok(Some(event)) => send(event),
        Ok(None) => send(SftpEvent::Done { id }),
        Err(error) => send(SftpEvent::Failed { id, error }),
    }
}
