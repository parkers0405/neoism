# CSV ledger report

`csv_io.py` provides a checked single-pass CSV stream and lexical decimal validation. `aggregation.py` owns group arithmetic; `pipeline.py` owns record ordering and reporting. Current behavior omits filters and rounds too early. The fixture is a synthetic ledger. Use `python -m unittest discover -v`.
