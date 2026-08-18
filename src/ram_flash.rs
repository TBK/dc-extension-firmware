//! Minimal, self-contained flash erase/program built directly on the bootrom,
//! mirroring the pico-sdk sequence: `connect_internal_flash` -> `flash_exit_xip`
//! -> erase/program -> `flash_flush_cache` -> `flash_enter_cmd_xip`, executed
//! from a `.data.ram_func` routine with PRIMASK-masked interrupts.
//!
//! This replaces embassy-rp's `blocking_write`/`blocking_erase`, which hang on
//! this board's hardware; the direct sequence has been verified working there.
//!
//! Safety model: single-core firmware with no DMA (the buffered UART is
//! interrupt-driven, and interrupts are masked for the duration of the
//! XIP-off window); flash reads elsewhere are memory-mapped XIP, which is
//! re-enabled before returning. After the ROM `flash_enter_cmd_xip`, XIP runs
//! in slower serial-read mode until the next reboot - negligible for this
//! firmware's post-write read load.

use embassy_rp::rom_data;

/// ROM entry points, looked up while XIP is still enabled.
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
/// `erase_bytes` == 0 skips the erase; `data`/`len` == null/0 skips program.
#[unsafe(link_section = ".data.ram_func")]
#[inline(never)]
unsafe fn xip_off_op(funcs: &RomFuncs, addr: u32, erase_bytes: usize, data: *const u8, len: usize) {
    unsafe {
        (funcs.connect_internal_flash)();
        (funcs.flash_exit_xip)();
        if erase_bytes != 0 {
            // block_size 1<<31 => plain 4K sector erases only, like embassy.
            (funcs.flash_range_erase)(addr, erase_bytes, 1 << 31, 0);
        }
        if len != 0 {
            (funcs.flash_range_program)(addr, data, len);
        }
        (funcs.flash_flush_cache)();
        (funcs.flash_enter_cmd_xip)();
    }
}

/// Erase `len` bytes starting at flash offset `addr` (both 4096-aligned).
pub fn erase(addr: u32, len: usize) {
    debug_assert!(addr.is_multiple_of(4096) && len.is_multiple_of(4096));
    let funcs = lookup();
    cortex_m::interrupt::free(|_| unsafe {
        xip_off_op(&funcs, addr, len, core::ptr::null(), 0);
    });
}

/// Program `data` at flash offset `addr` (256-aligned offset and length).
pub fn program(addr: u32, data: &[u8]) {
    debug_assert!(addr.is_multiple_of(256) && data.len().is_multiple_of(256));
    let funcs = lookup();
    cortex_m::interrupt::free(|_| unsafe {
        xip_off_op(&funcs, addr, 0, data.as_ptr(), data.len());
    });
}
