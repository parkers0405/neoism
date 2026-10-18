# Repair nested configuration merging

Fix `merge_config(base, override)` in `config_merge.py`. Both arguments are dictionaries containing JSON-style values (string keys, dictionaries, lists, strings, numbers, booleans, and None). Return a new dictionary with base-only keys preserved and override values taking precedence. When both values at a key are dictionaries, merge them recursively; otherwise replace the base value entirely, including when the override is a list, None, False, zero, or an empty string. Lists are replaced, not concatenated.

Neither input may be changed. Mutable dictionaries and lists anywhere in the returned result must be independent of both inputs, including inside lists and unchanged branches. Empty dictionaries follow the same recursive merge rule as other dictionaries. You need not support cyclic objects, custom types, or non-dictionary top-level arguments.

Keep the public function signature. Use only the Python standard library, with no third-party dependencies or external services. Public checks: `python -m unittest discover -s . -p 'test_*.py'`. Additional acceptance tests exercise the same contract.
