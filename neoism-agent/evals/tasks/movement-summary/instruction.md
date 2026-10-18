# Refactor inventory movement aggregation

Refactor `summarize_movements(records)` in `movements.py` to remove the separate counting and summing passes and make it work correctly for one-pass iterables, including generators. Each record is a dictionary with a string `sku` and an integer `delta`. Additional fields may exist and must be ignored. Inputs are valid; validation of malformed records is out of scope.

Return a list of dictionaries with exactly `sku`, `net`, and `movements` keys, sorted by SKU using Python's ordinary case-sensitive string ordering. `net` is the sum of all signed deltas for that SKU; `movements` is the number of its records, including zero-delta records. Include SKUs whose net is zero. Empty input returns an empty list. Empty-string SKUs are valid. Do not strip, normalize, or otherwise change SKU strings.

Consume the input at most once and aggregate incrementally, using storage proportional to distinct SKUs rather than buffering all records. Read each record before advancing the iterator; streaming producers may reuse a dictionary for subsequent records. Do not mutate the records or retain them in the returned result. Keep the public function signature and use only the Python standard library, without external services or dependencies.

Public checks: `python -m unittest discover -s . -p 'test_*.py'`. Additional acceptance tests exercise the same contract.
