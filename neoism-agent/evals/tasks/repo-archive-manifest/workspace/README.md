# Parcel manifest

Synthetic archive planning without I/O. `entry_schema.py` owns record shape, `paths.py` owns portable path rules, `manifest.py` owns identity/collision indexing and totals. The current happy-path implementation loses duplicates and omits parents. Public tests run with unittest discovery.
