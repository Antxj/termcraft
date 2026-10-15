//! Telnet client (RFC 854) with the options a terminal needs: ECHO and
//! SUPPRESS-GO-AHEAD from the server, TERMINAL-TYPE (RFC 1091) and NAWS
//! (window size, RFC 1073) from us. Everything else is refused.

use crate::{Notify, Stream, StreamEvent};
use crossbeam_channel::{Receiver, Sender};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

const IAC: u8 = 255;
const NOP: u8 = 241;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

const OPT_ECHO: u8 = 1;
const OPT_SGA: u8 = 3;
const OPT_TTYPE: u8 = 24;
const OPT_NAWS: u8 = 31;
const TTYPE_SEND: u8 = 1;
const TTYPE_IS: u8 = 0;

/// Session settings of a Telnet connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelnetOptions {
    /// "Active mode": offer our options at once instead of waiting for
    /// the server to ask.
    pub active: bool,
    /// "Send newlines instead of ctrl-M": Enter sends CR LF.
    pub newlines: bool,
    /// "Telnet keepalives": a NOP every minute keeps idle links open.
    pub keepalive: bool,
}

impl Default for TelnetOptions {
    fn default() -> Self {
        Self {
            active: true,
            newlines: true,
            keepalive: false,
        }
    }
}

const KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(60);

/// What we offered in active mode; the server's answer to it is not
/// answered again (RFC 854: no acknowledgement of an acknowledgement).
const ACTIVE_OFFER: [[u8; 3]; 4] = [
    [IAC, WILL, OPT_NAWS],
    [IAC, WILL, OPT_TTYPE],
    [IAC, DO, OPT_SGA],
    [IAC, DO, OPT_ECHO],
];

/// Protocol state shared by the reader thread and the writer.
struct Shared {
    socket: TcpStream,
    naws: bool,
    size: (u16, u16),
    term: String,
    /// Offers still waiting for the server's answer: (our verb, option).
    offered: Vec<(u8, u8)>,
}

impl Shared {
    fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.socket.write_all(bytes)
    }

    fn send_naws(&mut self) -> std::io::Result<()> {
        let (rows, cols) = self.size;
        let mut msg = vec![IAC, SB, OPT_NAWS];
        for b in cols.to_be_bytes().into_iter().chain(rows.to_be_bytes()) {
            msg.push(b);
            if b == IAC {
                msg.push(IAC); // escape 255 inside subnegotiation
            }
        }
        msg.extend_from_slice(&[IAC, SE]);
        self.send(&msg)
    }
}

pub struct Telnet {
    shared: Arc<Mutex<Shared>>,
    events: Receiver<StreamEvent>,
    newlines: bool,
    /// Dropped with the connection: stops the keepalive thread.
    _keepalive: Option<Sender<()>>,
}

impl Telnet {
    pub fn connect(
        host: &str,
        port: u16,
        term: &str,
        rows: u16,
        cols: u16,
        options: TelnetOptions,
        notify: Notify,
    ) -> std::io::Result<Self> {
        let socket = TcpStream::connect((host, port))?;
        socket.set_nodelay(true)?;
        let reader = socket.try_clone()?;
        let mut shared = Shared {
            socket,
            naws: false,
            size: (rows, cols),
            term: term.to_ascii_uppercase(),
            offered: Vec::new(),
        };
        if options.active {
            shared.send(&ACTIVE_OFFER.concat())?;
            shared.offered = ACTIVE_OFFER.iter().map(|o| (o[1], o[2])).collect();
        }
        let shared = Arc::new(Mutex::new(shared));
        let (tx, events) = crossbeam_channel::unbounded();
        let thread_shared = shared.clone();
        std::thread::Builder::new()
            .name(format!("telnet {host}"))
            .spawn(move || read_loop(reader, thread_shared, tx, notify))?;
        let keepalive = options.keepalive.then(|| {
            let (stop, stopped) = crossbeam_channel::bounded::<()>(0);
            let shared = shared.clone();
            let _ = std::thread::Builder::new()
                .name(format!("telnet keepalive {host}"))
                .spawn(move || {
                    while let Err(crossbeam_channel::RecvTimeoutError::Timeout) =
                        stopped.recv_timeout(KEEPALIVE)
                    {
                        if shared.lock().expect("lock").send(&[IAC, NOP]).is_err() {
                            break;
                        }
                    }
                });
            stop
        });
        Ok(Self {
            shared,
            events,
            newlines: options.newlines,
            _keepalive: keepalive,
        })
    }
}

impl Stream for Telnet {
    /// Send user input; 0xFF is doubled and a lone CR becomes CR LF
    /// ("Send newlines instead of ctrl-M") or CR NUL.
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.shared
            .lock()
            .expect("lock")
            .send(&encode(bytes, self.newlines))
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        let mut s = self.shared.lock().expect("lock");
        if s.size != (rows, cols) {
            s.size = (rows, cols);
            if s.naws {
                let _ = s.send_naws();
            }
        }
    }

    fn events(&self) -> &Receiver<StreamEvent> {
        &self.events
    }
}

