//! "TFTP server" (RFC 1350, with the RFC 2347 / 2348 block size option):
//! reads and writes files of the root folder (network devices fetch
//! firmware and push configurations this way). Each transfer gets its
//! own socket and thread; lost packets are sent again a few times.

use crate::{Ctx, ServerHandle, resolve};
use std::io::{Read, Write};
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::time::Duration;

const RRQ: u16 = 1;
const WRQ: u16 = 2;
const DATA: u16 = 3;
const ACK: u16 = 4;
const ERROR: u16 = 5;
const OACK: u16 = 6;

const DEFAULT_BLOCK: usize = 512;
/// Largest block size of RFC 2348.
pub const MAX_BLOCK: usize = 65_464;
const RETRIES: u32 = 5;
const TIMEOUT: Duration = Duration::from_secs(3);
/// Transfers running at the same time.
const MAX_TRANSFERS: usize = 32;

#[derive(Debug, Clone)]
pub struct TftpConfig {
    pub root: PathBuf,
    pub port: u16,
    pub bind: String,
    /// "TFTP buffer size": the largest block size offered.
    pub buffer_size: usize,
    /// "Stop server after".
    pub stop_after: Option<Duration>,
}

pub fn start(config: TftpConfig) -> std::io::Result<ServerHandle> {
    if !config.root.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} is not a folder", config.root.display()),
        ));
    }
    let socket = UdpSocket::bind((config.bind.as_str(), config.port))?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    let addr = socket.local_addr()?;
    let (ctx, log, stop) = Ctx::new(config.stop_after);
    ctx.say(format!(
        "TFTP server started on {addr}, serving {}",
        config.root.display()
    ));
    let worker = {
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("tc-tftp".into())
            .spawn(move || listen(&socket, &ctx, &config))?
    };
    Ok(crate::handle(ctx, log, stop, addr, vec![worker]))
}

fn listen(socket: &UdpSocket, ctx: &Ctx, config: &TftpConfig) {
    let busy = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut buf = vec![0u8; 1024];
    while !ctx.stopping() {
        let Ok((n, peer)) = socket.recv_from(&mut buf) else {
            continue;
        };
        let Some(request) = Request::parse(&buf[..n]) else {
            continue;
        };
        use std::sync::atomic::Ordering;
        if busy.fetch_add(1, Ordering::AcqRel) >= MAX_TRANSFERS {
            busy.fetch_sub(1, Ordering::AcqRel);
            let _ = socket.send_to(&error(0, "server busy"), peer);
            continue;
        }
        let (ctx, config, slot) = (ctx.clone(), config.clone(), busy.clone());
        let spawned = std::thread::Builder::new()
            .name("tc-tftp-transfer".into())
            .spawn(move || {
                let what = if request.write { "upload" } else { "download" };
                let line = match transfer(&request, peer, &config, &ctx) {
                    Ok(bytes) => format!("{peer} {what} of {} done ({bytes} bytes)", request.file),
                    Err(e) => format!("{peer} {what} of {} failed: {e}", request.file),
                };
                ctx.say(line);
                slot.fetch_sub(1, Ordering::AcqRel);
            });
        if spawned.is_err() {
            busy.fetch_sub(1, Ordering::AcqRel);
        }
    }
    ctx.say("TFTP server stopped".into());
}

#[derive(Debug, PartialEq, Eq)]
struct Request {
    write: bool,
    file: String,
    /// Requested block size (RFC 2348), if any.
    blksize: Option<usize>,
}

impl Request {
    fn parse(p: &[u8]) -> Option<Self> {
        let op = u16::from_be_bytes([*p.first()?, *p.get(1)?]);
        let write = match op {
            RRQ => false,
            WRQ => true,
            _ => return None,
        };
        let mut fields = p[2..].split(|&b| b == 0);
        let file = String::from_utf8(fields.next()?.to_vec()).ok()?;
        let _mode = fields.next()?;
        let mut blksize = None;
        while let (Some(k), Some(v)) = (fields.next(), fields.next()) {
            if k.eq_ignore_ascii_case(b"blksize") {
                blksize = std::str::from_utf8(v).ok()?.parse().ok();
            }
        }
        (!file.is_empty()).then_some(Self {
            write,
            file,
            blksize,
        })
    }
}

fn error(code: u16, message: &str) -> Vec<u8> {
    let mut p = Vec::with_capacity(5 + message.len());
    p.extend_from_slice(&ERROR.to_be_bytes());
    p.extend_from_slice(&code.to_be_bytes());
    p.extend_from_slice(message.as_bytes());
    p.push(0);
    p
}

fn oack(blksize: usize) -> Vec<u8> {
    let mut p = OACK.to_be_bytes().to_vec();
    p.extend_from_slice(b"blksize\0");
    p.extend_from_slice(blksize.to_string().as_bytes());
    p.push(0);
    p
}

