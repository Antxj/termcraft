//! SCP "SSH-browser" (3_26 "SCP (enhanced speed)" / "SCP (normal
//! speed)", and the fallback when a server has no SFTP subsystem). Each
//! request runs on its own exec channel:
//!
//! - listings with `ls -la` (epoch times when GNU `ls` is there),
//! - downloads / uploads with the SCP protocol (`scp -f` / `scp -t`),
//! - mkdir / rm / mv / chmod with the usual commands.
//!
//! Paths are always single-quoted for the remote shell.

use crate::{Events, SshEvent};
use russh::ChannelMsg;
use russh::client::Msg;
use tc_core::remote_fs::{EolConverter, FsEvent, FsRequest, RemoteEntry};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type Channel = russh::Channel<Msg>;

/// One argument for the remote `sh`.
pub(crate) fn quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// Run a command; stdout on success, the last stderr line otherwise.
pub(crate) async fn run(mut channel: Channel, command: &str) -> Result<Vec<u8>, String> {
    channel
        .exec(true, command)
        .await
        .map_err(|e| e.to_string())?;
    let (mut out, mut err, mut status) = (Vec::new(), Vec::new(), None);
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => out.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, .. } => err.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
            ChannelMsg::Eof | ChannelMsg::Close => break,
            _ => {}
        }
        if out.len() > 16 * 1024 * 1024 {
            return Err("answer too large".into());
        }
    }
    let ok = status == Some(0) || (status.is_none() && err.is_empty());
    match ok {
        true => Ok(out),
        false => {
            let text = String::from_utf8_lossy(&err);
            let line = text
                .lines()
                .rfind(|l| !l.trim().is_empty())
                .unwrap_or("failed");
            Err(line.trim().to_string())
        }
    }
}

/// `ls` for a folder: epoch times with GNU ls, else the plain format.
pub(crate) fn list_command(path: &str) -> String {
    let p = quote(path);
    format!("cd -- {p} && {{ LC_ALL=C ls -la --time-style=+%s 2>/dev/null || LC_ALL=C ls -la; }}")
}

/// Split off the first `n` whitespace-separated fields; the rest of the
/// line (with its inner spaces) is returned as is.
fn fields(line: &str, n: usize) -> Option<(Vec<&str>, &str)> {
    let mut out = Vec::with_capacity(n);
    let mut rest = line;
    for _ in 0..n {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        out.push(&rest[..end]);
        rest = &rest[end..];
    }
    // Exactly one separator space before the name (names may start with
    // spaces, which ls prints as is).
    Some((out, rest.strip_prefix(' ').unwrap_or(rest)))
}

fn mode_bits(perms: &str) -> Option<u32> {
    let b = perms.as_bytes();
    if b.len() < 10 {
        return None;
    }
    let mut mode = 0u32;
    for (i, bit) in [0o400, 0o200, 0o100, 0o40, 0o20, 0o10, 0o4, 0o2, 0o1]
        .into_iter()
        .enumerate()
    {
        let c = b[i + 1];
        if c != b'-' && c != b'S' && c != b'T' {
            mode |= bit;
        }
    }
    let special = |c: u8, set: u32| {
        if c == b's' || c == b'S' || c == b't' || c == b'T' {
            set
        } else {
            0
        }
    };
    mode |= special(b[3], 0o4000) | special(b[6], 0o2000) | special(b[9], 0o1000);
    Some(mode)
}

