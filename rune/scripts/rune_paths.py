"""Build output defaults shared by the Rune checks and app/build.sh."""
import os
from pathlib import Path


def default_binary():
    cache = Path(os.environ.get('XDG_CACHE_HOME') or Path.home() / '.cache')
    target = Path(os.environ.get('CARGO_TARGET_DIR') or cache / 'neoism-rune-target')
    # build.sh runs Cargo from rune/app, including for relative target overrides.
    if not target.is_absolute():
        target = Path(__file__).resolve().parents[1] / 'app' / target
    return str(target / 'release' / 'neoism-rune')