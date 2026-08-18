# Hardware Validation Guide

Procedures for validating the firmware on real hardware — after changes to
UART handling, flash persistence, or telemetry, or when bringing up a new
board revision.

## Test Environment Setup

### Required Hardware

- RP2040-based board with INA219
- DC power supply (0-30V, 0-5A recommended)
- Resistive load (for current testing)
- USB-UART adapter for monitoring
- Multimeter (for voltage/current reference)
- Logic analyzer (optional, for timing analysis)

### Software Requirements

```bash
# Install Rust toolchain
rustup target add thumbv6m-none-eabi
cargo install probe-rs-tools
cargo install cargo-binutils  # rust-objcopy, used by package.sh

# For serial monitoring
# Linux
sudo apt install minicom picocom
```

### Debug Build

Build with defmt logging enabled for testing:

```bash
cargo build  # dev profile enables defmt/RTT logging
```

### Connecting defmt Output

```bash
# Using probe-rs (build, flash, and stream RTT)
cargo run

# Or attach to running firmware
probe-rs attach --chip RP2040 target/thumbv6m-none-eabi/debug/jetkvm-dc-extension
```

---

## Hardware Validation Tests

### 1. UART Communication Test

**Objective**: Verify bidirectional UART works correctly

**Setup**:
- Connect USB-UART adapter to GPIO16 (TX) and GPIO17 (RX)
- Open serial terminal at 115200 baud

**Test Cases**:

| Test            | Command Sent | Expected Response           |
|-----------------|--------------|-----------------------------|
| Version query   | `VERSION\n`  | `EXTVER;jetkvm-dc;0.2.0\n`  |
| Power on        | `PWR_ON\n`   | Status updates show `1;...` |
| Power off       | `PWR_OFF\n`  | Status updates show `0;...` |
| Status interval | (wait)       | Status every ~1 second      |

**Verification Script** (Python):
```python
import serial
import time

ser = serial.Serial('/dev/ttyUSB0', 115200, timeout=2)

# Test VERSION command
ser.write(b'VERSION\n')
response = ser.readline()
assert response.startswith(b'EXTVER;'), f"Unexpected: {response}"
print(f"Version: {response.decode().strip()}")

# Test PWR_ON
ser.write(b'PWR_ON\n')
time.sleep(1.5)
status = ser.readline()
assert status.startswith(b'1;'), f"Power should be ON: {status}"
print(f"Power ON status: {status.decode().strip()}")

# Test PWR_OFF
ser.write(b'PWR_OFF\n')
time.sleep(1.5)
status = ser.readline()
assert status.startswith(b'0;'), f"Power should be OFF: {status}"
print(f"Power OFF status: {status.decode().strip()}")

print("All UART tests passed!")
ser.close()
```

### 2. Power Control GPIO Test

**Objective**: Verify GPIO4 controls power output correctly

**Setup**:
- Connect LED or multimeter to GPIO4
- Monitor pin state during commands

**Test Cases**:

| Command                          | GPIO4 State | Verification                   |
|----------------------------------|-------------|--------------------------------|
| `PWR_ON\n`                       | HIGH (3.3V) | LED on / multimeter reads 3.3V |
| `PWR_OFF\n`                      | LOW (0V)    | LED off / multimeter reads 0V  |
| Boot (restore=Off)               | LOW         | Pin starts low                 |
| Boot (restore=On)                | HIGH        | Pin starts high                |
| Boot (restore=LastState, was On) | HIGH        | Pin matches last state         |

### 3. INA219 Accuracy Test

**Objective**: Verify voltage/current readings are accurate

**Setup**:
- Precision power supply with known output
- Precision shunt resistor (0.01Ω ±1%)
- Reference multimeter

**Test Points**:

| Applied Voltage | Applied Current | Expected Reading | Tolerance      |
|-----------------|-----------------|------------------|----------------|
| 5.000V          | 0.000A          | 5000mV, 0mA      | ±50mV, ±10mA   |
| 12.000V         | 0.500A          | 12000mV, 500mA   | ±120mV, ±25mA  |
| 24.000V         | 1.000A          | 24000mV, 1000mA  | ±240mV, ±50mA  |
| 30.000V         | 2.000A          | 30000mV, 2000mA  | ±300mV, ±100mA |

**Tolerance Calculation**:
- Voltage: ±1% of reading
- Current: ±5% of reading (due to shunt tolerance and ADC resolution)

### 4. Flash Persistence Test

**Objective**: Verify data survives power cycles and flash wear

**Test Sequence**:

1. **Initial State Test**:
   ```
   1. Erase flash (full chip erase)
   2. Boot firmware
   3. Verify default state (power=Off, restore=Off)
   ```

2. **Single Write Test**:
   ```
   1. Send PWR_ON
   2. Send RESTORE_MODE_LAST_STATE
   3. Power cycle
   4. Verify power comes on automatically
   ```

3. **Circular Buffer Test**:
   ```
   1. Write 20 state changes
   2. Verify each write succeeds
   3. Power cycle
   4. Verify last state is correct
   ```

