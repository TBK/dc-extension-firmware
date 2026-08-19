//! JetKVM DC Power Extension Firmware
//!
//! Power monitoring and control module for the JetKVM platform.
//! Runs on RP2040 and communicates with the main system via UART.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::i2c::{self, I2c};
use embassy_rp::peripherals::{I2C0, UART0};
use embassy_rp::uart::{self, BufferedUart};
use embassy_time::{Duration, Ticker, Timer, with_timeout};
use static_cell::StaticCell;
use ina219::AsyncIna219;
use ina219::address::Address;
use ina219::calibration::UnCalibrated;

// Debug builds: use defmt-rtt for logging and panic-probe for panic handling
#[cfg(debug_build)]
use defmt_rtt as _;
#[cfg(debug_build)]
use panic_probe as _;

// Release builds: use custom panic handler that sends UART message and resets
#[cfg(release_build)]
mod panic_handler;

// Logging macros that compile to nothing in release builds
#[cfg(debug_build)]
macro_rules! log_info {
    ($($arg:tt)*) => { defmt::info!($($arg)*) };
}
#[cfg(release_build)]
macro_rules! log_info {
    ($($arg:tt)*) => {};
}

#[cfg(debug_build)]
macro_rules! log_debug {
    ($($arg:tt)*) => { defmt::debug!($($arg)*) };
}
#[cfg(release_build)]
macro_rules! log_debug {
    ($($arg:tt)*) => {};
}

#[cfg(debug_build)]
macro_rules! log_warn {
    ($($arg:tt)*) => { defmt::warn!($($arg)*) };
}
#[cfg(release_build)]
macro_rules! log_warn {
    ($($arg:tt)*) => {};
}

#[cfg(debug_build)]
macro_rules! log_error {
    ($($arg:tt)*) => { defmt::error!($($arg)*) };
}
#[cfg(release_build)]
macro_rules! log_error {
    ($($arg:tt)*) => {};
}

mod config;
mod direct_flash;
mod flash_store;
mod fw_update;
mod power_control;
mod uart_handler;

use config::*;
use direct_flash::DirectFlash;
use flash_store::{FlashStore, PowerState, RestoreMode, SharedFlash};
use power_control::PowerControl;
use uart_handler::{Command, UartHandler};

bind_interrupts!(struct Irqs {
    UART0_IRQ => embassy_rp::uart::BufferedInterruptHandler<UART0>;
    I2C0_IRQ => embassy_rp::i2c::InterruptHandler<I2C0>;
});