/// User input on the wire.
fn encode(bytes: &[u8], newlines: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for (i, &b) in bytes.iter().enumerate() {
        out.push(b);
        if b == IAC {
            out.push(IAC);
        } else if b == b'\r' && bytes.get(i + 1) != Some(&b'\n') {
            out.push(if newlines { b'\n' } else { 0 });
        }
    }
    out
}

enum State {
    Data,
    Iac,
    Verb(u8),
    Sub(Vec<u8>),
    SubIac(Vec<u8>),
}

/// Strips Telnet commands from the stream and answers negotiations.
pub(crate) struct Parser {
    state: State,
}

impl Parser {
    pub(crate) fn new() -> Self {
        Self { state: State::Data }
    }

    /// Returns (terminal bytes, protocol replies).
    pub(crate) fn feed(&mut self, input: &[u8], shared: &mut ShareView<'_>) -> (Vec<u8>, Vec<u8>) {
        let mut data = Vec::with_capacity(input.len());
        let mut reply = Vec::new();
        for &b in input {
            self.state = match std::mem::replace(&mut self.state, State::Data) {
                State::Data if b == IAC => State::Iac,
                State::Data => {
                    data.push(b);
                    State::Data
                }
                State::Iac => match b {
                    IAC => {
                        data.push(IAC);
                        State::Data
                    }
                    DO | DONT | WILL | WONT => State::Verb(b),
                    SB => State::Sub(Vec::new()),
                    _ => State::Data, // NOP, GA, etc.
                },
                State::Verb(verb) => {
                    negotiate(verb, b, &mut reply, shared);
                    State::Data
                }
                State::Sub(mut buf) => {
                    if b == IAC {
                        State::SubIac(buf)
                    } else {
                        buf.push(b);
                        State::Sub(buf)
                    }
                }
                State::SubIac(mut buf) => match b {
                    SE => {
                        subnegotiation(&buf, &mut reply, shared);
                        State::Data
                    }
                    IAC => {
                        buf.push(IAC);
                        State::Sub(buf)
                    }
                    _ => State::Data,
                },
            };
        }
        (data, reply)
    }
}

/// The parts of the connection state the parser may read or change.
pub(crate) struct ShareView<'a> {
    pub naws: &'a mut bool,
    pub size: (u16, u16),
    pub term: &'a str,
    pub offered: &'a mut Vec<(u8, u8)>,
}

fn negotiate(verb: u8, opt: u8, reply: &mut Vec<u8>, s: &mut ShareView<'_>) {
    // The answer to one of our offers: take note, do not answer it.
    let ours = match verb {
        DO | DONT => WILL,
        _ => DO,
    };
    if let Some(i) = s.offered.iter().position(|o| *o == (ours, opt)) {
        s.offered.remove(i);
        if opt == OPT_NAWS {
            *s.naws = verb == DO;
            if verb == DO {
                naws_message(s.size, reply);
            }
        }
        return;
    }
    match (verb, opt) {
        // Let the server echo and suppress go-ahead.
        (WILL, OPT_ECHO | OPT_SGA) => reply.extend_from_slice(&[IAC, DO, opt]),
        (WILL, _) => reply.extend_from_slice(&[IAC, DONT, opt]),
        (DO, OPT_TTYPE) => reply.extend_from_slice(&[IAC, WILL, OPT_TTYPE]),
        (DO, OPT_SGA) => reply.extend_from_slice(&[IAC, WILL, OPT_SGA]),
        (DO, OPT_NAWS) => {
            *s.naws = true;
            reply.extend_from_slice(&[IAC, WILL, OPT_NAWS]);
            naws_message(s.size, reply);
        }
        (DO, _) => reply.extend_from_slice(&[IAC, WONT, opt]),
        (DONT, OPT_NAWS) => *s.naws = false,
        _ => {} // WONT / DONT: nothing to do
    }
}

fn naws_message((rows, cols): (u16, u16), reply: &mut Vec<u8>) {
    reply.extend_from_slice(&[IAC, SB, OPT_NAWS]);
    for b in cols.to_be_bytes().into_iter().chain(rows.to_be_bytes()) {
        reply.push(b);
        if b == IAC {
            reply.push(IAC);
        }
    }
    reply.extend_from_slice(&[IAC, SE]);
}

fn subnegotiation(buf: &[u8], reply: &mut Vec<u8>, s: &mut ShareView<'_>) {
    if buf == [OPT_TTYPE, TTYPE_SEND] {
        reply.extend_from_slice(&[IAC, SB, OPT_TTYPE, TTYPE_IS]);
        reply.extend_from_slice(s.term.as_bytes());
        reply.extend_from_slice(&[IAC, SE]);
    }
}