/// Send `packet` until the peer acknowledges `block` (or gives up).
fn send_until_ack(sock: &UdpSocket, packet: &[u8], block: u16, ctx: &Ctx) -> Result<(), String> {
    let mut buf = [0u8; 516];
    for _ in 0..RETRIES {
        if ctx.stopping() {
            return Err("server stopped".into());
        }
        sock.send(packet).map_err(|e| e.to_string())?;
        loop {
            match sock.recv(&mut buf) {
                Ok(n) if n >= 4 => {
                    let op = u16::from_be_bytes([buf[0], buf[1]]);
                    let got = u16::from_be_bytes([buf[2], buf[3]]);
                    match op {
                        ACK if got == block => return Ok(()),
                        ACK => continue, // an older ACK: keep waiting
                        ERROR => return Err("the client aborted".into()),
                        _ => continue,
                    }
                }
                Ok(_) => continue,
                Err(_) => break, // timeout: send again
            }
        }
    }
    Err("no answer from the client".into())
}

fn transfer(
    req: &Request,
    peer: SocketAddr,
    config: &TftpConfig,
    ctx: &Ctx,
) -> Result<u64, String> {
    let local = if peer.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let sock = UdpSocket::bind(local).map_err(|e| e.to_string())?;
    sock.connect(peer).map_err(|e| e.to_string())?;
    sock.set_read_timeout(Some(TIMEOUT))
        .map_err(|e| e.to_string())?;
    let limit = config.buffer_size.clamp(DEFAULT_BLOCK, MAX_BLOCK);
    let blksize = req.blksize.map(|b| b.clamp(8, limit));
    let Some(path) = resolve(&config.root, &req.file, !req.write).filter(|p| !p.is_dir()) else {
        let _ = sock.send(&error(
            if req.write { 2 } else { 1 },
            "file not found or not allowed",
        ));
        return Err("refused (outside the root folder or missing)".into());
    };
    if req.write {
        receive(&sock, &path, blksize, ctx)
    } else {
        send(&sock, &path, blksize, ctx)
    }
}

fn send(
    sock: &UdpSocket,
    path: &std::path::Path,
    blksize: Option<usize>,
    ctx: &Ctx,
) -> Result<u64, String> {
    let mut file = std::fs::File::open(path).map_err(|e| {
        let _ = sock.send(&error(2, "cannot read the file"));
        e.to_string()
    })?;
    if let Some(b) = blksize {
        send_until_ack(sock, &oack(b), 0, ctx)?;
    }
    let size = blksize.unwrap_or(DEFAULT_BLOCK);
    let mut block: u16 = 1;
    let mut total = 0u64;
    let mut data = vec![0u8; size];
    loop {
        let mut n = 0;
        while n < size {
            let r = file.read(&mut data[n..]).map_err(|e| e.to_string())?;
            if r == 0 {
                break;
            }
            n += r;
        }
        let mut packet = Vec::with_capacity(4 + n);
        packet.extend_from_slice(&DATA.to_be_bytes());
        packet.extend_from_slice(&block.to_be_bytes());
        packet.extend_from_slice(&data[..n]);
        send_until_ack(sock, &packet, block, ctx)?;
        total += n as u64;
        if n < size {
            return Ok(total);
        }
        block = block.wrapping_add(1);
    }
}

