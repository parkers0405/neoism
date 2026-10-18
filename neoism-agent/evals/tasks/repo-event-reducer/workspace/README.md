# Event journal mini-project

`event_schema.py` validates persisted state and journal records; `event_ops.py` applies one versioned record; `reducer.py` owns batch atomicity. The implementation currently mishandles replays, stale versions, and tombstones. See the task contract. Fixtures are deliberately synthetic.
