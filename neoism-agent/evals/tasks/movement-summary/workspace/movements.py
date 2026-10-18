def summarize_movements(records):
    """Summarize inventory records by SKU."""
    counts = {}
    for record in records:
        sku = record['sku']
        counts[sku] = counts.get(sku, 0) + 1

    totals = {}
    for record in records:
        sku = record['sku']
        totals[sku] = totals.get(sku, 0) + abs(record['delta'])

    return [
        {'sku': sku, 'net': totals.get(sku, 0), 'movements': counts[sku]}
        for sku in counts
    ]
