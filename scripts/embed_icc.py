#!/usr/bin/env python3
"""Regenerate Sources/negcli/ICCProfiles.swift from NegPy's bundled profiles.

The embedded blobs are byte-for-byte NegPy's `icc/sRGB-v4.icc` and
`icc/AdobeCompat-v4.icc` (CC0) — the same profiles the ColorIO
display-transform oracle was regenerated from, so keep them in lock step:
re-run this only if upstream's profiles change, and re-check the oracle then.

Usage: python3 scripts/embed_icc.py [path-to-NegPy-checkout]
"""

import pathlib
import struct
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
NEGPY = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else pathlib.Path.home() / "Documents/code/NegPy"
OUT = REPO / "Sources/negcli/ICCProfiles.swift"

HEADER = '''import Foundation

#if !canImport(ImageIO)
/// Compact ICC v4 profiles for tagging Linux exports, embedded as bytes so
/// neither target (negcli, CoreBridge — this file is SYMLINKED into the
/// latter, like TIFF16.swift) needs SwiftPM resource-bundle plumbing.
///
/// Byte-for-byte NegPy's bundled `icc/sRGB-v4.icc` / `icc/AdobeCompat-v4.icc`
/// (CC0 per their embedded copyright tags) — the same profiles the ColorIO
/// display-transform oracle was regenerated from (2026-07-20), so a
/// colour-managed viewer applying these reproduces the transform the parity
/// suite pins. Tagging is deliberately separable from CONVERTING: the lcms2
/// export path is still to come, but a 16-bit file must say what space its
/// bytes are in either way (upstream dd6430f: an untagged working-space file
/// reads as sRGB everywhere, "wrong with nothing on screen to explain it").
///
/// Regenerate from the NegPy checkout if upstream's profiles ever change:
///   python3 scripts/embed_icc.py  (writes this file)
enum ICCProfiles {
'''


def sanity(data: bytes, path: pathlib.Path) -> None:
    """Refuse to embed a blob that isn't a well-formed RGB monitor profile."""
    size = struct.unpack(">I", data[0:4])[0]
    assert size == len(data), f"{path}: header size {size} != file size {len(data)}"
    assert data[36:40] == b"acsp", f"{path}: bad magic"
    assert data[16:20] == b"RGB ", f"{path}: not an RGB profile"
    tags = set()
    (n,) = struct.unpack(">I", data[128:132])
    for i in range(n):
        sig, off, sz = struct.unpack(">4sII", data[132 + i * 12 : 144 + i * 12])
        assert off + sz <= len(data), f"{path}: tag {sig} overruns"
        tags.add(sig)
    required = {b"desc", b"wtpt", b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC"}
    assert required <= tags, f"{path}: missing {required - tags}"


def swift_array(path: pathlib.Path, name: str, comment: str) -> str:
    data = path.read_bytes()
    sanity(data, path)
    lines = [
        "        " + ", ".join(f"0x{b:02x}" for b in data[i : i + 16]) + ","
        for i in range(0, len(data), 16)
    ]
    body = "\n".join(lines)
    return f"    /// {comment} ({len(data)} bytes).\n    static let {name}: [UInt8] = [\n{body}\n    ]\n"


def main() -> None:
    out = HEADER
    out += swift_array(NEGPY / "icc/sRGB-v4.icc", "sRGB", "sRGB IEC61966-2.1, v4 parametric")
    out += "\n"
    out += swift_array(
        NEGPY / "icc/AdobeCompat-v4.icc",
        "adobeRGB1998",
        'Adobe RGB (1998) compatible ("A98C"), v4 parametric',
    )
    out += "}\n#endif\n"
    OUT.write_text(out)
    print(f"wrote {OUT} ({len(out)} chars)")


if __name__ == "__main__":
    main()
