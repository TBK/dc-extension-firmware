//! Flash driver built directly on the RP2040 bootrom, bypassing embassy-rp's
//! flash wrapper. Shared design with the ATX extension firmware.
//!
//! This board's 2 MiB chip accepts embassy-rp's flash path too, but the
//! direct sequence - ROM calls from a `.data.ram_func` routine,
//! PRIMASK-masked interrupts, bootrom `flash_enter_cmd_xip` for XIP
//! re-entry - is the one proven across both extension boards, and it feeds
//! the hardware watchdog per operation (raw LOAD register write), so
//! multi-sector erases can never outlast the 8 s budget regardless of the
//! flash chip's speed.
//!
//! Implements `embedded-storage`'s blocking NorFlash traits so it can back
//! embassy-boot partitions in both the application and the bootloader.
//!
//! After `flash_enter_cmd_xip`, XIP runs in slower serial-read mode until the
//! next reboot - negligible for the read loads involved.

use embassy_rp::rom_data;
use embedded_storage::nor_flash::{
    ErrorType, NorFlash, NorFlashError, NorFlashErrorKind, ReadNorFlash,
};

const XIP_BASE: u32 = 0x1000_0000;
const SECTOR_SIZE: u32 = 4096;
const PAGE_SIZE: u32 = 256;

/// Feed the hardware watchdog directly (LOAD register). The watchdog is
/// armed by the bootloader and runs during flash operations; long erase
/// sequences must keep it fed. 0xF42400 = 8 s (1 MHz tick, x2 per RP2040-E1).
fn feed_watchdog() {
    // WATCHDOG registers: 0x00 CTRL, 0x04 LOAD.
    const WATCHDOG_LOAD: *mut u32 = 0x4005_8004 as *mut u32;
    unsafe { core::ptr::write_volatile(WATCHDOG_LOAD, 0x00F4_2400) };
}

/// ROM entry points, looked up while XIP is enabled (the lookup itself runs
/// from flash; the ROM table is mask ROM).
#[repr(C)]
struct RomFuncs {
    connect_internal_flash: unsafe extern "C" fn(),
    flash_exit_xip: unsafe extern "C" fn(),
    flash_range_erase: unsafe extern "C" fn(u32, usize, u32, u8),
    flash_range_program: unsafe extern "C" fn(u32, *const u8, usize),
    flash_flush_cache: unsafe extern "C" fn(),
    flash_enter_cmd_xip: unsafe extern "C" fn(),
}

fn lookup() -> RomFuncs {
    RomFuncs {
        connect_internal_flash: rom_data::connect_internal_flash::ptr(),
        flash_exit_xip: rom_data::flash_exit_xip::ptr(),
        flash_range_erase: rom_data::flash_range_erase::ptr(),
        flash_range_program: rom_data::flash_range_program::ptr(),
        flash_flush_cache: rom_data::flash_flush_cache::ptr(),
        flash_enter_cmd_xip: rom_data::flash_enter_cmd_xip::ptr(),
    }
}

/// The XIP-off window. Everything here must be RAM/ROM/registers only.
/// `erase_bytes` == 0 skips the erase; `len` == 0 skips the program.
#[unsafe(link_section = ".data.ram_func")]
#[inline(never)]
unsafe fn xip_off_op(funcs: &RomFuncs, addr: u32, erase_bytes: usize, data: *const u8, len: usize) {
    unsafe {
        (funcs.connect_internal_flash)();
        (funcs.flash_exit_xip)();
        if erase_bytes != 0 {
            // block_size 1<<31 => plain 4K sector erases only.
            (funcs.flash_range_erase)(addr, erase_bytes, 1 << 31, 0);
        }
        if len != 0 {
            (funcs.flash_range_program)(addr, data, len);
        }
        (funcs.flash_flush_cache)();
        (funcs.flash_enter_cmd_xip)();
    }
}

/// Flash-size-bounded direct-bootrom flash driver.
pub struct DirectFlash<const SIZE: u32>;

#[derive(Debug)]
pub struct FlashError(NorFlashErrorKind);

