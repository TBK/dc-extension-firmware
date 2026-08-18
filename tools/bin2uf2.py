#!/usr/bin/env python3
# Minimal RP2040 bin->UF2 converter (no deps).
import struct, sys
base = 0x10000000            # RP2040 flash XIP base (boot2 at offset 0)
FAMILY = 0xe48bff56          # RP2040
UF2_MAGIC0, UF2_MAGIC1, UF2_END = 0x0A324655, 0x9E5D5157, 0x0AB16F30
FLAG_FAMILY = 0x00002000
src, dst = sys.argv[1], sys.argv[2]
data = open(src, 'rb').read()
if data[:2] == b'':  # noop
    pass
chunk = 256
blocks = (len(data) + chunk - 1) // chunk
out = bytearray()
for i in range(blocks):
    payload = data[i*chunk:(i+1)*chunk]
    addr = base + i*chunk
    block = struct.pack('<IIIIIIII',
        UF2_MAGIC0, UF2_MAGIC1, FLAG_FAMILY, addr,
        len(payload), i, blocks, FAMILY)
    block += payload + b'\x00' * (476 - len(payload))
    block += struct.pack('<I', UF2_END)
    assert len(block) == 512
    out += block
open(dst, 'wb').write(out)
print(f"{src} ({len(data)} B) -> {dst} ({blocks} blocks, {len(out)} B), base=0x{base:08x}")
