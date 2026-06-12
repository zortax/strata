#!/usr/bin/env python3
"""Crop an ICON-D2 regular-lat-lon GRIB2 fixture to a lat/lon window.

Full ICON-D2 files are ~1 MB compressed per message (16-bit simple packing
of high-entropy data), too big for committed fixtures. This script shrinks a
single-message file by masking the Section-6 bitmap to a window: grid
geometry (Section 3), packing parameters (Section 5) and the packed values
of kept points (Section 7) stay byte-identical to the real DWD file; points
outside the window become bitmap-masked no-data, exactly like the points
outside the native icosahedral domain already are.

Requires: single GRIB2 message, DRT 5.0 (simple packing), 16 bits per value,
bitmap indicator 0 — which is what DWD publishes for the regular-lat-lon
ICON-D2 single-level and pressure-level files.

Usage:
    crop_fixture.py in.grib2.bz2 out.grib2.bz2 SOUTH NORTH WEST EAST
"""

import bz2
import struct
import sys

# ICON-D2 regular lat-lon grid (verified live 2026-06-10/11).
NI, NJ = 1215, 746
LAT0, LON0, STEP = 43.18, -3.94, 0.02


def signed_magnitude_16(b: bytes) -> int:
    v = struct.unpack(">H", b)[0]
    return -(v & 0x7FFF) if v & 0x8000 else v


def crop(data: bytes, south: float, north: float, west: float, east: float) -> bytes:
    assert data[:4] == b"GRIB", "not a GRIB2 file"
    total = struct.unpack(">Q", data[8:16])[0]
    assert total == len(data), "expected exactly one GRIB2 message"

    sections = []
    p = 16
    while p < total - 4:
        length = struct.unpack(">I", data[p : p + 4])[0]
        sections.append((data[p + 4], bytearray(data[p : p + length])))
        p += length
    assert data[p : p + 4] == b"7777", "missing end section"

    by_num = dict(sections)
    sec5, sec6, sec7 = by_num[5], by_num[6], by_num[7]
    n_defined = struct.unpack(">I", bytes(sec5[5:9]))[0]
    drt = struct.unpack(">H", bytes(sec5[9:11]))[0]
    bits = sec5[19]
    assert drt == 0 and bits == 16, f"need DRT 5.0 / 16 bits, got 5.{drt} / {bits}"
    assert sec6[5] == 0, "need an embedded bitmap (indicator 0)"
    bitmap = bytes(sec6[6:])
    packed = bytes(sec7[5:])
    assert len(packed) == 2 * n_defined

    i_lo = max(0, round((west - LON0) / STEP))
    i_hi = min(NI - 1, round((east - LON0) / STEP))
    j_lo = max(0, round((south - LAT0) / STEP))
    j_hi = min(NJ - 1, round((north - LAT0) / STEP))

    # Walk the scan order (mode 0x40: W->E, S->N, i consecutive); `c` counts
    # bitmap-defined points, i.e. the index into the packed 16-bit values.
    new_bitmap = bytearray(len(bitmap))
    kept = bytearray()
    c = 0
    for k in range(NI * NJ):
        if bitmap[k >> 3] & (0x80 >> (k & 7)):
            j, i = divmod(k, NI)
            if i_lo <= i <= i_hi and j_lo <= j <= j_hi:
                new_bitmap[k >> 3] |= 0x80 >> (k & 7)
                kept += packed[2 * c : 2 * c + 2]
            c += 1
    assert c == n_defined
    n_kept = len(kept) // 2

    sec5[5:9] = struct.pack(">I", n_kept)
    new_sec6 = struct.pack(">IBB", 6 + len(new_bitmap), 6, 0) + new_bitmap
    new_sec7 = struct.pack(">IB", 5 + len(kept), 7) + kept

    out = bytearray(data[:16])
    for num, sec in sections:
        out += {5: sec5, 6: new_sec6, 7: new_sec7}.get(num, sec)
    out += b"7777"
    out[8:16] = struct.pack(">Q", len(out))

    # Sanity stats: unpack kept values (Y = (R + X * 2^E) / 10^D).
    r = struct.unpack(">f", bytes(sec5[11:15]))[0]
    e = signed_magnitude_16(bytes(sec5[15:17]))
    d = signed_magnitude_16(bytes(sec5[17:19]))
    ys = [
        (r + struct.unpack(">H", kept[2 * m : 2 * m + 2])[0] * 2.0**e) / 10.0**d
        for m in range(n_kept)
    ]
    print(f"kept {n_kept}/{n_defined} values, min {min(ys):.2f}, max {max(ys):.2f}")
    return bytes(out)


def main() -> None:
    src, dst = sys.argv[1], sys.argv[2]
    south, north, west, east = map(float, sys.argv[3:7])
    cropped = crop(bz2.decompress(open(src, "rb").read()), south, north, west, east)
    compressed = bz2.compress(cropped, 9)
    with open(dst, "wb") as f:
        f.write(compressed)
    print(f"{dst}: {len(cropped)} bytes raw, {len(compressed)} bytes bz2")


if __name__ == "__main__":
    main()
