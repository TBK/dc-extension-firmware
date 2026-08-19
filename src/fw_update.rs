//! Firmware update over UART - no BOOTSEL button, no USB access required.
//!
//! The JetKVM sends `FW_UPDATE\n`, then streams the new application image
//! (the raw binary linked at the ACTIVE partition, as produced by
//! `package.sh`). The image is written to the DFU partition via
//! embassy-boot's `FirmwareUpdater`, CRC-checked end to end, marked, and the
//! device resets; the bootloader then performs a power-loss-safe page-by-page
//! swap into ACTIVE. If the new firmware fails to confirm itself
//! (`mark_booted`) before the watchdog fires, the bootloader reverts to the
//! previous image automatically.
//!
//! Wire protocol (device responses in caps):
//!
//! ```text
//! -> "FW_UPDATE\n"
//! <- "OK\n"
//! -> u32 LE image length
//! <- "RECV\n"            (or "ERR:size\n")
//! loop per 4 KiB chunk:
//!   -> chunk bytes (last chunk may be short)
//!   <- "ACK\n"           each chunk is written to the DFU partition before
//!                         the ACK, so the sender's wait provides flow control
//! -> u32 LE CRC-32 (IEEE, reflected - zlib/`python3 -c 'zlib.crc32'`)
//! <- "CRCOK\n"           (or "ERR:crc\n" / "ERR:verify\n" / "ERR:flash\n")
//! <- "FLASH\n"           then the device marks the update and resets; the
//!                         bootloader swaps and the new firmware boots
//! ```
//!
//! Safety properties:
//! - The live image is untouched by this code entirely; the swap is the
//!   bootloader's job and is resumable across power loss at any point.
//! - A failed or interrupted transfer (bad CRC, sender stall) changes
//!   nothing: the update is only marked after full verification.

use embassy_boot_rp::{AlignedBuffer, BlockingFirmwareState, FirmwareUpdaterConfig, State};
use embassy_time::{Duration, with_timeout};
use embedded_storage::nor_flash::NorFlash;

use crate::flash_store::SharedFlash;
use crate::uart_handler::UartHandler;

/// Maximum accepted image size: the ACTIVE partition (see memory.x).
const MAX_IMAGE_SIZE: u32 = 256 * 1024;

/// DFU partition flash offset (see memory.x) - used for CRC read-back.
const DFU_OFFSET: u32 = 0x10_0000;

const CHUNK_SIZE: usize = 4096;
const SECTOR_SIZE: usize = 4096;

/// Abort the transfer if the sender goes quiet: without this a stalled or
/// killed sender leaves the firmware consuming every future UART byte as
/// "chunk data", bricking the command interface until enough bytes drain.
const RX_TIMEOUT: Duration = Duration::from_secs(3);

/// CRC-32 (IEEE 802.3, reflected, init/xorout 0xFFFFFFFF) - matches zlib.
fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    crc
}

/// Run one update transaction. Returns on error or abort; on success the
/// device resets and the bootloader installs the update.
pub async fn run(
    uart: &mut UartHandler,
    flash: &'static SharedFlash,
    watchdog: &mut embassy_rp::watchdog::Watchdog,
) {
    uart.write_bytes(b"OK\n").await;

    // Image length
    let mut word = [0u8; 4];
    if with_timeout(RX_TIMEOUT, uart.read_exact(&mut word)).await.is_err() {
        uart.write_bytes(b"ERR:timeout\n").await;
        return;
    }
    let len = u32::from_le_bytes(word);
    if len == 0 || len > MAX_IMAGE_SIZE {
        uart.write_bytes(b"ERR:size\n").await;
        return;
    }
    // Split the updater config: the DFU partition is driven directly so
    // sectors can be erased on demand between chunks (a bulk erase of the
    // whole 260K partition takes several seconds on some flash chips - too
    // long a silent gap for the sender, and uncomfortably close to the
    // watchdog). The state partition handles the update marking.
    let config = FirmwareUpdaterConfig::from_linkerfile_blocking(flash, flash);
    let mut dfu = config.dfu;
    let mut state_buf = AlignedBuffer([0u8; 1]);
    let mut state = BlockingFirmwareState::new(config.state, &mut state_buf.0);

    // Refuse to start from a mid-swap state (mirrors prepare_update's check).
    if !matches!(
        state.get_state(),
        Ok(State::Boot | State::Revert | State::DfuDetach)
    ) {
        uart.write_bytes(b"ERR:flash\n").await;
        return;
    }
    uart.write_bytes(b"RECV\n").await;

    // Receive into the DFU partition, one ACK per chunk. The sender waits for
    // each ACK, so no bytes arrive while interrupts are masked for flash ops;
    // sectors are erased just ahead of the write, inside the ACKed window.

    let mut chunk = AlignedBuffer([0xFFu8; CHUNK_SIZE]);
    let mut crc = 0xFFFF_FFFFu32;
    let mut offset = 0usize;
    let mut erased_to = 0usize;
    while offset < len as usize {
        let n = CHUNK_SIZE.min(len as usize - offset);
        if with_timeout(RX_TIMEOUT, uart.read_exact(&mut chunk.0[..n]))
            .await
            .is_err()
        {
            uart.write_bytes(b"ERR:timeout\n").await;
            return;
        }
        crc = crc32_update(crc, &chunk.0[..n]);

        watchdog.feed(Duration::from_secs(8));
        while erased_to < offset + n {
            if dfu
                .erase(erased_to as u32, (erased_to + SECTOR_SIZE) as u32)
                .is_err()
            {
                uart.write_bytes(b"ERR:flash\n").await;
                return;
            }
            erased_to += SECTOR_SIZE;
        }
        if dfu.write(offset as u32, &chunk.0[..n]).is_err() {
            uart.write_bytes(b"ERR:flash\n").await;
            return;
        }
        offset += n;

        uart.write_bytes(b"ACK\n").await;
    }
    let received_crc = !crc;

    // Sender's CRC over the image
    if with_timeout(RX_TIMEOUT, uart.read_exact(&mut word)).await.is_err() {
        uart.write_bytes(b"ERR:timeout\n").await;
        return;
    }
    let expected_crc = u32::from_le_bytes(word);
    if received_crc != expected_crc {
        uart.write_bytes(b"ERR:crc\n").await;
        return;
    }

    // Verify what actually landed in the DFU partition (XIP read-back).
    watchdog.feed(Duration::from_secs(8));
    let staged = unsafe {
        core::slice::from_raw_parts(
            (embassy_rp::flash::FLASH_BASE as u32 + DFU_OFFSET) as *const u8,
            len as usize,
        )
    };
    let staged_crc = !crc32_update(0xFFFF_FFFF, staged);
    if staged_crc != expected_crc {
        uart.write_bytes(b"ERR:verify\n").await;
        return;
    }

    // Mark the update for the bootloader and reset into it.
    watchdog.feed(Duration::from_secs(8));
    if state.mark_updated().is_err() {
        uart.write_bytes(b"ERR:flash\n").await;
        return;
    }
    uart.write_bytes(b"CRCOK\nFLASH\n").await;
    drain_uart_tx();
    cortex_m::peripheral::SCB::sys_reset();
}

/// Wait until the UART has physically shifted out everything queued, so the
/// final protocol messages reach the JetKVM before the reset.
fn drain_uart_tx() {
    const UART0_FR: *const u32 = 0x4003_4018 as *const u32;
    const BUSY: u32 = 1 << 3;
    for _ in 0..4_000_000u32 {
        if unsafe { core::ptr::read_volatile(UART0_FR) } & BUSY == 0 {
            break;
        }
        core::hint::spin_loop();
    }
}