/// Start the crystal oscillator using the pico-SDK's exact sequence before
/// embassy's clock init runs.
///
/// embassy's `start_xosc` writes FREQ_RANGE and ENABLE to XOSC.CTRL in a single
/// register write; on this board's crystal that sequence fails to start the
/// oscillator on a cold power-on, hanging clock init at the PLL lock. The
/// pico-SDK writes FREQ_RANGE first, then ENABLE separately, which starts it
/// reliably. Pre-starting here means embassy finds an already-stable crystal.
/// Re-writing CTRL on a running XOSC has no effect, so this is safe on warm
/// boots too.
fn prestart_xosc() {
    const XOSC_CTRL: *mut u32 = 0x4002_4000 as *mut u32;
    const XOSC_STATUS: *const u32 = 0x4002_4004 as *const u32;
    const XOSC_STARTUP: *mut u32 = 0x4002_400c as *mut u32;
    const FREQ_RANGE_1_15MHZ: u32 = 0xaa0;
    const ENABLE: u32 = 0xfab << 12;
    const STABLE: u32 = 1 << 31;
    // ~128 ms at 12 MHz - 2x the C firmware's 64x multiplier, ample for a slow
    // crystal. delay = ((12000 * 128) + 128) / 256.
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

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    prestart_xosc();

    let p = embassy_rp::init(Default::default());

    // Bring the UART up first (interrupt-driven, ring-buffered RX and TX) so
    // the status stream can start as early as possible. Buffered RX keeps
    // draining the hardware FIFO via the UART interrupt even while the main
    // loop is busy or sleeping, so commands are never dropped.
    let mut uart_config = uart::Config::default();
    uart_config.baudrate = BAUD_RATE;
    static TX_BUF: StaticCell<[u8; 256]> = StaticCell::new();
    static RX_BUF: StaticCell<[u8; 256]> = StaticCell::new();
    let tx_buf = &mut TX_BUF.init([0u8; 256])[..];
    let rx_buf = &mut RX_BUF.init([0u8; 256])[..];
    let uart = BufferedUart::new(
        p.UART0,
        p.PIN_16,
        p.PIN_17,
        Irqs,
        tx_buf,
        rx_buf,
        uart_config,
    );
    let (tx, rx) = uart.split();
    let mut uart_handler = UartHandler::new(tx, rx);

    // Take over the watchdog the bootloader armed: feeding it from the main
    // loop proves liveness; a hang resets the chip, and an unconfirmed update
    // is then reverted by the bootloader.
    let mut watchdog = embassy_rp::watchdog::Watchdog::new(p.WATCHDOG);
    watchdog.start(Duration::from_secs(8));

    // Let the power rails and the INA219 settle before the first I2C access
    // (matches the C firmware's 1 s boot settle). Must come after
    // embassy_rp::init, which sets up the time driver.
    Timer::after(Duration::from_millis(1000)).await;

    log_info!("JetKVM DC Power Extension starting...");
    log_info!("Version: {} {}", NAME, VERSION);

    // Flash is shared between the settings store and the firmware updater;
    // all erase/program goes through the direct bootrom sequence.
    static FLASH: StaticCell<SharedFlash> = StaticCell::new();
    let flash: &'static SharedFlash = FLASH.init(embassy_sync::blocking_mutex::Mutex::new(
        core::cell::RefCell::new(DirectFlash),
    ));
    let mut flash_store = FlashStore::new(flash);

    log_info!(
        "Flash store initialized - power_state: {}, restore_mode: {}",
        flash_store.power_state() as u8,
        flash_store.restore_mode() as u8
    );

    // Initialize power control GPIO
    let pwr_pin = Output::new(p.PIN_4, Level::Low);
    let mut power_ctrl = PowerControl::new(pwr_pin);

    // Apply restore mode on startup
    match flash_store.restore_mode() {
        RestoreMode::Off => {
            log_info!("Restore mode: OFF");
            power_ctrl.off();
        }
        RestoreMode::On => {
            log_info!("Restore mode: ON");
            power_ctrl.on();
        }
        RestoreMode::LastState => {
            log_info!("Restore mode: LAST_STATE");
            if flash_store.power_state() == PowerState::On {
                power_ctrl.on();
            } else {
                power_ctrl.off();
            }
        }
    }

    log_info!("UART initialized at {} baud", BAUD_RATE);

    // Initialize I2C for INA219
    let mut i2c_config = i2c::Config::default();
    i2c_config.frequency = 100_000;
    let i2c = I2c::new_async(p.I2C0, p.PIN_9, p.PIN_8, Irqs, i2c_config);

    log_info!("I2C initialized");

    // Wrap the INA219 without a boot-time reset/probe: `new_unchecked` performs
    // no I2C, so a sensor that is slow to come up cannot disable telemetry for
    // the whole boot. The device's power-on defaults (continuous shunt+bus,
    // 12-bit, 32 V / 320 mV) are exactly the mode we need, and telemetry reads
    // the raw bus/shunt registers and derives current/power itself, so no
    // calibration write is required. A transiently unresponsive sensor
    // self-heals: the timeout-guarded reads below return zeros until it
    // answers again.
    let mut ina219 = match Address::from_byte(INA219_ADDR) {
        Ok(address) => {
            log_info!("INA219 at address 0x{:02X} (POR defaults)", INA219_ADDR);
            Some(AsyncIna219::new_unchecked(i2c, address, UnCalibrated))
        }
        Err(_) => {
            log_error!("Invalid INA219 address constant - telemetry disabled");
            None
        }
    };

    log_info!("Entering main loop");

    // Telemetry cadence: one status line per second, matching the C firmware.
    let mut ticker = Ticker::every(Duration::from_secs(1));
    let mut boot_confirmed = false;

    // Main loop: react to incoming commands the instant they arrive (RX is
    // buffered in the background) while still emitting telemetry once a second.
    // `read_command` is cancel-safe, so losing the race to the ticker never
    // drops a partially received command line.
    loop {
        match select(uart_handler.read_command(), ticker.next()).await {
            Either::First(cmd) => {
                log_debug!("Received command: {}", cmd);

                match cmd {
                    Command::PowerOn => {
                        power_ctrl.on();
                        flash_store.set_power_state(PowerState::On);
                        log_info!("Power ON");
                    }
                    Command::PowerOff => {
                        power_ctrl.off();
                        flash_store.set_power_state(PowerState::Off);
                        log_info!("Power OFF");
                    }
                    Command::RestoreModeOff => {
                        flash_store.set_restore_mode(RestoreMode::Off);
                        log_info!("Restore mode set to OFF");
                    }
                    Command::RestoreModeOn => {
                        flash_store.set_restore_mode(RestoreMode::On);
                        log_info!("Restore mode set to ON");
                    }
                    Command::RestoreModeLastState => {
                        flash_store.set_restore_mode(RestoreMode::LastState);
                        log_info!("Restore mode set to LAST_STATE");
                    }
                    Command::Version => {
                        uart_handler.send_version().await;
                        log_info!("Sent version info");
                    }
                    Command::FwUpdate => {
                        // Blocks the telemetry stream for the duration of the
                        // transfer; on success this resets and the bootloader
                        // installs the update.
                        log_info!("Firmware update requested");
                        fw_update::run(&mut uart_handler, flash, &mut watchdog).await;
                        log_warn!("Firmware update aborted");
                    }
                    Command::Unknown => {
                        log_warn!("Unknown command received");
                    }
                }
            }
            Either::Second(_) => {
                watchdog.feed(Duration::from_secs(8));

                // First healthy telemetry tick: confirm this image to the
                // bootloader so an update is not reverted on the next reset.
                // Only writes the state sector when a swap actually happened -
                // marking unconditionally would erase+program it every boot.
                if !boot_confirmed {
                    boot_confirmed = true;
                    let config = embassy_boot_rp::FirmwareUpdaterConfig::from_linkerfile_blocking(
                        flash, flash,
                    );
                    let mut aligned = embassy_boot_rp::AlignedBuffer([0u8; 1]);
                    let mut updater =
                        embassy_boot_rp::BlockingFirmwareUpdater::new(config, &mut aligned.0);
                    if matches!(updater.get_state(), Ok(embassy_boot_rp::State::Swap)) {
                        let _ = updater.mark_booted();
                        log_info!("swap confirmed as booted");
                    }
                }

                // Read power metrics from INA219 (zeroed if the sensor is
                // unavailable or a read fails - we still stream a status line).
                // Each I2C read is bounded by a timeout: a stuck bus (a wedged
                // or flaky INA219 holding SDA) makes the embassy-rp read future
                // never complete, which would otherwise freeze the loop - and
                // with it the command interface. with_timeout keeps us alive.
                const I2C_TIMEOUT: Duration = Duration::from_millis(50);
                let (voltage_mv, current_ma, power_mw) = match ina219.as_mut() {
                    Some(dev) => match with_timeout(I2C_TIMEOUT, dev.bus_voltage()).await {
                        Ok(Ok(bus_voltage)) => {
                            let voltage_mv = bus_voltage.voltage_mv() as f32;
                            // Current = Shunt Voltage / Shunt Resistance
                            // shunt_voltage_uv / 1000 = mV, then / 10 (0.01 ohm) = mA
                            let (current_ma, power_mw) =
                                match with_timeout(I2C_TIMEOUT, dev.shunt_voltage()).await {
                                    Ok(Ok(shunt_voltage)) => {
                                        let current_ma =
                                            shunt_voltage.shunt_voltage_uv() as f32 / 10.0;
                                        (current_ma, voltage_mv * current_ma / 1000.0)
                                    }
                                    _ => {
                                        log_warn!("Failed to read shunt voltage");
                                        (0.0, 0.0)
                                    }
                                };
                            (voltage_mv, current_ma, power_mw)
                        }
                        _ => {
                            // Stream zeros this tick; reads self-heal once the
                            // sensor answers. Deliberately no recovery writes:
                            // an INA219 reset write on a marginal bus can wedge
                            // the I2C controller.
                            log_error!("Failed to read INA219 bus voltage");
                            (0.0, 0.0, 0.0)
                        }
                    },
                    None => (0.0, 0.0, 0.0),
                };

                // Determine power state from GPIO
                let power_state = if power_ctrl.is_on() {
                    PowerState::On
                } else {
                    PowerState::Off
                };

                log_debug!(
                    "V={} mV, I={} mA, P={} mW, state={}",
                    voltage_mv,
                    current_ma,
                    power_mw,
                    power_state as u8
                );

                // Always send a status line so the serial link stays alive even
                // without a working sensor.
                uart_handler
                    .send_status(
                        power_state,
                        voltage_mv,
                        current_ma,
                        power_mw,
                        flash_store.restore_mode(),
                    )
                    .await;
            }
        }
    }
}
