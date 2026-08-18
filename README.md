<div align="center">
    <img alt="JetKVM logo" src="https://jetkvm.com/logo-blue.png" height="28">

### DC Power Extension Firmware

[Discord](https://jetkvm.com/discord) | [Website](https://jetkvm.com) | [Issues](https://github.com/jetkvm/cloud-api/issues) | [Docs](https://jetkvm.com/docs)

[![Twitter](https://img.shields.io/twitter/url/https/twitter.com/jetkvm.svg?style=social&label=Follow%20%40JetKVM)](https://twitter.com/jetkvm)

</div>

This is a power monitoring and control module for the JetKVM platform, built on the Raspberry Pi RP2040, the same chip as the Raspberry Pi Pico.

## Features

- Voltage, current, and power measurements via INA219 sensor
- Power state control through UART interface
- Persistent power state and restore mode storage in flash

If you've found an issue and want to report it, please check our [Issues](https://github.com/jetkvm/dc-extension-firmware/issues) page. Make sure the description contains information about the firmware version you're using, your hardware setup, and a clear explanation of the steps to reproduce the issue.

# Development

The firmware is written in Async I/O Rust using the [Embassy](https://embassy.dev/) framework. Knowledge of Rust programming and embedded systems is recommended.

## Requirements

- Rust stable (v1.95+, edition 2024)
- `thumbv6m-none-eabi` target
- `cargo-binutils` (provides `rust-objcopy`) and Python 3 for UF2 generation
- `probe-rs` for flashing via debug probe (optional)

## Setup

Install the required target:

```bash
rustup target add thumbv6m-none-eabi
```

Install tools for firmware generation (the `llvm-tools` component is pulled in
via `rust-toolchain.toml`):

```bash
cargo install cargo-binutils
```

## Building

### Release Build (production, no debug logging)

```bash
cargo build --release
```

This produces an optimized binary without debug logging, using the custom UART + reset panic handler.

### Development Build (with debug logging)

```bash
cargo build
```

This produces a debug build with defmt RTT logging enabled, useful for development with a debug probe.

### Generate UF2 File

Always package via `package.sh` — it pads the image with one 4 KB sector of
0xFF after `.data`, because BOOTSEL flashing has proven unreliable in the final
pages of the image on some units and `.data` carries the RAM-resident
flash-write routine:

For release:
```bash
./package.sh
```

For development:
```bash
./package.sh debug
```

Both produce `jetkvm-dc-extension.uf2`.

## Flashing

See the [DC Extension OTA Updates documentation](https://jetkvm.com/docs/advanced-usage/ota-updates#dc-extension) for flashing instructions.

### Using probe-rs (development)

```bash
cargo run --release
```

## Debug Logging

Debug logging is automatically controlled by the build profile:

| Build Command           | Profile | Debug Logging       | Panic Handler         |
|-------------------------|---------|---------------------|-----------------------|
| `cargo build`           | dev     | Enabled (defmt-rtt) | panic-probe           |
| `cargo build --release` | release | Disabled            | Custom (UART + reset) |

To view debug output during development, use a debug probe with RTT support:

```bash
# Run with probe-rs (automatically connects RTT)
cargo run

# Or attach to running firmware
probe-rs attach --chip RP2040 target/thumbv6m-none-eabi/debug/jetkvm-dc-extension
```

### Panic Behavior

In release builds, if a panic occurs the firmware will:

1. Send a `PANIC;` message via UART with location info (e.g., `PANIC;DC extension firmware panic occurred at src/main.rs:123`)
2. Automatically reset the device

This allows the main JetKVM system to detect firmware crashes and the device to recover automatically.

## Hardware Configuration

| Function      | Pin    | Peripheral |
|---------------|--------|------------|
| UART TX       | GPIO16 | UART0      |
| UART RX       | GPIO17 | UART0      |
| I2C SDA       | GPIO8  | I2C0       |
| I2C SCL       | GPIO9  | I2C0       |
| Power Control | GPIO4  | GPIO       |

### INA219 Configuration

- I2C Address: 0x40
- Shunt Resistance: 0.01 ohms (10 mΩ)
- Bus Voltage Range: 32V
- PGA: ±320mV (chip power-on defaults; the firmware writes no configuration)

## UART Protocol

### Commands (115200 baud, 8N1)

| Command                     | Description                    |
|-----------------------------|--------------------------------|
| `PWR_ON\n`                  | Turn power output on           |
| `PWR_OFF\n`                 | Turn power output off          |
| `RESTORE_MODE_OFF\n`        | Set restore mode to OFF        |
| `RESTORE_MODE_ON\n`         | Set restore mode to ON         |
| `RESTORE_MODE_LAST_STATE\n` | Set restore mode to last state |
| `VERSION\n`                 | Request firmware version       |

### Responses

Status update (every 1 second):
```
<power_state>;<voltage_mV>;<current_mA>;<power_mW>;<restore_mode>\n
```

Version response:
```
EXTVER;<name>;<version>\n
```

## Project Structure

```
src/
├── main.rs          # Entry point, task orchestration
├── config.rs        # Configuration constants
├── flash_store.rs   # Persistent storage for power state
├── power_control.rs # GPIO power output control
├── uart_handler.rs  # UART command processing
├── ram_flash.rs     # RAM-resident bootrom flash erase/program routines
└── panic_handler.rs # Release-build panic handler (UART report + reset)
```

## Dependencies

- `embassy-executor` - Async executor for embedded
- `embassy-rp` - RP2040 HAL with Embassy support
- `embassy-time` - Async timers
- `ina219` - INA219 power monitor driver
- `defmt` / `defmt-rtt` - Efficient logging for embedded (dev builds only)
- `panic-probe` - Panic handler with debug output (dev builds only)

## License

MIT
