//! UART command processing for communication with main JetKVM system.
//!
//! RX is interrupt-driven and ring-buffered (embassy-rp `BufferedUart`), so
//! incoming bytes are never dropped even while the main loop is busy or
//! sleeping - matching the interrupt-driven RX of the original C firmware.

use embassy_rp::uart::{BufferedUartRx, BufferedUartTx};
use embedded_io_async::{BufRead, Write};
use heapless::String;

use crate::config::{NAME, VERSION};
use crate::flash_store::{PowerState, RestoreMode};

const RX_LINE_SIZE: usize = 128;
const TX_BUF_SIZE: usize = 128;
const CHUNK: usize = 64;

/// Commands received via UART
#[derive(Clone, Copy, Debug)]
#[cfg_attr(debug_build, derive(defmt::Format))]
pub enum Command {
    PowerOn,
    PowerOff,
    RestoreModeOff,
    RestoreModeOn,
    RestoreModeLastState,
    Version,
    FwUpdate,
    Unknown,
}

/// UART handler for command processing
pub struct UartHandler {
    tx: BufferedUartTx,
    rx: BufferedUartRx,
    line: [u8; RX_LINE_SIZE],
    line_pos: usize,
}

impl UartHandler {
    /// Create new UART handler
    pub fn new(tx: BufferedUartTx, rx: BufferedUartRx) -> Self {
        Self {
            tx,
            rx,
            line: [0u8; RX_LINE_SIZE],
            line_pos: 0,
        }
    }

    /// Await and return the next complete command line.
    ///
    /// Cancel-safe: the only await point is `fill_buf`, and partial lines are
    /// kept in `self.line` across calls, so racing this against a timer (the
    /// telemetry tick) never loses bytes or a partial command.
    pub async fn read_command(&mut self) -> Command {
        loop {
            // Copy out of the ring buffer, then release the borrow before we
            // touch `self.line` / call methods on `self`.
            let mut chunk = [0u8; CHUNK];
            let n;
            {
                let buf = match self.rx.fill_buf().await {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                n = buf.len().min(CHUNK);
                chunk[..n].copy_from_slice(&buf[..n]);
            }

            let mut used = 0;
            let mut cmd = None;
            for &ch in &chunk[..n] {
                used += 1;
                if self.line_pos < RX_LINE_SIZE - 1 {
                    self.line[self.line_pos] = ch;
                    self.line_pos += 1;
                }
                if ch == b'\n' || self.line_pos >= RX_LINE_SIZE - 1 {
                    cmd = Some(self.parse_command());
                    self.line_pos = 0;
                    break;
                }
            }
            // Only consume what we processed; any bytes after the newline stay
            // in the ring buffer for the next call.
            self.rx.consume(used);

            if let Some(c) = cmd {
                return c;
            }
        }
    }

    /// Parse the accumulated line into a command
    fn parse_command(&self) -> Command {
        let line = &self.line[..self.line_pos];

        // Convert to string for comparison (ignore non-UTF8)
        if let Ok(s) = core::str::from_utf8(line) {
            let s = s.trim();
            match s {
                "PWR_ON" => Command::PowerOn,
                "PWR_OFF" => Command::PowerOff,
                "RESTORE_MODE_OFF" => Command::RestoreModeOff,
                "RESTORE_MODE_ON" => Command::RestoreModeOn,
                "RESTORE_MODE_LAST_STATE" => Command::RestoreModeLastState,
                "VERSION" => Command::Version,
                "FW_UPDATE" => Command::FwUpdate,
                _ => Command::Unknown,
            }
        } else {
            Command::Unknown
        }
    }

    /// Send status update
    pub async fn send_status(
        &mut self,
        power_state: PowerState,
        voltage_mv: f32,
        current_ma: f32,
        power_mw: f32,
        restore_mode: RestoreMode,
    ) {
        let mut buf: String<TX_BUF_SIZE> = String::new();

        // Format: <power_state>;<voltage_mV>;<current_mA>;<power_mW>;<restore_mode>\n
        let _ = core::fmt::write(
            &mut buf,
            format_args!(
                "{};{:.2};{:.2};{:.2};{}\n",
                power_state as u8, voltage_mv, current_ma, power_mw, restore_mode as u8
            ),
        );

        let _ = self.tx.write_all(buf.as_bytes()).await;
    }

    /// Send version response
    pub async fn send_version(&mut self) {
        let mut buf: String<TX_BUF_SIZE> = String::new();

        // Format: EXTVER;<name>;<version>\n
        let _ = core::fmt::write(&mut buf, format_args!("EXTVER;{};{}\n", NAME, VERSION));
        let _ = self.tx.write_all(buf.as_bytes()).await;
    }

    /// Send raw bytes (used by the firmware-update protocol).
    pub async fn write_bytes(&mut self, bytes: &[u8]) {
        let _ = self.tx.write_all(bytes).await;
    }

    /// Read exactly `buf.len()` raw bytes from the UART.
    ///
    /// Bypasses the line parser - the firmware-update protocol transfers
    /// binary data. Any partial command line accumulated by `read_command`
    /// is discarded first so stray bytes can't leak into the binary stream.
    pub async fn read_exact(&mut self, buf: &mut [u8]) {
        self.line_pos = 0;
        let mut filled = 0;
        while filled < buf.len() {
            let chunk = match self.rx.fill_buf().await {
                Ok(c) if !c.is_empty() => c,
                _ => continue,
            };
            let n = chunk.len().min(buf.len() - filled);
            buf[filled..filled + n].copy_from_slice(&chunk[..n]);
            self.rx.consume(n);
            filled += n;
        }
    }
}
