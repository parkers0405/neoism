def merge_config(base, override):
    """Combine configuration dictionaries, giving override precedence."""
    result = base.copy()
    result.update(override)
    return result
