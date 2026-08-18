//! Custom panic handler for release builds.
//!
//! Sends an error message via UART before resetting the device,
//! allowing the main JetKVM system to know something went wrong.

use core::panic::PanicInfo;

// UART0 register addresses (RP2040)
const UART0_BASE: u32 = 0x4003_4000;
const UART0_DR: *mut u32 = UART0_BASE as *mut u32;
const UART0_FR: *const u32 = (UART0_BASE + 0x18) as *const u32;

// Flag register bits
const UART_FR_BUSY: u32 = 1 << 3; // UART busy transmitting
const UART_FR_TXFF: u32 = 1 << 5; // TX FIFO full

/// Blocking write of a single byte to UART0.
/// Uses direct register access since we can't use Embassy in a panic handler.
#[inline(never)]
fn uart_write_byte(byte: u8) {
    unsafe {
        // Wait until TX FIFO is not full. Bounded like the final BUSY drain:
        // a wedged UART must not keep us from reaching sys_reset(). On
        // timeout the byte is dropped (message may truncate) but the reset
        // still happens.
        for _ in 0..2_000_000 {
            if (core::ptr::read_volatile(UART0_FR) & UART_FR_TXFF) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        // Write byte to data register
        core::ptr::write_volatile(UART0_DR, byte as u32);
    }
}

/// Write a string to UART0.
#[inline(never)]
fn uart_write_str(s: &str) {
    for byte in s.bytes() {
        uart_write_byte(byte);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // Disable interrupts to prevent further issues
    cortex_m::interrupt::disable();

    // Send panic message via UART
    // Format: PANIC;<message>\n
    uart_write_str("PANIC;DC extension firmware panic occurred");

    // Try to include location if available
    if let Some(location) = _info.location() {
        uart_write_str(" at ");
        uart_write_str(location.file());
        uart_write_str(":");
        // Simple number to string for line number
        let line = location.line();
        let mut buf = [0u8; 10];
        let mut n = line;
        let mut i = buf.len();
        loop {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        for byte in &buf[i..] {
            uart_write_byte(*byte);
        }
    }

    uart_write_str("\n");

    // Wait for the UART to finish shifting the whole message out before we
    // reset - a fixed spin can truncate a long "PANIC;... at <file>:<line>"
    // report. Bounded so a wedged UART can't hang the reset forever.
    for _ in 0..2_000_000 {
        if unsafe { core::ptr::read_volatile(UART0_FR) & UART_FR_BUSY } == 0 {
            break;
        }
        core::hint::spin_loop();
    }

    // Reset via Cortex-M SCB (standard, reliable method)
    cortex_m::peripheral::SCB::sys_reset()
}
