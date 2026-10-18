# Normalize plain-text notes

Complete `normalize_text(text)` in `note_text.py`. It accepts a Python string and returns a canonical plain-text note with these rules:

- Treat CRLF and standalone CR as line endings, converting them to LF.
- Remove trailing ASCII spaces and tabs from every line. Preserve leading indentation, internal spacing, and all other characters (including Unicode whitespace).
- Remove empty lines at the start and end after that trimming. Collapse each remaining run of empty lines to one empty line.
- Return an empty string if no content remains. Otherwise end the result with exactly one LF.

Only CR and LF are line separators for this API. Unicode line-separator characters remain literal content. A line containing only ASCII spaces or tabs becomes empty; other whitespace does not. Applying normalization again must not change the result.

Keep the function signature and use only the Python standard library. No third-party dependencies, model calls, or external services are needed. Public checks: `python -m unittest discover -s . -p 'test_*.py'`. Additional acceptance checks follow the same contract.