impl NorFlashError for FlashError {
    fn kind(&self) -> NorFlashErrorKind {
        self.0
    }
}

impl<const SIZE: u32> ErrorType for DirectFlash<SIZE> {
    type Error = FlashError;
}

impl<const SIZE: u32> ReadNorFlash for DirectFlash<SIZE> {
    const READ_SIZE: usize = 1;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        if offset.checked_add(bytes.len() as u32).is_none_or(|end| end > SIZE) {
            return Err(FlashError(NorFlashErrorKind::OutOfBounds));
        }
        // Memory-mapped XIP read.
        let src = (XIP_BASE + offset) as *const u8;
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = unsafe { core::ptr::read_volatile(src.add(i)) };
        }
        Ok(())
    }

    fn capacity(&self) -> usize {
        SIZE as usize
    }
}

impl<const SIZE: u32> NorFlash for DirectFlash<SIZE> {
    const WRITE_SIZE: usize = 1;
    const ERASE_SIZE: usize = SECTOR_SIZE as usize;

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        if !from.is_multiple_of(SECTOR_SIZE) || !to.is_multiple_of(SECTOR_SIZE) || from > to {
            return Err(FlashError(NorFlashErrorKind::NotAligned));
        }
        if to > SIZE {
            return Err(FlashError(NorFlashErrorKind::OutOfBounds));
        }
        let funcs = lookup();
        // One sector per XIP-off window, watchdog fed in between: multi-
        // sector erases on a slow chip must not starve the watchdog and the
        // interrupt-masked windows stay short.
        let mut addr = from;
        while addr < to {
            feed_watchdog();
            cortex_m::interrupt::free(|_| unsafe {
                xip_off_op(&funcs, addr, SECTOR_SIZE as usize, core::ptr::null(), 0);
            });
            addr += SECTOR_SIZE;
        }
        Ok(())
    }

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        if bytes.is_empty() {
            return Ok(());
        }
        if offset
            .checked_add(bytes.len() as u32)
            .is_none_or(|end| end > SIZE)
        {
            return Err(FlashError(NorFlashErrorKind::OutOfBounds));
        }
        // The bootrom programs whole 256-byte pages. Page-aligned spans are
        // programmed straight from the caller's buffer in a single XIP-off
        // window; unaligned head/tail bytes are emulated (WRITE_SIZE = 1,
        // like embassy-rp's driver) by read-merging the surrounding page via
        // XIP. Re-programming unchanged bytes is a no-op at the NOR level.
        let funcs = lookup();
        let mut remaining = bytes;
        let mut pos = offset;
        while !remaining.is_empty() {
            if pos.is_multiple_of(PAGE_SIZE) && remaining.len() >= PAGE_SIZE as usize {
                // Fast path: whole consecutive pages in one window.
                let span = remaining.len() & !(PAGE_SIZE as usize - 1);
                feed_watchdog();
                cortex_m::interrupt::free(|_| unsafe {
                    xip_off_op(&funcs, pos, 0, remaining.as_ptr(), span);
                });
                remaining = &remaining[span..];
                pos += span as u32;
            } else {
                // Slow path: merge one partial page.
                let page_base = pos & !(PAGE_SIZE - 1);
                let in_page_off = (pos - page_base) as usize;
                let take = remaining.len().min(PAGE_SIZE as usize - in_page_off);
                let mut buf = [0xFFu8; PAGE_SIZE as usize];
                let src = (XIP_BASE + page_base) as *const u8;
                for (i, b) in buf.iter_mut().enumerate() {
                    *b = unsafe { core::ptr::read_volatile(src.add(i)) };
                }
                buf[in_page_off..in_page_off + take].copy_from_slice(&remaining[..take]);
                feed_watchdog();
                cortex_m::interrupt::free(|_| unsafe {
                    xip_off_op(&funcs, page_base, 0, buf.as_ptr(), PAGE_SIZE as usize);
                });
                remaining = &remaining[take..];
                pos += take as u32;
            }
        }
        Ok(())
    }
}
