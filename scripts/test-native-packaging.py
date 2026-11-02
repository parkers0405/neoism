#!/usr/bin/env python3
"""Exercise both Linux packaging entrypoints with native-only fixture artifacts."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parent
MARKER = b'<!-- Neoism native runtime resources; no browser UI. -->\n'


@unittest.skipUnless(platform.system() == 'Linux' and platform.machine() == 'x86_64',
                     'Linux x86_64 packaging fixtures')
class NativePackagingTests(unittest.TestCase):
    def test_packagers_discard_gui_and_recreate_only_legacy_markers(self):
        for packager in ['package-gpuix.sh', 'package-gpuix-release.py']:
            with self.subTest(packager=packager), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)

                def write(name, content, executable=False):
                    path = root / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(content)
                    if executable:
                        path.chmod(0o755)
                    return path

                for script in [packager, 'install-gpuix-bundle.sh']:
                    destination = root / 'scripts' / script
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(SCRIPTS / script, destination)
                write('Cargo.toml', '[workspace.package]\nversion = "0.0.1"\n')
                dist = 'neoism-frontend/gpuix/dist/'
                for binary in ['neoism', 'neoism-agent', 'neoism-workspace-daemon', 'neoism-agent-lua-runner']:
                    write(dist + binary, '#!/bin/sh\necho neoism 0.0.1\n', True)
                write('target/activation/neoism-agent', '#!/bin/sh\nexit 0\n', True)
                for resource in ['LICENSE', 'NOTICE', 'LICENSE.ghostty', 'libneoism_gpuix_core.so', 'libghostty-vt.so', 'licenses/font.txt']:
                    write(dist + resource, resource)
                for resource in ['LICENSE', 'NOTICE', 'misc/neoism.desktop', 'neoism-frontend/shared/assets/icons/neoism.png',
                                 'neoism-frontend/gpuix/node_modules/@gpuix/native/LICENSE',
                                 'neoism-frontend/gpuix/node_modules/@gpuix/solid/LICENSE']:
                    write(resource, resource)
                files = {'entry.js': 'native app', 'entry.js.jsc': 'bytecode'}
                for name, content in files.items():
                    write(dist + 'web/app/' + name, content)
                write(dist + 'web/app/manifest.json', json.dumps({
                    'entry': 'entry.js', 'files': {name: hashlib.sha256(content.encode()).hexdigest()
                                                 for name, content in files.items()}}))
                display = 'gpuix-display-linux-x64.node'
                write(dist + 'web/native/' + display, 'native display')
                write('neoism-frontend/gpuix/artifacts/gpuix-display-linux-x64.json', json.dumps({
                    'artifact': display, 'sha256': hashlib.sha256(b'native display').hexdigest()}))
                for name in ['index.html', 'agent-gui/index.html', 'agent-gui/assets/old.js', 'assets/old.js']:
                    write(dist + 'web/' + name, 'stale browser UI')
                # These are packaging-only fixtures: never build or run real native code.
                write('mock/cargo', '#!/bin/sh\nexit 0\n', True)
                write('mock/git', '#!/bin/sh\necho fixture-commit\n', True)
                env = dict(os.environ, PATH=str(root / 'mock') + os.pathsep + os.environ['PATH'])
                command = ['bash' if packager.endswith('.sh') else 'python3', str(root / 'scripts' / packager)]
                subprocess.run(command, cwd=root, env=env, check=True, capture_output=True, text=True)
                with tarfile.open(root / 'release/neoism-linux-x86_64.tar.gz') as archive:
                    prefix = 'neoism-linux-x86_64/web/'
                    names = {p.name for p in archive.getmembers() if p.isfile()}
                    self.assertEqual({name for name in names if name.startswith(prefix + 'agent-gui/')},
                                     {prefix + 'agent-gui/index.html'})
                    for marker in ['index.html', 'agent-gui/index.html']:
                        self.assertEqual(archive.extractfile(prefix + marker).read(), MARKER)
                    self.assertFalse(any(name.startswith(prefix + 'assets/') for name in names))
                    self.assertIn(prefix + 'app/manifest.json', names)
                    self.assertIn(prefix + 'native/' + display, names)


if __name__ == '__main__':
    unittest.main()