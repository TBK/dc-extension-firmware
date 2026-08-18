#!/bin/sh
# Build and package the firmware UF2.
#
# Usage: package.sh [release|debug]   (default: release)
#
# The binary is padded with one 4 KB sector of 0xFF after .data: BOOTSEL
# flashing has proven unreliable in the final pages of the image on some
# units, and .data carries the RAM-resident flash-write routine, so it must
# never sit in the last flashed page.
#
# Flash with:  sudo picotool load -v -x jetkvm-dc-extension.uf2
# Always pass -v: it verifies the written image and catches the unreliable
# writes described above. Re-run the load if verify fails.
set -eu
cd "$(dirname "$0")"

PROFILE="${1:-release}"
case "$PROFILE" in
release) cargo build --release ;;
debug) cargo build ;;
*)
    echo "usage: $0 [release|debug]" >&2
    exit 1
    ;;
esac

rust-objcopy -O binary "target/thumbv6m-none-eabi/$PROFILE/jetkvm-dc-extension" dcfw.bin
python3 -c "
data = open('dcfw.bin', 'rb').read()
open('dcfw-padded.bin', 'wb').write(data + b'\xff' * 4096)
print(f'padded {len(data)} -> {len(data) + 4096} bytes')
"
python3 tools/bin2uf2.py dcfw-padded.bin jetkvm-dc-extension.uf2
