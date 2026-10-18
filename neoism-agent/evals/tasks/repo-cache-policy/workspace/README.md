# Cache laboratory

A synthetic deterministic cache simulator. `cache_schema.py` validates JSON requests, `cache_policy.py` maintains entries, and `cache_api.py` coordinates batches. The current policy is FIFO-like, ignores TTL, and commits partially on invalid batches. Fix against the task contract. Tests: `python -m unittest discover -v`.
