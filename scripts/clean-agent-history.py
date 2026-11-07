#!/usr/bin/env python3
"""Clean saved Neoism chats and prompt history, using Turso instead of SQLite.

Default: scan only. Quit Neoism before using --apply. Requires Cargo, or pass
--binary pointing to an already-built neoism-history-cleaner executable.
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="back up and rewrite saved history (offline only)")
    parser.add_argument("--db", type=Path, default=Path(os.environ.get(
        "NEOISM_AGENT_STATE_DIR", Path.home() / ".local/state/neoism")) / "agent.turso.db")
    parser.add_argument("--history", type=Path, default=Path(os.environ.get(
        "NEOISM_AGENT_PROMPT_HISTORY_FILE", Path.home() / ".local/share/neoism/agent_prompt_history")))
    parser.add_argument("--replacements", type=Path, help="personal JSON overrides, as for silly-words.py")
    parser.add_argument("--binary", type=Path, help="use an existing cleaner binary instead of cargo run")
    args = parser.parse_args()
    directory = Path(__file__).resolve().parent
    spec = importlib.util.spec_from_file_location("silly_words", directory / "silly-words.py")
    words = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(words)
    try:
        replacements = words.load_replacements(args.replacements)
        command = [str(args.binary.resolve())] if args.binary else [
            "cargo", "run", "--quiet", "--manifest-path",
            str(directory / "history-cleaner/Cargo.toml"), "--",
        ]
        command.extend([str(args.db.resolve()), str(args.history.resolve())])
        if args.apply:
            command.append("--apply")
        return subprocess.run(command, input=json.dumps(replacements), text=True).returncode
    except (OSError, ValueError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    sys.exit(main())