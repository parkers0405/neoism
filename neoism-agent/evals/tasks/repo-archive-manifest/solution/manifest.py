"""Atomic manifest indexing with shared leaf/ancestor identity space."""
from entry_schema import valid_entry
from paths import normalize
from manifest_index import ManifestIndex

def build_manifest(entries, case_sensitive=True):
    def fail(code, index=None):
        return {"entries":[],"total_bytes":0,"error":{"code":code,"index":index}}
    if type(case_sensitive) is not bool:
        return fail("invalid_options")
    if not isinstance(entries,list):
        return fail("invalid_entries")
    index_store = ManifestIndex(case_sensitive)
    for index,row in enumerate(entries):
        if not valid_entry(row):
            return fail("invalid_entry",index)
        path = normalize(row["path"],row["type"])
        if path is None:
            return fail("unsafe_path",index)
        code = index_store.insert(path,row)
        if code is not None:
            return fail(code,index)
    rows, total = index_store.finish()
    return {"entries":rows,"total_bytes":total,"error":None}
