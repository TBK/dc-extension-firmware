#!/bin/sh
# Update the DC extension firmware over its UART - no BOOTSEL button needed.
#
# Runs ON the JetKVM (busybox sh). The vendor app must not be holding the
# serial port while this runs; stop it first (it is restarted by init on
# reboot, or resume it with kill -CONT):
#
#   for p in $(pgrep jetkvm_app); do kill -STOP $p; done
#   ./uart-fw-update.sh firmware.bin <crc32>
#   for p in $(pgrep jetkvm_app); do kill -CONT $p; done
#
# <crc32> is the decimal zlib CRC-32 of the image, computed on the host:
#   python3 -c "import sys,zlib;print(zlib.crc32(open(sys.argv[1],'rb').read()))" firmware.bin
#
# The image is the application-only wire binary produced by package.sh
# (jetkvm-dc-extension.bin, CRC in the .crc32 sidecar), NOT the UF2.
#
# Protocol (see src/fw_update.rs): FW_UPDATE -> OK -> len(LE32) -> RECV ->
# 4 KiB chunks each ACKed -> crc(LE32) -> CRCOK + FLASH -> device reboots
# into the new firmware.

set -eu

IMG=$1
CRC=$2
TTY=${3:-/dev/ttyS3}

LEN=$(wc -c < "$IMG")
[ "$LEN" -gt 0 ] || { echo "empty image" >&2; exit 1; }

stty -F "$TTY" 115200 raw -echo
exec 3<"$TTY" 4>"$TTY"

# Wait for a protocol line, ignoring interleaved telemetry; die on ERR.
# Bounded: a lost reply fails the transfer quickly instead of wedging the
# port (the device aborts cleanly on its side when the stream stops).
wait_for() {
    deadline=$((15))
    while [ "$deadline" -gt 0 ] && read -r -t 5 line <&3; do
        case "$line" in
            *"$1"*) return 0 ;;
            *ERR*) echo "device error: $line" >&2; exit 1 ;;
        esac
        deadline=$((deadline - 1))
    done
    echo "timeout waiting for $1" >&2
    exit 1
}

# Write a u32 as 4 little-endian raw bytes.
le32() {
    v=$1
    # shellcheck disable=SC2059
    printf "$(printf '\\%03o\\%03o\\%03o\\%03o' \
        $((v & 255)) $(((v >> 8) & 255)) $(((v >> 16) & 255)) $(((v >> 24) & 255)))"
}

echo "requesting update (image: $LEN bytes)"
printf 'FW_UPDATE\n' >&4
wait_for OK
le32 "$LEN" >&4
wait_for RECV

off=0
while [ "$off" -lt "$LEN" ]; do
    dd if="$IMG" bs=4096 skip=$((off / 4096)) count=1 2>/dev/null >&4
    wait_for ACK
    off=$((off + 4096))
    printf '\r%d/%d' "$off" "$LEN" >&2
done
printf '\n' >&2

le32 "$CRC" >&4
wait_for CRCOK
wait_for FLASH
echo "update verified and applied - device is rebooting into the new firmware"
