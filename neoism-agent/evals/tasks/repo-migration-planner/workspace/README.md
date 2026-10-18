# Migration rollouts

`planner.py` computes the target closure, `schema.py` validates inputs, and `graph.py` schedules indivisible transaction batches. Current implementation only handles independent targets. The fixture models a small synthetic schema rollout. Public tests: `python -m unittest discover -v`.
