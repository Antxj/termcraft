//! Byte-stream terminal backends with the same shape as a local PTY:
//! output arrives on a channel from a reader thread, input is written
//! directly. [`telnet`] speaks the Telnet protocol (RFC 854) with the
//! options a terminal needs; [`rlogin`] is the Rsh session protocol;
//! [`serial`] drives a COM / tty port.

pub mod ftp;
pub mod ftps;
pub mod rlogin;
pub mod s3;
pub mod serial;
pub mod telnet;

use crossbeam_channel::Receiver;
use std::sync::Arc;

pub enum StreamEvent {
    Output(Vec<u8>),
    /// The connection or port was closed (with a reason, if any).
    Closed(Option<String>),
}

/// A connected backend.
pub trait Stream: Send {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    fn resize(&mut self, _rows: u16, _cols: u16) {}
    fn events(&self) -> &Receiver<StreamEvent>;
}

pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// A file browser connection (FTP, S3): requests in, [`FsEvent`]s out.
///
/// [`FsEvent`]: tc_core::remote_fs::FsEvent
pub trait FsBackend: Send {
    fn request(&self, request: tc_core::remote_fs::FsRequest) -> u64;
    fn events(&self) -> &crossbeam_channel::Receiver<tc_core::remote_fs::FsEvent>;
}
