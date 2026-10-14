//! FTP browser backend against a tiny in-memory FTP server (passive mode).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tc_core::remote_fs::{FsEvent, FsRequest};
use tc_net::ftp::{FtpParams, FtpSession};

#[derive(Default)]
struct Fs {
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
}

fn abs(cwd: &str, p: &str) -> String {
    if p.starts_with('/') {
        p.to_string()
    } else if cwd == "/" {
        format!("/{p}")
    } else {
        format!("{cwd}/{p}")
    }
}

/// Minimal FTP server: enough commands for the browser, one client.
fn server(fs: Arc<Mutex<Fs>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (ctrl, _) = listener.accept().unwrap();
        let mut out = ctrl.try_clone().unwrap();
        let mut lines = BufReader::new(ctrl);
        let mut reply = |s: &str| out.write_all(format!("{s}\r\n").as_bytes()).unwrap();
        reply("220 test server ready");
        let mut cwd = "/".to_string();
        let mut pasv: Option<TcpListener> = None;
        let mut rename_from = String::new();
        let mut line = String::new();
        while {
            line.clear();
            lines.read_line(&mut line).unwrap() > 0
        } {
            let line = line.trim_end();
            let (cmd, arg) = line.split_once(' ').unwrap_or((line, ""));
            let data = |pasv: &mut Option<TcpListener>| -> TcpStream {
                pasv.take().expect("PASV first").accept().unwrap().0
            };
            match cmd.to_ascii_uppercase().as_str() {
                "USER" => reply("331 password please"),
                "PASS" if arg == "secret" => reply("230 logged in"),
                "PASS" => reply("530 bad password"),
                "TYPE" => reply("200 ok"),
                "PWD" => reply(&format!("257 \"{cwd}\" is current")),
                "CWD" => {
                    cwd = abs(&cwd, arg);
                    reply("250 ok")
                }
                "PASV" => {
                    let l = TcpListener::bind("127.0.0.1:0").unwrap();
                    let p = l.local_addr().unwrap().port();
                    pasv = Some(l);
                    reply(&format!(
                        "227 Entering Passive Mode (127,0,0,1,{},{})",
                        p >> 8,
                        p & 0xff
                    ));
                }
                "LIST" => {
                    reply("150 listing");
                    let mut d = data(&mut pasv);
                    let dir = abs(&cwd, arg);
                    let fs = fs.lock().unwrap();
                    let prefix = if dir == "/" {
                        "/".to_string()
                    } else {
                        format!("{dir}/")
                    };
                    for sub in fs
                        .dirs
                        .iter()
                        .filter_map(|x| x.strip_prefix(&prefix))
                        .filter(|x| !x.contains('/'))
                    {
                        writeln!(d, "drwxr-xr-x 2 owner group 0 Jan 01 12:00 {sub}\r").unwrap();
                    }
                    for (name, bytes) in fs
                        .files
                        .iter()
                        .filter_map(|(k, v)| Some((k.strip_prefix(&prefix)?, v)))
                    {
                        if !name.contains('/') {
                            writeln!(
                                d,
                                "-rw-r--r-- 1 owner group {} Jan 01 12:00 {name}\r",
                                bytes.len()
                            )
                            .unwrap();
                        }
                    }
                    drop(d);
                    reply("226 done");
                }
                "SIZE" => match fs.lock().unwrap().files.get(&abs(&cwd, arg)) {
                    Some(b) => reply(&format!("213 {}", b.len())),
                    None => reply("550 no such file"),
                },
                "RETR" => {
                    let content = fs.lock().unwrap().files.get(&abs(&cwd, arg)).cloned();
                    match content {
                        Some(b) => {
                            reply("150 sending");
                            data(&mut pasv).write_all(&b).unwrap();
                            reply("226 done");
                        }
                        None => reply("550 no such file"),
                    }
                }
                "STOR" => {
                    reply("150 ready");
                    let mut buf = Vec::new();
                    data(&mut pasv).read_to_end(&mut buf).unwrap();
                    fs.lock().unwrap().files.insert(abs(&cwd, arg), buf);
                    reply("226 stored");
                }
                "MKD" => {
                    fs.lock().unwrap().dirs.insert(abs(&cwd, arg));
                    reply("257 created")
                }
                "RMD" => {
                    let removed = fs.lock().unwrap().dirs.remove(&abs(&cwd, arg));
                    reply(if removed {
                        "250 removed"
                    } else {
                        "550 no such dir"
                    })
                }
                "DELE" => {
                    let removed = fs.lock().unwrap().files.remove(&abs(&cwd, arg)).is_some();
                    reply(if removed {
                        "250 deleted"
                    } else {
                        "550 no such file"
                    })
                }
                "RNFR" => {
                    rename_from = abs(&cwd, arg);
                    reply("350 ready")
                }
                "RNTO" => {
                    let mut fs = fs.lock().unwrap();
                    let b = fs.files.remove(&rename_from).unwrap();
                    fs.files.insert(abs(&cwd, arg), b);
                    reply("250 renamed")
                }
                "QUIT" => {
                    reply("221 bye");
                    break;
                }
                _ => reply("502 not implemented"),
            }
        }
    });
    port
}

