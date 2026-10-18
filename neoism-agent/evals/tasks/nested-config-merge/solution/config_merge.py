def _clone(value):
    if isinstance(value, dict):
        return {key: _clone(item) for key, item in value.items()}
    if isinstance(value, list):
        return [_clone(item) for item in value]
    return value


def merge_config(base, override):
    """Merge dictionaries while detaching all mutable result values."""
    result = _clone(base)
    for key, value in override.items():
        if key in base and isinstance(base[key], dict) and isinstance(value, dict):
            result[key] = merge_config(base[key], value)
        else:
            result[key] = _clone(value)
    return result
