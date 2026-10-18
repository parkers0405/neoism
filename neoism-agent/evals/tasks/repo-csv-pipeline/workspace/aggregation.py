"""Monetary totals indexed by exact group key."""
from decimal import Decimal, ROUND_HALF_UP

def add(groups, key, amount):
    row = groups.setdefault(key,{"count":0,"total":Decimal(0)})
    row["count"] += 1
    row["total"] += amount.quantize(Decimal("0.01"),rounding=ROUND_HALF_UP)

def finish(groups):
    return [{"key":key,"count":row["count"],"total":format(row["total"],".2f")} for key,row in groups.items()]
