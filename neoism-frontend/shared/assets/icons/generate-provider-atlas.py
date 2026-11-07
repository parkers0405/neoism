#!/usr/bin/env python3
"""Rebuild the xAI cell, preserving the seven legacy masks. Requires rsvg-convert and ImageMagick."""
import base64
from pathlib import Path
import subprocess
import struct
import sys
import tempfile
import textwrap


def generate():
    assets = Path(__file__).resolve().parent
    atlas = assets / "provider-logos.png.b64"
    with tempfile.TemporaryDirectory(prefix="neoism-provider-atlas-") as directory:
        work = Path(directory)
        legacy = work / "legacy.png"
        original = base64.b64decode(atlas.read_text())
        if original[:8] != b"\x89PNG\r\n\x1a\n" or struct.unpack(">II", original[16:24]) not in [(448, 64), (512, 64)]:
            raise SystemExit("Expected a seven- or eight-cell provider PNG atlas")
        legacy.write_bytes(original)
        subprocess.run([
            "convert", str(legacy), "-crop", "448x64+0+0", "+repage", str(legacy),
        ], check=True)
        xai = work / "xai.png"
        subprocess.run([
            "rsvg-convert", "-w", "64", "-h", "64", str(assets / "xai.svg"), "-o", str(xai),
        ], check=True)
        png = subprocess.check_output([
            "convert", str(legacy), str(xai), "+append", "-strip", "-depth", "8", "PNG32:-",
        ])
        legacy_pixels = subprocess.check_output(["convert", str(legacy), "-depth", "8", "rgba:-"])
        generated_pixels = subprocess.check_output([
            "convert", "png:-", "-crop", "448x64+0+0", "+repage", "-depth", "8", "rgba:-",
        ], input=png)
        if legacy_pixels != generated_pixels:
            raise SystemExit("Generation changed legacy provider pixels")
        alpha = subprocess.check_output(["convert", str(xai), "-alpha", "extract", "-depth", "8", "gray:-"])
        fingerprint = 0xCBF29CE484222325
        for byte in alpha:
            fingerprint = ((fingerprint ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
        print(f"xAI alpha FNV-1a: {fingerprint:#018x}")
    encoded = "\n".join(textwrap.wrap(base64.b64encode(png).decode(), 100)) + "\n"
    if "--check" in sys.argv:
        if atlas.read_text() != encoded:
            raise SystemExit("Provider atlas drifted; run generate-provider-atlas.py")
    else:
        atlas.write_text(encoded)


if __name__ == "__main__":
    generate()