fn read_loop(
    mut reader: TcpStream,
    shared: Arc<Mutex<Shared>>,
    tx: Sender<StreamEvent>,
    notify: Notify,
) {
    let mut parser = Parser::new();
    let mut buf = [0u8; 8192];
    let reason = loop {
        match reader.read(&mut buf) {
            Ok(0) => break None,
            Err(e) => break Some(e.to_string()),
            Ok(n) => {
                let data = {
                    let mut s = shared.lock().expect("lock");
                    let term = s.term.clone();
                    let size = s.size;
                    let mut offered = std::mem::take(&mut s.offered);
                    let mut view = ShareView {
                        naws: &mut s.naws,
                        size,
                        term: &term,
                        offered: &mut offered,
                    };
                    let out = parser.feed(&buf[..n], &mut view);
                    s.offered = offered;
                    if !out.1.is_empty() {
                        let _ = s.send(&out.1);
                    }
                    out.0
                };
                if !data.is_empty() && tx.send(StreamEvent::Output(data)).is_err() {
                    return;
                }
                notify();
            }
        }
    };
    let _ = tx.send(StreamEvent::Closed(reason));
    notify();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(p: &mut Parser, naws: &mut bool, input: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut offered = Vec::new();
        let mut view = ShareView {
            naws,
            size: (24, 80),
            term: "XTERM",
            offered: &mut offered,
        };
        p.feed(input, &mut view)
    }

    #[test]
    fn answers_to_active_offers_are_not_answered() {
        let mut p = Parser::new();
        let mut naws = false;
        let mut offered: Vec<(u8, u8)> = ACTIVE_OFFER.iter().map(|o| (o[1], o[2])).collect();
        let mut view = ShareView {
            naws: &mut naws,
            size: (24, 80),
            term: "XTERM",
            offered: &mut offered,
        };
        let input = [IAC, DO, OPT_NAWS, IAC, WILL, OPT_ECHO, IAC, DONT, OPT_TTYPE];
        let (_, reply) = p.feed(&input, &mut view);
        // Only the window size: no WILL NAWS / DO ECHO again.
        assert_eq!(reply, [IAC, SB, OPT_NAWS, 0, 80, 0, 24, IAC, SE]);
        assert_eq!(offered, [(DO, OPT_SGA)], "still waiting for SGA");
        assert!(naws);
    }

    #[test]
    fn enter_as_newline_or_cr_nul() {
        assert_eq!(encode(b"ls\r", true), b"ls\r\n");
        assert_eq!(encode(b"ls\r", false), b"ls\r\0");
        assert_eq!(encode(b"a\r\nb", true), b"a\r\nb", "CR LF stays");
        assert_eq!(encode(&[IAC], true), [IAC, IAC]);
    }

    #[test]
    fn strips_commands_and_answers_negotiation() {
        let mut p = Parser::new();
        let mut naws = false;
        let input = [
            b'h', b'i', IAC, WILL, OPT_ECHO, IAC, DO, OPT_NAWS, IAC, DO, 99, IAC, IAC, b'!',
        ];
        let (data, reply) = feed(&mut p, &mut naws, &input);
        assert_eq!(data, [b'h', b'i', IAC, b'!']);
        assert!(naws);
        let expected: Vec<u8> = [
            &[IAC, DO, OPT_ECHO][..],
            &[IAC, WILL, OPT_NAWS],
            &[IAC, SB, OPT_NAWS, 0, 80, 0, 24, IAC, SE],
            &[IAC, WONT, 99],
        ]
        .concat();
        assert_eq!(reply, expected);
    }

    #[test]
    fn terminal_type_subnegotiation_split_across_reads() {
        let mut p = Parser::new();
        let mut naws = false;
        let (_, r1) = feed(&mut p, &mut naws, &[IAC, SB, OPT_TTYPE]);
        assert!(r1.is_empty());
        let (data, r2) = feed(&mut p, &mut naws, &[TTYPE_SEND, IAC, SE, b'x']);
        assert_eq!(data, b"x");
        assert_eq!(
            r2,
            [&[IAC, SB, OPT_TTYPE, TTYPE_IS][..], b"XTERM", &[IAC, SE]].concat()
        );
    }

    #[test]
    fn end_to_end_with_local_server() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(&[IAC, DO, OPT_NAWS]).unwrap();
            s.write_all(b"login: ").unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 64];
            while !got.ends_with(b"root\r\0") {
                let n = s.read(&mut buf).unwrap();
                assert!(n > 0, "client closed early");
                got.extend_from_slice(&buf[..n]);
            }
            got
        });
        let passive = TelnetOptions {
            active: false,
            newlines: false,
            keepalive: false,
        };
        let mut t = Telnet::connect(
            "127.0.0.1",
            port,
            "xterm",
            30,
            100,
            passive,
            Arc::new(|| {}),
        )
        .unwrap();
        match t
            .events()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
        {
            StreamEvent::Output(data) => assert_eq!(data, b"login: "),
            StreamEvent::Closed(e) => panic!("closed: {e:?}"),
        }
        t.write(b"root\r").unwrap();
        let got = server.join().unwrap();
        // NAWS reply with our 100x30 size, then the login with CR NUL.
        assert!(got.starts_with(&[
            IAC, WILL, OPT_NAWS, IAC, SB, OPT_NAWS, 0, 100, 0, 30, IAC, SE
        ]));
        assert!(got.ends_with(b"root\r\0"));
    }
}
