"""Public manifest assembly API."""
from entry_schema import valid_entry, record
from paths import normalize, identity, ancestors

def build_manifest(entries, case_sensitive=True):
    def fail(code, index=None):
        return {"entries":[],"total_bytes":0,"error":{"code":code,"index":index}}
    if type(case_sensitive) is not bool:
        return fail("invalid_options")
    if not isinstance(entries,list):
        return fail("invalid_entries")
    output = {}
    for index,row in enumerate(entries):
        if not valid_entry(row):
            return fail("invalid_entry",index)
        path = normalize(row["path"],row["type"])
        if path is None:
            return fail("unsafe_path",index)
        output[identity(path,case_sensitive)] = record(path,row)
    rows = sorted(output.values(),key=lambda row:row["path"])
    return {"entries":rows,"total_bytes":sum(r.get("size",0) for r in rows),"error":None}
