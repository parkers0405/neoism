#!/usr/bin/env python3
"""Exercise the actual download installer offline, including late rollback."""
import os
import shutil
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parent / 'install.sh'

class InstallerTests(unittest.TestCase):
    def test_install_upgrade_missing_payload_and_rollback(self):
        with tempfile.TemporaryDirectory(prefix='neoism-installer-') as tmp:
            root = Path(tmp)
            mock = root / 'mock'
            mock.mkdir()
            install = root / 'installed'
            payload = root / 'neoism-linux-x86_64'
            gui = payload / 'web/app/assets'
            gui.mkdir(parents=True)
            (payload / 'web/native').mkdir()
            (payload / 'web/app/manifest.json').write_text('{}')
            (gui / 'app.js').write_text('new JS')
            (payload / 'licenses').mkdir()
            for name in ['libneoism_gpuix_core.so', 'libghostty-vt.so', 'LICENSE.ghostty', 'LICENSE', 'NOTICE']:
                (payload / name).write_text(name)
            shutil.copy2(SCRIPT.with_name('install-gpuix-bundle.sh'), payload / 'install-bundle.sh')
            for binary in ['neoism', 'neoism-agent', 'neoism-agent-lua-runner', 'neoism-workspace-daemon']:
                path = payload / binary
                path.write_text('#!/bin/sh\necho new\n')
                path.chmod(0o755)
            archive = root / 'archive.tar.gz'
            def pack():
                with tarfile.open(archive, 'w:gz') as tar:
                    tar.add(payload, arcname=payload.name)
            def executable(name, text):
                path = mock / name
                path.write_text('#!/bin/bash\n' + text)
                path.chmod(0o755)
            executable('uname', 'if [ "$1" = -s ]; then echo Linux; else echo x86_64; fi\n')
            executable('curl', 'cp "$FIXTURE_ARCHIVE" "$3"\n')
            executable('mv', 'if [[ "$1" == */new/web && "${FAIL_MOVE:-}" == 1 ]]; then exit 1; fi\nexec /bin/mv "$@"\n')
            env = dict(os.environ, PATH=f'{mock}:{os.environ["PATH"]}', FIXTURE_ARCHIVE=str(archive),
                       NEOISM_BIN_DIR=str(install), NEOISM_SKIP_CHECKSUM='1', NEOISM_VERSION='fixture')
            def run(ok, **extra):
                result = subprocess.run(['bash', str(SCRIPT)], env=dict(env, **extra), capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, ok, result.stdout + result.stderr)
            pack()
            run(True)
            self.assertEqual((install / 'web/app/assets/app.js').read_text(), 'new JS')
            (install / 'web/agent-gui').mkdir()
            (install / 'web/agent-gui/stale.js').write_text('stale')
            run(True)
            self.assertFalse((install / 'web/agent-gui').exists())
            (install / 'neoism').write_text('#!/bin/sh\necho original\n')
            (install / 'unrelated').write_text('keep')
            (install / 'web/app/assets/app.js').write_text('original JS')
            run(False, FAIL_MOVE='1')
            self.assertIn('original', (install / 'neoism').read_text())
            self.assertEqual((install / 'web/app/assets/app.js').read_text(), 'original JS')
            (payload / 'web/app/manifest.json').unlink()
            pack()
            run(False)
            self.assertIn('original', (install / 'neoism').read_text())
            self.assertEqual((install / 'unrelated').read_text(), 'keep')
            self.assertEqual(list(install.glob('.neoism-install.*')), [])

if __name__ == '__main__':
    unittest.main()
