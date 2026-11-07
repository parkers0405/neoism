#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('windows_bundle', Path(__file__).with_name('windows-bundle.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class WindowsBundleTests(unittest.TestCase):
    def test_gui_subsystem_and_exact_service_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle, services = root / 'bundle', root / 'services'
            bundle.mkdir()
            services.mkdir()
            image = bytearray(256)
            image[:2] = b'MZ'
            struct.pack_into('<I', image, 0x3c, 128)
            image[128:132] = b'PE\0\0'
            struct.pack_into('<H', image, 152, 0x20b)
            struct.pack_into('<H', image, 220, 2)
            (bundle / 'neoism.exe').write_bytes(image)
            names = ('neoism-workspace-daemon.exe', 'neoism-agent.exe', 'neoism-agent-lua-runner.exe')
            for name in names:
                (bundle / name).write_bytes(b'fresh service')
                (services / name).write_bytes(b'fresh service')
            self.assertEqual(len(module.verify_windows_bundle(bundle, services)), 3)
            for name in names:
                (bundle / name).write_bytes(b'stale installed service')
                with self.assertRaisesRegex(ValueError, 'stale packaged service'):
                    module.verify_windows_bundle(bundle, services)
                (bundle / name).write_bytes(b'fresh service')
            struct.pack_into('<H', image, 220, 3)
            (bundle / 'neoism.exe').write_bytes(image)
            with self.assertRaisesRegex(ValueError, 'subsystem 2'):
                module.verify_windows_bundle(bundle, services)
            (bundle / 'neoism.exe').write_bytes(b'not a PE')
            with self.assertRaises(ValueError):
                module.verify_windows_bundle(bundle, services)


if __name__ == '__main__':
    unittest.main()