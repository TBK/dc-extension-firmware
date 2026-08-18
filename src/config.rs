//! Configuration constants for the JetKVM DC Power Extension.

// UART configuration
pub const BAUD_RATE: u32 = 115_200;

// INA219 configuration. The chip runs uncalibrated (POR defaults): telemetry
// reads the raw bus/shunt registers and derives current from the 0.01 ohm
// shunt in main.rs, so no calibration constants are needed.
pub const INA219_ADDR: u8 = 0x40;

// Flash configuration
pub const FLASH_TARGET_OFFSET: u32 = 512 * 1024;

// Version info
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAME: &str = "jetkvm-dc";
