def summarize_movements(records):
    """Aggregate signed movements in one pass, with one bucket per SKU."""
    buckets = {}
    for record in records:
        sku = record['sku']
        if sku not in buckets:
            buckets[sku] = {'sku': sku, 'net': 0, 'movements': 0}
        buckets[sku]['net'] += record['delta']
        buckets[sku]['movements'] += 1
    return [buckets[sku] for sku in sorted(buckets)]
