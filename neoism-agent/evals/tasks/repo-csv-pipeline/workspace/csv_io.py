"""CSV record stream, options and decimal syntax."""
import csv
import re
from decimal import Decimal

DECIMAL = re.compile(r"[+-]?(?:[0-9]{1,18}(?:\.[0-9]{1,6})?|\.[0-9]{1,6})\Z")

def number(value):
    if not isinstance(value,str) or not DECIMAL.fullmatch(value.strip()):
        return None
    return Decimal(value.strip())

def valid_options(options):
    if not isinstance(options,dict) or set(options) != {"group","amount","where","min_amount"}:
        return False
    if not all(isinstance(options[k],str) and options[k] for k in ("group","amount")):
        return False
    where = options["where"]
    if where is not None:
        if not isinstance(where,dict) or set(where) != {"column","value"}:
            return False
        if not isinstance(where["column"],str) or not where["column"] or not isinstance(where["value"],str):
            return False
    return options["min_amount"] is None or number(options["min_amount"]) is not None

def records(lines):
    if isinstance(lines,(str,dict)) or lines is None:
        raise TypeError("lines must be an iterable of strings")
    def checked():
        for line in lines:
            if not isinstance(line,str):
                raise TypeError("line is not a string")
            yield line
    return csv.reader(checked(),strict=True)

def valid_header(header, options):
    if header:
        header[0] = header[0].removeprefix("\ufeff")
    required = [options["group"],options["amount"]]
    if options["where"] is not None:
        required.append(options["where"]["column"])
    return (bool(header) and all(header) and len(set(header)) == len(header)
            and all(key in header for key in required))
