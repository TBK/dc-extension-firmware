//! embassy-boot bootloader for the JetKVM DC Power Extension.
//!
//! Owns the power-loss-safe firmware swap: the application stages a new image
//! in the DFU partition and marks it; on the next boot this bootloader swaps
//! DFU into ACTIVE page by page, tracking progress in BOOTLOADER_STATE so an
//! interrupted swap resumes on the following boot. If the new application
//! never confirms itself (`mark_booted`), the watchdog resets the chip and the
//! swap is reverted - a crashing update rolls back automatically.
//!
//! This bootloader is installed once and never rewritten by updates.

#![no_std]
#![no_main]

mod direct_flash;

use core::cell::RefCell;

use cortex_m_rt::{entry, exception};
use direct_flash::DirectFlash;
use embassy_boot_rp::*;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::Duration;

/// Address envelope: the full 2 MiB part (settings sector at 0x80000 and the
/// DFU partition at 0x100000 both live within it).
const FLASH_SIZE: u32 = 2 * 1024 * 1024;

/// Start the crystal oscillator using the pico-SDK's exact sequence before
/// embassy's clock init runs.
///
/// embassy's `start_xosc` writes FREQ_RANGE and ENABLE to XOSC.CTRL in a single
/// register write; on this board's crystal that sequence fails to start the
/// oscillator on a cold power-on, hanging clock init at the PLL lock. The
/// pico-SDK writes FREQ_RANGE first, then ENABLE separately, which starts it
/// reliably. Re-writing CTRL on a running XOSC has no effect, so this is safe
/// on warm boots too.
fn prestart_xosc() {
    const XOSC_CTRL: *mut u32 = 0x4002_4000 as *mut u32;
    const XOSC_STATUS: *const u32 = 0x4002_4004 as *const u32;
    const XOSC_STARTUP: *mut u32 = 0x4002_400c as *mut u32;
    const FREQ_RANGE_1_15MHZ: u32 = 0xaa0;
    const ENABLE: u32 = 0xfab << 12;
    const STABLE: u32 = 1 << 31;
    // ~128 ms at 12 MHz, ample for this board's slow crystal.
    const STARTUP_DELAY: u32 = 6000;
    unsafe {
        core::ptr::write_volatile(XOSC_CTRL, FREQ_RANGE_1_15MHZ); // freq range first
        core::ptr::write_volatile(XOSC_STARTUP, STARTUP_DELAY);
        core::ptr::write_volatile(XOSC_CTRL, FREQ_RANGE_1_15MHZ | ENABLE); // then enable
        while core::ptr::read_volatile(XOSC_STATUS) & STABLE == 0 {
            core::hint::spin_loop();
        }
    }
}

#[entry]
fn main() -> ! {
    prestart_xosc();

    let p = embassy_rp::init(Default::default());


    // The watchdog stays armed across the jump into the application: an app
    // that never feeds it (hung, or too broken to reach its main loop) resets
    // the chip, and an unconfirmed update is then reverted by the swap logic.
    // DirectFlash feeds it during its own erase/program operations.
    let mut watchdog = embassy_rp::watchdog::Watchdog::new(p.WATCHDOG);
    watchdog.start(Duration::from_secs(8));
    let flash: Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, _> =
        Mutex::new(RefCell::new(DirectFlash::<FLASH_SIZE>));

    let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let active_offset = config.active.offset();
    let bl: BootLoader = BootLoader::prepare(config);

    unsafe { bl.load(embassy_rp::flash::FLASH_BASE as u32 + active_offset) }
}

#[unsafe(no_mangle)]
#[cfg_attr(target_os = "none", unsafe(link_section = ".HardFault.user"))]
unsafe extern "C" fn HardFault() {
    cortex_m::peripheral::SCB::sys_reset();
}

#[exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    cortex_m::peripheral::SCB::sys_reset();
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    cortex_m::peripheral::SCB::sys_reset();
}
