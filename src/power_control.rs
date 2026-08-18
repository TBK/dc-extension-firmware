//! Power output control via GPIO.

use embassy_rp::gpio::Output;

/// Power control wrapper for GPIO output
pub struct PowerControl<'d> {
    pin: Output<'d>,
}

impl<'d> PowerControl<'d> {
    /// Create new power control instance
    pub fn new(pin: Output<'d>) -> Self {
        Self { pin }
    }

    /// Check if power is on
    pub fn is_on(&self) -> bool {
        self.pin.is_set_high()
    }

    /// Turn power on
    pub fn on(&mut self) {
        self.pin.set_high();
    }

    /// Turn power off
    pub fn off(&mut self) {
        self.pin.set_low();
    }
}
