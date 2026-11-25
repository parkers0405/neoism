"""Local browser viewer for the actual Rune mobile scenes (no daemon required)."""

import os
from pathlib import Path
import re
import struct
import zlib
from http.server import BaseHTTPRequestHandler, HTTPServer
import subprocess
from tempfile import TemporaryDirectory
from urllib.parse import parse_qs, urlsplit


APP = Path(__file__).resolve().parents[1]
CACHE = Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache")
TARGET = Path(os.environ.get("CARGO_TARGET_DIR") or CACHE / "neoism-rune-target")
BIN = APP / TARGET / "debug/neoism-rune"
ENV = dict(os.environ, NEOISM_MOBILE="1", NEOISM_RUNE_OFFLINE="1")
SCENES = {"pair", "welcome", "chat", "history", "models", "systems", "crafting", "settings", "themes", "composer-menu"}


def splash_animation(frames):
    """Package Rune's PNG frames as a one-shot APNG without an external encoder."""
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    output = bytearray(b"\x89PNG\r\n\x1a\n")
    sequence = 0
    for index, frame in enumerate(frames):
        png = frame.read_bytes()
        width, height = struct.unpack_from(">II", png, 16)
        if index == 0:
            output.extend(png[8:33])  # IHDR
            output.extend(chunk(b"acTL", struct.pack(">II", len(frames), 1)))
        output.extend(chunk(b"fcTL", struct.pack(">IIIIIHHBB", sequence, width, height, 0, 0, 1, 24, 0, 0)))
        sequence += 1
        offset = 33
        while offset < len(png):
            length, kind = struct.unpack_from(">I4s", png, offset)
            if kind == b"IDAT":
                data = png[offset + 8:offset + 8 + length]
                if index:
                    output.extend(chunk(b"fdAT", struct.pack(">I", sequence) + data))
                    sequence += 1
                else:
                    output.extend(chunk(kind, data))
            offset += length + 12
    output.extend(chunk(b"IEND", b""))
    return bytes(output)


class Preview(BaseHTTPRequestHandler):
    def reply(self, content, mime, status=200):
        self.send_response(status)
        self.send_header("Content-Type", mime)
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(content)))
        self.end_headers()
        self.wfile.write(content)

    def do_GET(self):
        url = urlsplit(self.path)
        if url.path == "/":
            self.reply((APP / "mobile/preview.html").read_bytes(), "text/html; charset=utf-8")
            return
        animate = url.path == "/splash.png"
        if url.path != "/render.png" and not animate:
            self.send_error(404)
            return
        query = parse_qs(url.query)
        scene = query.get("scene", ["chat"])[0]
        size = query.get("size", ["390x844"])[0]
        if scene not in SCENES or not re.fullmatch(r"[3-7][0-9]{2}x[5-9][0-9]{2}", size):
            self.send_error(400)
            return
        try:
            with TemporaryDirectory(prefix="neoism-preview-") as temp:
                output = Path(temp) / "preview.png"
                args = ["--out", temp, "--frames", "60", "--fps", "24"] if animate else ["--snap", output, "--at", "1"]
                subprocess.run([BIN, "--scene", "splash-replay" if animate else scene, "--size", size, "--scale", "1" if animate else "2", *args],
                               check=True, env=ENV, cwd=APP, timeout=30)
                frames = sorted(Path(temp).glob("frame_*.png")) if animate else []
                self.reply(splash_animation(frames) if frames else output.read_bytes(), "image/apng" if animate else "image/png")
        except (OSError, subprocess.SubprocessError) as error:
            print(error, flush=True)
            self.send_error(500, "Could not render. Build the local Rune binary first.")


if __name__ == "__main__":
    port = int(os.environ.get("NEOISM_PREVIEW_PORT", "4173"))
    with HTTPServer(("127.0.0.1", port), Preview) as server:
        print(f"Neoism Mobile preview: http://127.0.0.1:{port}", flush=True)
        server.serve_forever()