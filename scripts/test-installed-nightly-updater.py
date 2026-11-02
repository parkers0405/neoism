#!/usr/bin/env python3
"""Exercise the deployed updater against GitHub in an isolated installation."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

installed = Path(sys.argv[1]).resolve()
expected = sys.argv[2]
with tempfile.TemporaryDirectory(prefix='neoism-old-updater-test-') as temporary:
    root = Path(temporary)
    # Never point the updater's process shutdown or replacement at the live app.
    for name in ['neoism', 'neoism-workspace-daemon', 'neoism-agent', 'neoism-agent-lua-runner']:
        shutil.copy2(installed / name, root / name)
    shutil.copytree(installed / 'web', root / 'web')
    executable = root / 'neoism'
    subprocess.run([executable, '--version'], check=True)
    subprocess.run([executable, 'update', '--nightly', '--no-relaunch'], check=True)
    version = subprocess.check_output([executable, '--version'], text=True).strip()
    assert version == f'neoism {expected}', version
    subprocess.run([executable, '--neoism-verify-runtime'], check=True)
    assert (root / 'web/agent-gui/index.html').read_text() == '<!-- Neoism native runtime resources; no browser UI. -->\n'
    assert sorted(path.name for path in (root / 'web/agent-gui').iterdir()) == ['index.html']
    print(f'PASS: deployed updater downloaded and installed {version}; native runtime verified; live installation untouched')