4. **Sector Wrap Test**:
   ```
   1. Write 17+ state changes (exceeds 16 entries per sector)
   2. Verify sector erase and wrap occurs
   3. Power cycle
   4. Verify state is correct
   ```

### 5. Timing Test

| Timing Aspect          | Expected Value | Tolerance |
|------------------------|----------------|-----------|
| Status update interval | 1000ms         | ±50ms     |
| Command response time  | <100ms         | -         |
| Boot to first status   | <2000ms        | -         |

### 6. JetKVM Integration Test

**Objective**: Verify firmware works with actual JetKVM system

**Prerequisites**:
- JetKVM device with DC extension port
- Firmware flashed to DC extension

**Test Procedure**:

1. **Connection Test**:
   - Connect DC extension to JetKVM
   - Verify JetKVM recognizes the extension
   - Check web UI shows power status

2. **Control Test**:
   - Toggle power via JetKVM web UI
   - Verify power state changes
   - Verify status updates in UI

3. **Monitoring Test**:
   - Apply load to DC output
   - Verify voltage/current/power display in UI
   - Compare with multimeter readings

4. **Restore Mode Test**:
   - Set restore mode via UI
   - Power cycle entire system
   - Verify correct behavior on restore

---

## Stress & Edge Case Tests

### 1. Rapid Command Test

**Objective**: Verify firmware handles rapid commands without issues

**Test**: Send 100 commands in quick succession
```python
for i in range(50):
    ser.write(b'PWR_ON\n')
    ser.write(b'PWR_OFF\n')
```

**Pass Criteria**:
- No crashes or hangs
- Final state is correct
- All status updates continue

### 2. Long-Running Stability Test

**Objective**: Verify firmware runs stably for extended periods

**Duration**: 24-72 hours

**Monitoring**:
- Status updates continue at regular intervals
- No memory leaks (stack overflow)
- No timing drift
- Voltage/current readings remain accurate

**Automated Check**:
```python
import time
last_time = time.time()
status_count = 0

while True:
    line = ser.readline()
    if line:
        status_count += 1
        now = time.time()
        interval = now - last_time
        if interval > 1.5:  # More than 500ms late
            print(f"WARNING: Late status at count {status_count}, interval={interval:.3f}s")
        last_time = now
```

### 3. Flash Wear Test

**Objective**: Verify flash handling under heavy write load

**Test**: Perform 1000 power state changes
```python
for i in range(1000):
    ser.write(b'PWR_ON\n')
    time.sleep(0.1)
    ser.write(b'PWR_OFF\n')
    time.sleep(0.1)
    if i % 100 == 0:
        print(f"Completed {i} cycles")
```

**Pass Criteria**:
- All writes succeed
- Sector wrap works correctly
- Final state is correct after power cycle

### 4. Buffer Overflow Test

**Objective**: Verify UART buffer handles overflow gracefully

**Test**: Send oversized command
```python
# Send 500 bytes without newline
ser.write(b'X' * 500)
ser.write(b'\n')

# Then send valid command
ser.write(b'VERSION\n')
response = ser.readline()
assert b'EXTVER' in response  # Should recover and respond
```

### 5. I2C Error Recovery Test

**Objective**: Verify firmware handles I2C errors gracefully

**Method**: Temporarily disconnect INA219 SDA line during operation

**Expected Behavior**:
- Error logged (defmt in debug builds)
- Status updates continue (with zero values)
- Firmware does not crash
- Recovery when INA219 reconnected

### 6. Power Glitch Test

**Objective**: Verify firmware handles brown-outs gracefully

**Method**: Rapidly cycle power (100ms on, 100ms off)

**Pass Criteria**:
- Flash not corrupted
- State consistent after stable power restored

---

## Appendix: Test Equipment Specifications

### Recommended Equipment

| Equipment        | Specification               | Purpose                   |
|------------------|-----------------------------|---------------------------|
| Power Supply     | 0-30V, 0-5A, ±0.1% accuracy | Voltage/current reference |
| Multimeter       | 4.5 digit, ±0.05% accuracy  | Measurement verification  |
| Logic Analyzer   | 24MHz+, 8+ channels         | Timing analysis           |
| USB-UART Adapter | 3.3V TTL, 115200 baud       | Serial communication      |
| Oscilloscope     | 100MHz+, 2+ channels        | Signal integrity          |

### Test Fixture Wiring

```
Power Supply (+) ──┬── INA219 VIN+ ── INA219 VOUT+ ── Load (+)
                   │
                   └── Multimeter V+

Power Supply (-) ──┬── INA219 VIN- ── Load (-)
                   │
                   └── Multimeter V-

RP2040 GPIO8  ──── INA219 SDA
RP2040 GPIO9  ──── INA219 SCL
RP2040 GPIO16 ──── USB-UART RX
RP2040 GPIO17 ──── USB-UART TX
RP2040 GPIO4  ──── Power Control Output (to relay/MOSFET)
RP2040 GND    ──── Common Ground
```
