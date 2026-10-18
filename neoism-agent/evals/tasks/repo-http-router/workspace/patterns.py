"""Legacy literal-only path helpers."""
import re

def method(value):
    if not isinstance(value, str) or not re.fullmatch("[A-Za-z]+", value):
        raise ValueError("method")
    return value.upper()

def segments(path):
    if not isinstance(path, str) or not path.startswith("/"):
        raise ValueError("path")
    return path.strip("/").split("/") if path.strip("/") else []

def compile_path(path):
    return segments(path)

def match(pattern, parts):
    return {} if pattern == parts else None

def rank(pattern):
    return len(pattern)