fn next(s: &FtpSession) -> FsEvent {
    loop {
        let e = s
            .events()
            .recv_timeout(Duration::from_secs(10))
            .expect("ftp event timeout");
        if !matches!(e, FsEvent::Progress { .. }) {
            return e;
        }
    }
}

fn params(port: u16, password: &str) -> FtpParams {
    FtpParams {
        host: "127.0.0.1".into(),
        port,
        user: "me".into(),
        password: password.into(),
        passive: true,
        ascii: false,
        start_dir: None,
    }
}

#[test]
fn browse_download_upload_and_manage() {
    let fs = Arc::new(Mutex::new(Fs::default()));
    fs.lock()
        .unwrap()
        .files
        .insert("/hello.txt".into(), b"hi from ftp".to_vec());
    fs.lock().unwrap().dirs.insert("/pub".into());
    let s = FtpSession::connect(params(server(fs.clone()), "secret"), Arc::new(|| {}));
    assert_eq!(next(&s), FsEvent::Ready { home: "/".into() });

    let id = s.request(FsRequest::List("/".into()));
    let FsEvent::Listing {
        id: got, entries, ..
    } = next(&s)
    else {
        panic!("listing")
    };
    assert_eq!(got, id);
    let names: Vec<_> = entries
        .iter()
        .map(|e| (e.name.as_str(), e.is_dir, e.size))
        .collect();
    assert_eq!(names, [("pub", true, 0), ("hello.txt", false, 11)]);
    assert_eq!(entries[1].permissions, Some(0o644));

    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("dl.txt");
    let id = s.request(FsRequest::Download {
        remote: "/hello.txt".into(),
        local: local.clone(),
        ascii: false,
    });
    assert_eq!(next(&s), FsEvent::Done { id });
    assert_eq!(std::fs::read(&local).unwrap(), b"hi from ftp");

    let up = dir.path().join("up.bin");
    let big: Vec<u8> = (0..150_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(&up, &big).unwrap();
    let id = s.request(FsRequest::Upload {
        local: up,
        remote: "/pub/up.bin".into(),
        ascii: false,
    });
    assert_eq!(next(&s), FsEvent::Done { id });
    assert_eq!(fs.lock().unwrap().files["/pub/up.bin"], big);

    let id = s.request(FsRequest::Rename {
        from: "/pub/up.bin".into(),
        to: "/pub/renamed.bin".into(),
    });
    assert_eq!(next(&s), FsEvent::Done { id });
    let id = s.request(FsRequest::Remove {
        path: "/pub/renamed.bin".into(),
        is_dir: false,
    });
    assert_eq!(next(&s), FsEvent::Done { id });
    let id = s.request(FsRequest::Mkdir("/new".into()));
    assert_eq!(next(&s), FsEvent::Done { id });
    assert!(fs.lock().unwrap().dirs.contains("/new"));

    let id = s.request(FsRequest::Remove {
        path: "/missing".into(),
        is_dir: false,
    });
    assert!(matches!(next(&s), FsEvent::Failed { id: got, .. } if got == id));
}

#[test]
fn wrong_password_is_reported() {
    let fs = Arc::new(Mutex::new(Fs::default()));
    let s = FtpSession::connect(params(server(fs), "nope"), Arc::new(|| {}));
    assert!(matches!(next(&s), FsEvent::Unavailable(e) if e.contains("login failed")));
}
