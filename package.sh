#!/bin/sh
# Build and package the firmware.
#
# Usage: ./package.sh [debug|release]   (default: release)
# The profile applies to the application; the bootloader is always built
# release (it must stay small and has no debug logging).
#
# Produces:
#
#   jetkvm-dc-combined.uf2      - bootloader + application in one image, for
#                                 the one-time install of the embassy-boot
#                                 layout (BOOTSEL: `sudo picotool load -v -x`,
#                                 always with -v; or send dcfw-combined.bin
#                                 through the pre-bootloader 0.2.x FW_UPDATE,
#                                 which writes it verbatim at flash offset 0).
#
#   jetkvm-dc-extension.bin     - application-only wire image for regular
#                                 UART updates (tools/uart-fw-update.sh).
#                                 Written to the DFU partition and swapped in
#                                 by the bootloader, power-loss-safe, with
#                                 automatic revert if the new image fails to
#                                 confirm. CRC-32 (zlib) in the .crc32 sidecar.
#
# Flash layout: see memory.x. The gap between the bootloader and the
# application at 0x7000 is padded with 0xFF; that includes the
# BOOTLOADER_STATE sector at 0x6000, whose erased state means "boot normally".
set -eu
cd "$(dirname "$0")"

PROFILE=${1:-release}
APP_OFFSET=28672 # 0x7000

case "$PROFILE" in
    release)
        cargo build --release
        APP_ELF=target/thumbv6m-none-eabi/release/jetkvm-dc-extension
        ;;
    debug)
        cargo build
        APP_ELF=target/thumbv6m-none-eabi/debug/jetkvm-dc-extension
        ;;
    *)
        echo "usage: $0 [debug|release]" >&2
        exit 1
        ;;
esac

(cd bootloader && cargo build --release)

rust-objcopy -O binary bootloader/target/thumbv6m-none-eabi/release/jetkvm-dc-bootloader bootloader.bin
rust-objcopy -O binary "$APP_ELF" jetkvm-dc-extension.bin

python3 - "$APP_OFFSET" <<'EOF'
import sys, zlib
app_offset = int(sys.argv[1])
bl = open('bootloader.bin', 'rb').read()
app = open('jetkvm-dc-extension.bin', 'rb').read()
assert len(bl) <= app_offset, f"bootloader too big: {len(bl)} > {app_offset}"
assert len(app) <= 256 * 1024, f"application exceeds ACTIVE partition: {len(app)}"
# Tail-pad: BOOTSEL flashing has proven unreliable in the final pages of an
# image on some units (observed on both extension boards), and .data -
# carrying the RAM-resident flash routines - sits at the image tail. One
# padding sector keeps it out of the risky last pages.
combined = bl + b'\xff' * (app_offset - len(bl)) + app + b'\xff' * 4096
open('dcfw-combined.bin', 'wb').write(combined)
crc = zlib.crc32(app)
open('jetkvm-dc-extension.bin.crc32', 'w').write(f"{crc}\n")
print(f"bootloader {len(bl)} B + app {len(app)} B -> combined {len(combined)} B")
print(f"app wire image CRC32 (for uart-fw-update.sh): {crc}")
print(f"combined wire image CRC32 (for 0.2.x migration): {zlib.crc32(combined)}")
EOF

python3 tools/bin2uf2.py dcfw-combined.bin jetkvm-dc-combined.uf2
