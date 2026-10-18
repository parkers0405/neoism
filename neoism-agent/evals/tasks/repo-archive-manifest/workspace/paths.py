"""Portable lexical archive paths."""
def normalize(path, kind):
    return path.replace("\\", "/").strip("/")

def ancestors(path):
    pieces = path.split("/")
    return ["/".join(pieces[:i]) for i in range(1,len(pieces))]

def identity(path, case_sensitive):
    return path if case_sensitive else path.casefold()