/// Parse `ls -la` output (with `--time-style=+%s` or the default dates).
pub fn parse_ls(text: &str) -> Vec<RemoteEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let Some((head, _)) = fields(line, 6) else {
            continue;
        };
        let perms = head[0];
        let kind = perms.as_bytes()[0];
        if !b"-dlcbps".contains(&kind) || perms.len() < 10 {
            continue; // "total 12" and the like
        }
        // Epoch time (one field) or "Mon DD HH:MM|YYYY" (three fields).
        let epoch = head[5].parse::<u64>().ok().filter(|_| head[5].len() >= 9);
        let (modified, name) = if epoch.is_some() {
            (epoch, fields(line, 6).map(|(_, r)| r))
        } else {
            (None, fields(line, 8).map(|(_, r)| r))
        };
        let Some(mut name) = name else { continue };
        if kind == b'l'
            && let Some((n, _target)) = name.split_once(" -> ")
        {
            name = n;
        }
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        entries.push(RemoteEntry {
            name: name.to_string(),
            is_dir: kind == b'd',
            size: head[4].parse().unwrap_or(0),
            modified,
            permissions: mode_bits(perms),
            owner: Some(head[2].to_string()),
            group: Some(head[3].to_string()),
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

/// Read one SCP status byte (0 = ok, 1 = warning, 2 = error + message).
async fn ack(stream: &mut (impl AsyncReadExt + Unpin)) -> Result<(), String> {
    let mut b = [0u8; 1];
    stream.read_exact(&mut b).await.map_err(|e| e.to_string())?;
    match b[0] {
        0 => Ok(()),
        1 | 2 => Err(read_line(stream).await.unwrap_or_else(|e| e)),
        other => Err(format!("unexpected SCP answer {other}")),
    }
}

async fn read_line(stream: &mut (impl AsyncReadExt + Unpin)) -> Result<String, String> {
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    loop {
        stream.read_exact(&mut b).await.map_err(|e| e.to_string())?;
        if b[0] == b'\n' {
            break;
        }
        line.push(b[0]);
        if line.len() > 4096 {
            return Err("SCP header too long".into());
        }
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

/// `C0644 1234 name` → size.
fn parse_c_line(line: &str) -> Result<u64, String> {
    let mut parts = line.splitn(3, ' ');
    let (Some(mode), Some(size)) = (parts.next(), parts.next()) else {
        return Err(format!("bad SCP header {line:?}"));
    };
    if !mode.starts_with('C') {
        return Err(if mode.starts_with('D') {
            "this is a folder".into()
        } else {
            format!("bad SCP header {line:?}")
        });
    }
    size.parse().map_err(|_| format!("bad SCP size {line:?}"))
}

pub(crate) struct Transfer {
    pub chunk: usize,
}

pub(crate) async fn execute(
    channel: Channel,
    id: u64,
    request: FsRequest,
    transfer: Transfer,
    events: Events,
) {
    let send = |e: FsEvent| events.send(SshEvent::Sftp(e));
    let result: Result<Option<FsEvent>, String> = async {
        match request {
            FsRequest::List(path) => {
                let out = run(channel, &list_command(&path)).await?;
                let entries = parse_ls(&String::from_utf8_lossy(&out));
                Ok(Some(FsEvent::Listing { id, path, entries }))
            }
            FsRequest::Download {
                remote,
                local,
                ascii,
            } => {
                channel
                    .exec(true, format!("scp -f -- {}", quote(&remote)))
                    .await
                    .map_err(|e| e.to_string())?;
                let mut stream = channel.into_stream();
                stream.write_all(&[0]).await.map_err(|e| e.to_string())?;
                // Optional warnings, then the "C" line.
                let mut first = [0u8; 1];
                stream
                    .read_exact(&mut first)
                    .await
                    .map_err(|e| e.to_string())?;
                if first[0] == 1 || first[0] == 2 {
                    return Err(read_line(&mut stream).await.unwrap_or_else(|e| e));
                }
                let mut line = (first[0] as char).to_string();
                line.push_str(&read_line(&mut stream).await?);
                let total = parse_c_line(&line)?;
                stream.write_all(&[0]).await.map_err(|e| e.to_string())?;
                let mut eol = ascii.then(EolConverter::to_local).flatten();
                let mut converted = Vec::new();
                let mut dst = tokio::fs::File::create(&local)
                    .await
                    .map_err(|e| format!("{}: {e}", local.display()))?;
                let mut buf = vec![0u8; transfer.chunk];
                let mut done = 0u64;
                while done < total {
                    let want = usize::try_from(total - done)
                        .unwrap_or(usize::MAX)
                        .min(buf.len());
                    let n = stream
                        .read(&mut buf[..want])
                        .await
                        .map_err(|e| e.to_string())?;
                    if n == 0 {
                        return Err("connection closed during the transfer".into());
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
                    send(FsEvent::Progress {
                        id,
                        done,
                        total: Some(total),
                    });
                }
                if let Some(c) = &mut eol {
                    converted.clear();
                    c.finish(&mut converted);
                    dst.write_all(&converted).await.map_err(|e| e.to_string())?;
                }
                dst.flush().await.map_err(|e| e.to_string())?;
                ack(&mut stream).await?;
                let _ = stream.write_all(&[0]).await;
                Ok(None)
            }
            FsRequest::Upload {
                local,
                remote,
                ascii,
            } => {
                let mut data = tokio::fs::read(&local)
                    .await
                    .map_err(|e| format!("{}: {e}", local.display()))?;
                if ascii {
                    let mut c = EolConverter::to_lf();
                    let mut out = Vec::with_capacity(data.len());
                    c.convert(&data, &mut out);
                    c.finish(&mut out);
                    data = out;
                }
                let name = remote.rsplit('/').next().unwrap_or_default();
                if name.is_empty() || name.contains(['\n', '/']) {
                    return Err(format!("invalid file name {name:?}"));
                }
                channel
                    .exec(true, format!("scp -t -- {}", quote(&remote)))
                    .await
                    .map_err(|e| e.to_string())?;
                let mut stream = channel.into_stream();
                ack(&mut stream).await?;
                let header = format!("C0644 {} {name}\n", data.len());
                stream
                    .write_all(header.as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                ack(&mut stream).await?;
                let total = data.len() as u64;
                let mut done = 0u64;
                for chunk in data.chunks(transfer.chunk) {
                    stream.write_all(chunk).await.map_err(|e| e.to_string())?;
                    done += chunk.len() as u64;
                    send(FsEvent::Progress {
                        id,
                        done,
                        total: Some(total),
                    });
                }
                stream.write_all(&[0]).await.map_err(|e| e.to_string())?;
                ack(&mut stream).await?;
                Ok(None)
            }
            FsRequest::Mkdir(path) => run(channel, &format!("mkdir -- {}", quote(&path)))
                .await
                .map(|_| None),
            FsRequest::CreateFile(path) => {
                // noclobber: fails if the file exists.
                run(channel, &format!("set -C; : > {}", quote(&path)))
                    .await
                    .map(|_| None)
            }
            FsRequest::Remove { path, is_dir } => {
                let cmd = if is_dir { "rmdir" } else { "rm -f" };
                run(channel, &format!("{cmd} -- {}", quote(&path)))
                    .await
                    .map(|_| None)
            }
            FsRequest::Rename { from, to } => {
                run(channel, &format!("mv -- {} {}", quote(&from), quote(&to)))
                    .await
                    .map(|_| None)
            }
            FsRequest::SetPermissions { path, mode } => run(
                channel,
                &format!("chmod {:o} -- {}", mode & 0o7777, quote(&path)),
            )
            .await
            .map(|_| None),
        }
    }
    .await;
    match result {
        Ok(Some(event)) => send(event),
        Ok(None) => send(FsEvent::Done { id }),
        Err(error) => send(FsEvent::Failed { id, error }),
    }
}

/// Start the SCP browser: the home folder, and whether `scp` exists.
pub(crate) async fn open(
    session: &russh::client::Handle<crate::Handler>,
) -> Result<String, String> {
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| format!("cannot open a channel: {e}"))?;
    let out = run(
        channel,
        "command -v scp >/dev/null 2>&1 || echo __NO_SCP__; pwd",
    )
    .await?;
    let text = String::from_utf8_lossy(&out);
    if text.contains("__NO_SCP__") {
        return Err("scp is not installed on the server".into());
    }
    Ok(text
        .lines()
        .rfind(|l| l.starts_with('/'))
        .unwrap_or("/")
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listings() {
        let gnu = "total 12\n\
            drwxr-xr-x  3 bob staff 4096 1700000000 .\n\
            drwxr-xr-x 20 root root 4096 1700000001 ..\n\
            -rw-r--r--  1 bob staff  123 1700000002 notes.txt\n\
            drwxr-sr-x  2 bob staff 4096 1700000003 my dir\n\
            lrwxrwxrwx  1 bob staff    7 1700000004 link -> target\n\
            -rwsr-x---  1 root bob   99 1700000005  lead space\n";
        let e = parse_ls(gnu);
        let names: Vec<&str> = e.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["my dir", " lead space", "link", "notes.txt"]);
        assert!(e[0].is_dir && e[0].permissions == Some(0o2755));
        assert_eq!(e[1].permissions, Some(0o4750));
        assert_eq!(e[3].modified, Some(1_700_000_002));
        assert_eq!(e[3].size, 123);

        let plain = "-rw-r--r-- 1 bob staff 5 Oct 14 07:53 a b.txt\n\
                     -rw------- 1 bob staff 6 Jan  2  2024 old\n";
        let e = parse_ls(plain);
        assert_eq!(e[0].name, "a b.txt");
        assert_eq!(e[1].name, "old");
        assert_eq!(e[0].modified, None);
    }

    #[test]
    fn quoting_and_headers() {
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert!(list_command("/tmp/a b").starts_with("cd -- '/tmp/a b' && "));
        assert_eq!(parse_c_line("C0644 1234 file name"), Ok(1234));
        assert!(parse_c_line("D0755 0 dir").is_err());
        assert!(parse_c_line("C0644 x f").is_err());
    }
}
