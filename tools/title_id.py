#!/usr/bin/env python3
"""Prints a .3ds/.cci image's title id, and fails for one still encrypted.

usage: title_id.py game.3ds
"""
import struct
import sys

with open(sys.argv[1], "rb") as rom:
    header = rom.read(0x200)
    if header[0x100:0x104] != b"NCSD":
        sys.exit(f"{sys.argv[1]} is not a .3ds/.cci image")
    title_id = struct.unpack_from("<Q", header, 0x108)[0]
    first = struct.unpack_from("<I", header, 0x120)[0] * 0x200
    rom.seek(first)
    ncch = rom.read(0x200)
    if ncch[0x100:0x104] != b"NCCH":
        sys.exit("the image's first partition is not an NCCH")
    # flags[7] bit 2, no crypto
    if not ncch[0x18F] & 0x4:
        sys.exit("the image is encrypted: decrypt it first (GodMode9: Decrypt file), or build it with your aes_keys.txt")
print(f"{title_id:016X}")
