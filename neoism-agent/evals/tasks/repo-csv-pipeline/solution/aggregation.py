"""Accumulate exact amounts and round only at report finalization."""
from decimal import Decimal, ROUND_HALF_UP

def add(groups, key, amount):
    row = groups.setdefault(key,{"count":0,"total":Decimal(0)})
    row["count"] += 1
    row["total"] += amount

def finish(groups):
    output = []
    for key in sorted(groups):
        row = groups[key]
        rounded = row["total"].quantize(Decimal("0.01"),rounding=ROUND_HALF_UP)
        if rounded == 0:
            rounded = abs(rounded)
        output.append({"key":key,"count":row["count"],"total":format(rounded,".2f")})
    return output
