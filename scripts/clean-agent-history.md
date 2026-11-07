# Clean saved Neoism history

This is an offline saved-history cleaner, not a desktop text filter. It replaces words in existing conversation titles, message text, reasoning text, and the agent composer's saved prompt history. It uses Neoism's native Turso 0.8.3 engine, including its custom full-text-index support; Python's SQLite reader cannot read this database schema.

## Run

Quit Neoism and its agent server first. The initial utility supports Linux and checks visible `/proc` file descriptors for other history owners, refusing when one is found. Some processes hide their file descriptors, so this is a best-effort check, not proof of exclusive ownership. Do not restart Neoism until cleanup finishes. Cargo and a Rust toolchain are required on the first run; compilation is separate from the desktop build and does not build a release desktop binary.

Scan first, without replacing history:

```sh
python3 scripts/clean-agent-history.py
```

Back up and apply replacements:

```sh
python3 scripts/clean-agent-history.py --apply
```

Both commands print counts, not conversation contents. The cleaner uses the expanded dictionary from `silly-words.py`, including case-insensitive matching, whole-word boundaries, and known disguised spellings. It does not detect every possible spelling. Some entries have innocent meanings; review personal exceptions before applying this broad list.

Use personal replacements or disable individual words with `null` in a JSON object:

```sh
python3 scripts/clean-agent-history.py --replacements /path/to/words.json
python3 scripts/clean-agent-history.py --replacements /path/to/words.json --apply
```

For example:

```json
{"nuts": null, "screw": null, "cracker": null, "damn": "flibber"}
```

Custom dictionary keys currently must be ASCII. `--db` and `--history` select different storage paths. The usual `NEOISM_AGENT_STATE_DIR` and `NEOISM_AGENT_PROMPT_HISTORY_FILE` environment overrides are respected. Pass `--binary /path/to/neoism-history-cleaner` to use an already-built helper instead of invoking Cargo.

## Safety and scope

Apply mode creates a private, timestamped `agent.turso.db.history-backup-*` directory alongside the database before changing history. It contains the original database, existing WAL/SHM sidecars, and prompt-history file. A failed backup aborts cleanup. Originals in the backup still contain the unfiltered wording. Keep the backup until you have reopened Neoism and verified the result.

Database edits run in one transaction and roll back on errors. Prompt history is replaced separately, using a temporary file and rename after the database commits. These two stores are not one atomic transaction; the backup covers both if the second step fails. The cleaner checks for concurrent access, but it is not a process supervisor: Neoism must stay closed throughout the operation.

Affected lexical search documents are removed and queued for rebuilding when Neoism restarts. Affected semantic embeddings are invalidated. Session/message IDs, timestamps, tool calls, tool output, code files, event journals, and other metadata are left alone. The cleaner does not securely erase original bytes from disk or scrub all historical copies, exports, backups, logs, or tool dumps. This cleans the saved conversational text Neoism displays, not every occurrence on your computer. Future messages are not automatically filtered.