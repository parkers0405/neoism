"""Graph ordering and transaction packaging."""
def topological(nodes, dependencies):
    return sorted(nodes)

def steps(rows):
    order = topological(rows, {k: set(v["requires"]) & rows.keys() for k,v in rows.items()})
    return [{"transaction":rows[k]["transaction"], "migrations":[k]} for k in order], None
