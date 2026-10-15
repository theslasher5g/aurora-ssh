//! Serial console backend using blocking I/O on dedicated threads.

use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;

use super::{Sink, TermCmd, TermEvent};
use crate::model::{FlowControl, Parity, SerialSettings};

pub fn available_ports() -> Vec<String> {
    serialport::available_ports()
        .map(|ports| ports.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default()
}

pub fn spawn(cfg: SerialSettings, events: Sink<TermEvent>) -> mpsc::UnboundedSender<TermCmd> {
    let (tx, mut rx) = mpsc::unbounded_channel::<TermCmd>();
    std::thread::spawn(move || {
        events.send(TermEvent::Status(format!(
            "Öffne {} @ {} baud…",
            cfg.port_name, cfg.baud_rate
        )));
        let builder = serialport::new(&cfg.port_name, cfg.baud_rate)
            .data_bits(match cfg.data_bits {
                5 => serialport::DataBits::Five,
                6 => serialport::DataBits::Six,
                7 => serialport::DataBits::Seven,
                _ => serialport::DataBits::Eight,
            })
            .stop_bits(if cfg.stop_bits == 2 {
                serialport::StopBits::Two
            } else {
                serialport::StopBits::One
            })
            .parity(match cfg.parity {
                Parity::None => serialport::Parity::None,
                Parity::Odd => serialport::Parity::Odd,
                Parity::Even => serialport::Parity::Even,
            })
            .flow_control(match cfg.flow_control {
                FlowControl::None => serialport::FlowControl::None,
                FlowControl::Software => serialport::FlowControl::Software,
                FlowControl::Hardware => serialport::FlowControl::Hardware,
            })
            .timeout(Duration::from_millis(50));
        let mut port = match builder.open() {
            Ok(p) => p,
            Err(e) => {
                events.send(TermEvent::Closed(Some(format!("{}: {e}", cfg.port_name))));
                return;
            }
        };
        let mut reader = match port.try_clone() {
            Ok(r) => r,
            Err(e) => {
                events.send(TermEvent::Closed(Some(e.to_string())));
                return;
            }
        };
        events.send(TermEvent::Connected);

        let stop = Arc::new(AtomicBool::new(false));
        let stop_reader = stop.clone();
        let ev = events.clone();
        let read_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while !stop_reader.load(Ordering::Relaxed) {
                match reader.read(&mut buf) {
                    Ok(0) => {}
                    Ok(n) => ev.send(TermEvent::Output(buf[..n].to_vec())),
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(e) => return Some(e.to_string()),
                }
            }
            None
        });

        let mut error = None;
        while let Some(cmd) = rx.blocking_recv() {
            match cmd {
                TermCmd::Input(bytes) => {
                    let bytes = translate_newlines(&bytes, cfg.line_ending.bytes());
                    if cfg.local_echo {
                        events.send(TermEvent::Output(bytes.clone()));
                    }
                    if let Err(e) = port.write_all(&bytes) {
                        error = Some(e.to_string());
                        break;
                    }
                }
                TermCmd::Resize { .. } => {}
                TermCmd::Upload { .. } => events.send(TermEvent::UploadDone(Err(
                    "Dateien hochladen geht nur bei SSH-Sessions".into(),
                ))),
                TermCmd::Close => break,
            }
            if read_thread.is_finished() {
                break;
            }
        }
        stop.store(true, Ordering::Relaxed);
        if let Ok(Some(e)) = read_thread.join() {
            error.get_or_insert(e);
        }
        events.send(TermEvent::Closed(error));
    });
    tx
}

/// The terminal sends `\r` for Enter; map it to the configured line ending.
fn translate_newlines(input: &[u8], ending: &[u8]) -> Vec<u8> {
    if ending == b"\r" {
        return input.to_vec();
    }
    let mut out = Vec::with_capacity(input.len());
    for &b in input {
        if b == b'\r' {
            out.extend_from_slice(ending);
        } else {
            out.push(b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn newline_translation() {
        assert_eq!(super::translate_newlines(b"ls\r", b"\r\n"), b"ls\r\n");
        assert_eq!(super::translate_newlines(b"ls\r", b"\r"), b"ls\r");
    }
}
