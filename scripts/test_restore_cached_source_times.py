import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('cache_times', Path(__file__).with_name('restore-cached-source-times.py'))
cache_times = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cache_times)


class CacheSourceTimes(unittest.TestCase):
    def test_changed_dirty_and_different_native_cache_sources_stay_fresh(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(['git', '-C', directory, *args]).decode().strip()
            git('init', '-q')
            git('config', 'user.email', 'fixture@example.test')
            git('config', 'user.name', 'Fixture')
            git('config', 'core.autocrlf', 'false')
            for name in ['same.rs', 'changed.rs', 'dirty.rs', 'native.rs']:
                (root / name).write_text('original\n')
            git('add', '.')
            git('commit', '-qm', 'original cache')
            native = git('rev-parse', 'HEAD')
            (root / 'native.rs').write_text('new native source\n')
            git('commit', '-qam', 'service cache with newer source')
            service = git('rev-parse', 'HEAD')
            (root / 'changed.rs').write_text('new release source\n')
            git('commit', '-qam', 'release')
            (root / 'dirty.rs').write_text('uncommitted edit\n')
            (root / 'untracked.rs').write_text('generated\n')
            before = {p.name: p.stat().st_mtime_ns for p in root.glob('*.rs')}
            self.assertEqual(cache_times.restore(root, [service, native]), 1)
            self.assertLess((root / 'same.rs').stat().st_mtime_ns, before['same.rs'])
            for name in ['changed.rs', 'dirty.rs', 'native.rs', 'untracked.rs']:
                self.assertEqual((root / name).stat().st_mtime_ns, before[name], name)
            self.assertEqual((root / 'dirty.rs').read_text(), 'uncommitted edit\n')


if __name__ == '__main__':
    unittest.main()
