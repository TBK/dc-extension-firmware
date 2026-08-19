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
- Firmware updates over the same UART (no BOOTSEL button needed), installed
  power-loss-safely by an embassy-boot A/B bootloader with automatic revert

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

### Packaging

Always package via `package.sh` (`./package.sh` for release, `./package.sh
debug` for a dev build). It builds the application and the bootloader and
produces:

| Artifact                       | Purpose                                        |
|--------------------------------|------------------------------------------------|
| `jetkvm-dc-combined.uf2`       | Bootloader + application, for the one-time install of the A/B layout |
| `jetkvm-dc-extension.bin`      | Application-only wire image for UART updates   |
| `jetkvm-dc-extension.bin.crc32`| CRC-32 (zlib) of the wire image, consumed by the update script |

### Flash layout

```
0x000000  boot2 + bootloader (24K reserved)   installed once, never updated
0x006000  bootloader swap state (4K)
0x007000  ACTIVE partition (256K)             the running application
0x080000  settings sector (4K)                power state / restore mode
0x100000  DFU partition (260K)                updates staged here
```

The bootloader (embassy-boot) swaps a verified DFU image into ACTIVE page by
page with progress tracked in the state sector, so power loss during an
update is recoverable at any instant. The application confirms itself on its
first healthy telemetry tick; an update that crashes or hangs before
confirming is automatically reverted to the previous image.

All flash erase/program (settings, DFU staging, and the bootloader's swap)
goes through `direct_flash.rs`: direct bootrom calls from a RAM-resident
routine that feeds the hardware watchdog on every operation, so multi-sector
erases can never outlast the watchdog regardless of the flash chip's speed.
The combined install image is tail-padded with one 0xFF sector because
BOOTSEL flashing has proven unreliable in the final pages of an image on
some units, and `.data` - carrying the RAM-resident flash routines - sits at
the image tail. UART updates are immune (CRC-verified read-back).

## Flashing

First install (or recovery): put the board in BOOTSEL mode and flash
`jetkvm-dc-combined.uf2` — preferably with verification, since BOOTSEL
flashing has proven unreliable on some units:

```bash
sudo picotool load -v -x jetkvm-dc-combined.uf2
```

Subsequent updates need no physical access: send `jetkvm-dc-extension.bin`
over the extension's UART with `tools/uart-fw-update.sh` (run on the
JetKVM; see the script header for usage). The wire protocol is documented in
`src/fw_update.rs`.

See also the [DC Extension OTA Updates documentation](https://jetkvm.com/docs/advanced-usage/ota-updates#dc-extension).

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
| `FW_UPDATE\n`               | Start a firmware update (see `src/fw_update.rs` for the wire protocol) |

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
├── direct_flash.rs  # Bootrom-direct flash driver (erase/program, watchdog-fed)
├── flash_store.rs   # Persistent storage for power state
├── fw_update.rs     # UART firmware-update protocol (writes the DFU partition)
├── power_control.rs # GPIO power output control
├── uart_handler.rs  # UART command processing
└── panic_handler.rs # Release-build panic handler (UART report + reset)
bootloader/          # embassy-boot bootloader (separate crate, installed once)
tools/
├── bin2uf2.py        # RP2040 bin -> UF2 converter, used by package.sh
└── uart-fw-update.sh # Update sender, runs on the JetKVM against /dev/ttyS3
```

## Dependencies

- `embassy-executor` - Async executor for embedded
- `embassy-rp` - RP2040 HAL with Embassy support
- `embassy-time` - Async timers
- `embassy-boot-rp` - A/B bootloader and firmware updater
- `ina219` - INA219 power monitor driver
- `defmt` / `defmt-rtt` - Efficient logging for embedded (dev builds only)
- `panic-probe` - Panic handler with debug output (dev builds only)

## License

MIT
