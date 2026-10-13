//! Serial port terminal (COM on Windows, /dev/tty* on Unix).

use crate::{Notify, Stream, StreamEvent};
use crossbeam_channel::Receiver;
use std::io::Read;
use std::time::Duration;

pub use serialport::{DataBits, FlowControl, Parity, StopBits};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialConfig {
    pub port: String,
    pub baud: u32,
    pub data_bits: DataBits,
    pub parity: Parity,
    pub stop_bits: StopBits,
    pub flow: FlowControl,
}

impl SerialConfig {
    /// From session parameter indices (reference "Advanced Serial settings"):
    /// data bits 0..3 = 5..8, stop bits 0..1 = 1/2, parity 0..4 =
    /// none/odd/even/mark/space, flow 0..3 = none/XON-XOFF/RTS-CTS/DSR-DTR.
    pub fn from_indices(
        port: &str,
        baud: u32,
        data_bits: u8,
        stop_bits: u8,
        parity: u8,
        flow: u8,
    ) -> Result<Self, String> {
        Ok(Self {
            port: port.to_string(),
            baud,
            data_bits: match data_bits {
                0 => DataBits::Five,
                1 => DataBits::Six,
                2 => DataBits::Seven,
                _ => DataBits::Eight,
            },
            stop_bits: if stop_bits == 1 {
                StopBits::Two
            } else {
                StopBits::One
            },
            parity: match parity {
                0 => Parity::None,
                1 => Parity::Odd,
                2 => Parity::Even,
                _ => return Err("mark / space parity is not supported".into()),
            },
            flow: match flow {
                0 => FlowControl::None,
                1 => FlowControl::Software,
                2 => FlowControl::Hardware,
                _ => return Err("DSR/DTR flow control is not supported".into()),
            },
        })
    }
}

pub struct Serial {
    port: Box<dyn serialport::SerialPort>,
    events: Receiver<StreamEvent>,
}

impl Serial {
    pub fn open(config: &SerialConfig, notify: Notify) -> Result<Self, String> {
        let port = serialport::new(&config.port, config.baud)
            .data_bits(config.data_bits)
            .parity(config.parity)
            .stop_bits(config.stop_bits)
            .flow_control(config.flow)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| format!("{}: {e}", config.port))?;
        let mut reader = port.try_clone().map_err(|e| e.to_string())?;
        let (tx, events) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name(format!("serial {}", config.port))
            .spawn(move || {
                let mut buf = [0u8; 4096];
                let reason = loop {
                    match reader.read(&mut buf) {
                        Ok(0) => continue,
                        Ok(n) => {
                            if tx.send(StreamEvent::Output(buf[..n].to_vec())).is_err() {
                                return;
                            }
                            notify();
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                        Err(e) => break Some(e.to_string()),
                    }
                };
                let _ = tx.send(StreamEvent::Closed(reason));
                notify();
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { port, events })
    }
}

impl Stream for Serial {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(&mut self.port, bytes)
    }

    fn events(&self) -> &Receiver<StreamEvent> {
        &self.events
    }
}

/// Names of the serial ports present on this machine.
pub fn available_ports() -> Vec<String> {
    serialport::available_ports()
        .map(|ports| ports.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_mapping() {
        let c = SerialConfig::from_indices("COM3", 9600, 3, 0, 2, 1).unwrap();
        assert_eq!(c.data_bits, DataBits::Eight);
        assert_eq!(c.stop_bits, StopBits::One);
        assert_eq!(c.parity, Parity::Even);
        assert_eq!(c.flow, FlowControl::Software);
        assert!(SerialConfig::from_indices("COM3", 9600, 3, 0, 3, 0).is_err());
        assert!(SerialConfig::from_indices("COM3", 9600, 3, 0, 0, 3).is_err());
    }

    #[test]
    fn missing_port_is_a_clean_error() {
        let c = SerialConfig::from_indices("COM-does-not-exist", 9600, 3, 0, 0, 0).unwrap();
        assert!(Serial::open(&c, std::sync::Arc::new(|| {})).is_err());
    }
}
