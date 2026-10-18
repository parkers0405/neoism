"""Host-independent lexical normalization; never resolves traversal."""
import re

def normalize(path, kind):
    path = path.replace("\\", "/")
    if path.startswith("/") or re.match(r"^[A-Za-z]:",path) or "\x00" in path:
        return None
    parts = path.split("/")
    if ".." in parts or (kind == "file" and path.endswith("/")):
        return None
    clean = "/".join(part for part in parts if part not in ("", "."))
    return clean or None

def ancestors(path):
    pieces = path.split("/")
    return ["/".join(pieces[:i]) for i in range(1,len(pieces))]

def identity(path, case_sensitive):
    return path if case_sensitive else path.casefold()
