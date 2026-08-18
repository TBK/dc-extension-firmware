//! Power-sensor abstraction with runtime detection: INA219 or INA700.
//!
//! Some DC-extension hardware revisions ship a TI INA700 (integrated shunt,
//! address 0x44) in place of the INA219 (external 0.01 ohm shunt, address
//! 0x40). Detection probes the INA700's MANUFACTURER_ID register a few times
//! at boot and falls back to assuming an INA219 - the safe default for all
//! existing hardware. Because the sensor kind is just data (the bus stays
//! owned here), a slow-to-wake INA700 that misses the boot probes is still
//! recovered at runtime: after several consecutive failed reads in INA219
//! mode the probe is retried, so neither chip can be locked out for a whole
//! boot by a transient NAK.
//!
//! Both chips run on their power-on defaults (continuous conversion), and
//! telemetry is derived from raw registers - no configuration or calibration
//! writes, so an unresponsive sensor costs nothing and reads self-heal the
//! moment it answers.

use embassy_time::{Duration, Timer, with_timeout};
use embedded_hal_async::i2c::I2c;

/// INA700 I2C address (A0 = GND).
const INA700_ADDR: u8 = 0x44;
/// INA700 registers.
const INA700_REG_VBUS: u8 = 0x05;
const INA700_REG_CURRENT: u8 = 0x07;
const INA700_REG_MANUFACTURER_ID: u8 = 0x3E;
/// MANUFACTURER_ID reads "TI" in ASCII.
const MFG_ID_TI: u16 = 0x5449;
/// INA700 scale factors (datasheet fixed LSBs).
const INA700_VBUS_LSB_MV: f32 = 3.125;
const INA700_CURRENT_LSB_MA: f32 = 0.480;

/// INA219 registers.
const INA219_REG_SHUNT: u8 = 0x01; // i16, 10 uV/LSB
const INA219_REG_BUS: u8 = 0x02; // bits 15..3 = voltage, 4 mV/LSB

/// After this many consecutive fully-failed reads in INA219 mode, retry the
/// INA700 probe: an INA700 whose rail was slow at boot would otherwise be
/// misclassified until the next power cycle. On a real (but faulty) INA219
/// board this costs one NAKed address per interval - negligible.
const REDETECT_AFTER_FAILURES: u8 = 5;

/// Bound on every I2C transaction: a wedged bus must not stall the main loop
/// (and with it the command interface).
const I2C_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ina219,
    Ina700,
}

/// The power monitor. Owns the I2C bus; the detected chip is data, so the
/// classification can be revised at runtime.
pub struct PowerSensor<I2C> {
    i2c: I2C,
    ina219_addr: u8,
    kind: Kind,
    consecutive_failures: u8,
}

/// Probe once for an INA700 by reading its MANUFACTURER_ID. Address-level:
/// boards without an INA700 have no device at 0x44 and NAK immediately.
async fn probe_ina700<I2C: I2c>(i2c: &mut I2C) -> bool {
    let mut id = [0u8; 2];
    matches!(
        with_timeout(
            I2C_TIMEOUT,
            i2c.write_read(INA700_ADDR, &[INA700_REG_MANUFACTURER_ID], &mut id),
        )
        .await,
        Ok(Ok(())) if u16::from_be_bytes(id) == MFG_ID_TI
    )
}

/// Read a big-endian 16-bit register, timeout-bounded.
async fn read_reg<I2C: I2c>(i2c: &mut I2C, addr: u8, reg: u8) -> Option<u16> {
    let mut data = [0u8; 2];
    match with_timeout(I2C_TIMEOUT, i2c.write_read(addr, &[reg], &mut data)).await {
        Ok(Ok(())) => Some(u16::from_be_bytes(data)),
        _ => None,
    }
}

/// Detect which sensor is fitted and take ownership of the bus.
///
/// Probes the INA700 MANUFACTURER_ID up to three times (transient boot-time
/// NAKs shouldn't misclassify an INA700 board), then falls back to INA219.
pub async fn detect<I2C: I2c>(mut i2c: I2C, ina219_addr: u8) -> PowerSensor<I2C> {
    let mut kind = Kind::Ina219;
    for attempt in 0..3 {
        if attempt > 0 {
            Timer::after(Duration::from_millis(50)).await;
        }
        if probe_ina700(&mut i2c).await {
            kind = Kind::Ina700;
            break;
        }
    }
    PowerSensor {
        i2c,
        ina219_addr,
        kind,
        consecutive_failures: 0,
    }
}

impl<I2C: I2c> PowerSensor<I2C> {
    /// Human-readable name of the detected sensor (for debug logging; the
    /// release build's logging macros compile to nothing).
    #[cfg_attr(release_build, allow(dead_code))]
    pub fn kind(&self) -> &'static str {
        match self.kind {
            Kind::Ina219 => "INA219",
            Kind::Ina700 => "INA700",
        }
    }

    /// Read `(voltage_mV, current_mA, power_mW)`.
    ///
    /// Returns zeros when the sensor doesn't answer - the caller always
    /// streams a status line, and reads self-heal once the sensor responds.
    /// Every transaction is timeout-bounded.
    pub async fn read(&mut self) -> (f32, f32, f32) {
        match self.kind {
            Kind::Ina219 => {
                let Some(bus_raw) = read_reg(&mut self.i2c, self.ina219_addr, INA219_REG_BUS).await
                else {
                    self.note_failure().await;
                    return (0.0, 0.0, 0.0);
                };
                self.consecutive_failures = 0;
                // Bus voltage lives in bits 15..3, 4 mV/LSB.
                let voltage_mv = f32::from(bus_raw >> 3) * 4.0;
                let Some(shunt_raw) =
                    read_reg(&mut self.i2c, self.ina219_addr, INA219_REG_SHUNT).await
                else {
                    return (voltage_mv, 0.0, 0.0);
                };
                // Shunt register is signed, 10 uV/LSB. With the 0.01 ohm
                // shunt: current_mA = shunt_uV / 10 = raw counts * 1.0.
                let current_ma = f32::from(shunt_raw as i16);
                (voltage_mv, current_ma, voltage_mv * current_ma / 1000.0)
            }
            Kind::Ina700 => {
                let Some(vbus_raw) = read_reg(&mut self.i2c, INA700_ADDR, INA700_REG_VBUS).await
                else {
                    return (0.0, 0.0, 0.0);
                };
                let voltage_mv = f32::from(vbus_raw as i16) * INA700_VBUS_LSB_MV;
                let Some(current_raw) =
                    read_reg(&mut self.i2c, INA700_ADDR, INA700_REG_CURRENT).await
                else {
                    return (voltage_mv, 0.0, 0.0);
                };
                let current_ma = f32::from(current_raw as i16) * INA700_CURRENT_LSB_MA;
                (voltage_mv, current_ma, voltage_mv * current_ma / 1000.0)
            }
        }
    }

    /// Track failed INA219 reads and periodically retry the INA700 probe, in
    /// case an INA700 board was misclassified because its sensor was still
    /// waking up during boot detection.
    async fn note_failure(&mut self) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= REDETECT_AFTER_FAILURES {
            self.consecutive_failures = 0;
            if probe_ina700(&mut self.i2c).await {
                self.kind = Kind::Ina700;
            }
        }
    }
}
