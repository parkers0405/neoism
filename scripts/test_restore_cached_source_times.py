import importlib.util
import os
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('cache_times', Path(__file__).with_name('restore-cached-source-times.py'))
cache_times = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cache_times)


class CacheSourceTimes(unittest.TestCase):
    def test_only_identical_sources_in_every_cache_get_their_original_time(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            files = {}
            for name in ['same.rs', 'changed.rs', 'native.rs']:
                path = root / name
                path.write_text('original\n')
                files[name] = {'sha256': cache_times.digest(path), 'mtime_ns': 1_000_000_000}
                os.utime(path, ns=(3_000_000_000, 3_000_000_000))
            native = {name: dict(entry) for name, entry in files.items()}
            native['native.rs']['sha256'] = 'different-cache-source'
            (root / 'changed.rs').write_text('edited\n')
            before = {p.name: p.stat().st_mtime_ns for p in root.glob('*.rs')}
            self.assertEqual(cache_times.restore(root, [files, native]), 1)
            self.assertEqual((root / 'same.rs').stat().st_mtime_ns, 1_000_000_000)
            for name in ['changed.rs', 'native.rs']:
                self.assertEqual((root / name).stat().st_mtime_ns, before[name])

    def test_failed_build_keeps_uncompiled_source_newer_than_cached_objects(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'source.rs'
            path.write_text('new source not yet compiled\n')
            old_object_time = 1_000_000_000
            fresh_source_time = 2_000_000_000
            files = {'source.rs': {'sha256': cache_times.digest(path), 'mtime_ns': fresh_source_time}}
            # A new checkout is later again. Restore the actual failed-build
            # timestamp, never an arbitrary ancient time based on commit identity.
            os.utime(path, ns=(3_000_000_000, 3_000_000_000))
            self.assertEqual(cache_times.restore(root, [files]), 1)
            self.assertEqual(path.stat().st_mtime_ns, fresh_source_time)
            self.assertGreater(path.stat().st_mtime_ns, old_object_time)


if __name__ == '__main__':
    unittest.main()
