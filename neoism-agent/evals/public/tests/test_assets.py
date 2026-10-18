"""Offline asset tests: fixtures only, no native execution or Docker required."""
from __future__ import annotations

import hashlib
import importlib.util
import io
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("public_assets", Path(__file__).resolve().parents[1] / "assets.py")
assets = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(assets)


class AssetTests(unittest.TestCase):
    def test_version_selection(self):
        for text, version in [("neoism-agent 0.7.128", "0.7.128"),
                              ("2.1.292 (Claude Code)", "2.1.292"), ("2.0.32\n", "2.0.32")]:
            self.assertEqual(assets.version_number(text), version)
        with self.assertRaises(ValueError):
            assets.version_number("no version")

    def test_freeze_copies_sha_named_elf_and_executes_only_copy(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "mutable"
            source.write_bytes(b"\x7fELFfixture")
            digest = hashlib.sha256(source.read_bytes()).hexdigest()
            with patch.object(assets, "run", return_value=b"2.0.32\n") as run:
                result = assets.freeze_binary(source, "2.0.32", digest, root / "cache")
            frozen = Path(result["binary_path"])
            self.assertEqual(frozen.name, digest + ".elf")
            self.assertNotEqual(run.call_args.args[0][0], source)
            self.assertEqual(run.call_args.args[0][1:], ["--version"])
            self.assertEqual(frozen.stat().st_mode & 0o777, 0o555)
            source.write_bytes(b"changed")
            self.assertEqual(assets.sha256(frozen), digest)

    def test_hash_version_and_elf_mismatches_fail(self):
        with tempfile.TemporaryDirectory() as temp:
            source = Path(temp) / "binary"
            source.write_bytes(b"\x7fELFfixture")
            with patch.object(assets, "run", return_value=b"1.0.0") as run:
                with self.assertRaises(ValueError):
                    assets.freeze_binary(source, "2.0.32", "0" * 64, Path(temp) / "cache")
                run.assert_not_called()
                with self.assertRaises(ValueError):
                    assets.freeze_binary(source, "2.0.32", None, Path(temp) / "cache")
                source.write_bytes(b"#!/bin/sh")
                with self.assertRaises(ValueError):
                    assets.freeze_binary(source, "1.0.0", None, Path(temp) / "cache")

    def make_tar(self, path, extra=None):
        with tarfile.open(str(path), "w") as tar:
            for name in ("bin/neoism-agent", "lib/libc.so.6", "lib/ld-linux-x86-64.so.2"):
                member = tarfile.TarInfo(name)
                member.size, member.mode = 1, 0o555
                tar.addfile(member, io.BytesIO(b"x"))
            if extra:
                tar.addfile(extra, io.BytesIO(b"x") if extra.size else None)

    def test_valid_regular_file_only_archive(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "bundle.tar"
            self.make_tar(path)
            assets.validate_archive(path)

    def test_archive_rejects_traversal_links_special_files_and_real_elf(self):
        cases = [("/lib/evil.so", tarfile.REGTYPE), ("lib/../evil.so", tarfile.REGTYPE),
                 ("lib/link.so", tarfile.SYMTYPE), ("lib/hard.so", tarfile.LNKTYPE),
                 ("lib/fifo.so", tarfile.FIFOTYPE), ("bin/neoism-agent.real", tarfile.REGTYPE),
                 ("home/secret", tarfile.REGTYPE), ("lib/libc.so.6", tarfile.REGTYPE),
                 ("lib/./evil.so", tarfile.REGTYPE)]
        with tempfile.TemporaryDirectory() as temp:
            for name, kind in cases:
                with self.subTest(name=name, kind=kind):
                    member = tarfile.TarInfo(name)
                    member.type, member.linkname = kind, "../../outside"
                    path = Path(temp) / "bundle.tar"
                    self.make_tar(path, member)
                    with self.assertRaises(ValueError):
                        assets.validate_archive(path)

    def test_wrapper_scopes_loader_not_child_environment(self):
        self.assertIn(b"--library-path /opt/neoism-public-eval/lib", assets.WRAPPER)
        self.assertIn(b'neoism-agent.real "$@"', assets.WRAPPER)
        self.assertNotIn(b"LD_LIBRARY_PATH", assets.WRAPPER)

    def test_existing_lock_does_not_consult_mutable_sources(self):
        with tempfile.TemporaryDirectory() as temp:
            lock = Path(temp) / "lock.json"
            lock.write_text('{"fixture": true}')
            with patch.object(assets, "LOCK", lock), patch.object(assets, "verify_lock", return_value={"ok": True}), patch.object(assets, "freeze_binary") as freeze:
                self.assertEqual(assets.freeze_assets(), {"ok": True})
                freeze.assert_not_called()


if __name__ == "__main__":
    unittest.main()