fn receive(
    sock: &UdpSocket,
    path: &std::path::Path,
    blksize: Option<usize>,
    ctx: &Ctx,
) -> Result<u64, String> {
    let size = blksize.unwrap_or(DEFAULT_BLOCK);
    let mut file = std::fs::File::create(path).map_err(|e| {
        let _ = sock.send(&error(2, "cannot write the file"));
        e.to_string()
    })?;
    let mut last_ack = match blksize {
        Some(b) => oack(b),
        None => [ACK.to_be_bytes(), 0u16.to_be_bytes()].concat(),
    };
    let mut expected: u16 = 1;
    let mut total = 0u64;
    let mut buf = vec![0u8; size + 4];
    let mut retries = 0;
    sock.send(&last_ack).map_err(|e| e.to_string())?;
    loop {
        if ctx.stopping() {
            return Err("server stopped".into());
        }
        match sock.recv(&mut buf) {
            Ok(n) if n >= 4 && u16::from_be_bytes([buf[0], buf[1]]) == DATA => {
                let block = u16::from_be_bytes([buf[2], buf[3]]);
                if block == expected {
                    file.write_all(&buf[4..n]).map_err(|e| e.to_string())?;
                    total += (n - 4) as u64;
                    last_ack = [ACK.to_be_bytes(), block.to_be_bytes()].concat();
                    sock.send(&last_ack).map_err(|e| e.to_string())?;
                    retries = 0;
                    if n - 4 < size {
                        file.flush().map_err(|e| e.to_string())?;
                        return Ok(total);
                    }
                    expected = expected.wrapping_add(1);
                } else {
                    // A duplicate: acknowledge it again.
                    sock.send(&last_ack).map_err(|e| e.to_string())?;
                }
            }
            Ok(n) if n >= 2 && u16::from_be_bytes([buf[0], buf[1]]) == ERROR => {
                return Err("the client aborted".into());
            }
            Ok(_) => {}
            Err(_) => {
                retries += 1;
                if retries > RETRIES {
                    return Err("no data from the client".into());
                }
                sock.send(&last_ack).map_err(|e| e.to_string())?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rrq(file: &str, blksize: Option<usize>, write: bool) -> Vec<u8> {
        let mut p = (if write { WRQ } else { RRQ }).to_be_bytes().to_vec();
        p.extend_from_slice(file.as_bytes());
        p.extend_from_slice(b"\0octet\0");
        if let Some(b) = blksize {
            p.extend_from_slice(format!("blksize\0{b}\0").as_bytes());
        }
        p
    }

    fn server(root: &std::path::Path) -> ServerHandle {
        start(TftpConfig {
            root: root.to_path_buf(),
            port: 0,
            bind: "127.0.0.1".into(),
            buffer_size: 8192,
            stop_after: None,
        })
        .unwrap()
    }

    /// A minimal TFTP client: download `file`.
    fn download(addr: SocketAddr, file: &str, blksize: Option<usize>) -> Result<Vec<u8>, u16> {
        let c = UdpSocket::bind("127.0.0.1:0").unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.send_to(&rrq(file, blksize, false), addr).unwrap();
        let mut out = Vec::new();
        let mut buf = vec![0u8; 70_000];
        let size = blksize.map_or(512, |b| b.min(8192));
        loop {
            let (n, from) = c.recv_from(&mut buf).unwrap();
            let op = u16::from_be_bytes([buf[0], buf[1]]);
            match op {
                OACK => {
                    c.send_to(&[0, 4, 0, 0], from).unwrap();
                }
                DATA => {
                    out.extend_from_slice(&buf[4..n]);
                    c.send_to(&[0, 4, buf[2], buf[3]], from).unwrap();
                    if n - 4 < size {
                        return Ok(out);
                    }
                }
                ERROR => return Err(u16::from_be_bytes([buf[2], buf[3]])),
                _ => panic!("op {op}"),
            }
        }
    }

    #[test]
    fn downloads_with_and_without_block_size() {
        let dir = tempfile::tempdir().unwrap();
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 253) as u8).collect();
        std::fs::write(dir.path().join("fw.bin"), &data).unwrap();
        std::fs::write(dir.path().join("exact.bin"), vec![7u8; 1024]).unwrap();
        let s = server(dir.path());
        assert_eq!(download(s.addr, "fw.bin", None).unwrap(), data);
        assert_eq!(download(s.addr, "fw.bin", Some(1428)).unwrap(), data);
        assert_eq!(
            download(s.addr, "fw.bin", Some(60_000)).unwrap(),
            data,
            "capped at 8192"
        );
        assert_eq!(
            download(s.addr, "exact.bin", None).unwrap().len(),
            1024,
            "empty last block"
        );
        assert_eq!(download(s.addr, "../x", None), Err(1));
        assert_eq!(download(s.addr, "missing", None), Err(1));
    }

    #[test]
    fn uploads() {
        let dir = tempfile::tempdir().unwrap();
        let s = server(dir.path());
        let c = UdpSocket::bind("127.0.0.1:0").unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.send_to(&rrq("conf.txt", None, true), s.addr).unwrap();
        let mut buf = [0u8; 600];
        let (_, from) = c.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..4], &[0, 4, 0, 0], "ACK 0");
        let body: Vec<u8> = (0..700u32).map(|i| (i % 7) as u8 + b'a').collect();
        for (i, chunk) in body.chunks(512).enumerate() {
            let block = (i + 1) as u16;
            let mut p = vec![0, 3];
            p.extend_from_slice(&block.to_be_bytes());
            p.extend_from_slice(chunk);
            c.send_to(&p, from).unwrap();
            c.recv_from(&mut buf).unwrap();
            assert_eq!(&buf[..4], &[0, 4, (block >> 8) as u8, block as u8]);
        }
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(std::fs::read(dir.path().join("conf.txt")).unwrap(), body);
        // Outside the root: refused.
        c.send_to(&rrq("../evil.txt", None, true), s.addr).unwrap();
        c.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..4], &[0, 5, 0, 2]);
    }

    #[test]
    fn request_parsing() {
        assert_eq!(
            Request::parse(&rrq("a.bin", Some(1024), false)),
            Some(Request {
                write: false,
                file: "a.bin".into(),
                blksize: Some(1024),
            })
        );
        assert_eq!(Request::parse(&[0, 3, 0, 1]), None);
        assert_eq!(Request::parse(&[0, 1, 0]), None, "empty name");
    }
}
