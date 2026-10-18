"""One-pass CSV validation, filtering and atomic finalization."""
import csv
from decimal import localcontext
from csv_io import records, valid_options, valid_header, number
from aggregation import add, finish

def summarize(lines, options):
    def fail(code):
        return {"groups":[],"accepted":0,"filtered":0,"errors":[],"error":{"code":code}}
    if not valid_options(options):
        return fail("invalid_options")
    groups, errors = {}, []
    accepted = filtered = 0
    try:
        reader = records(lines)
        header = next(reader,[])
        if not valid_header(header,options):
            return fail("invalid_header")
        where = options["where"]
        minimum = number(options["min_amount"]) if options["min_amount"] is not None else None
        with localcontext() as context:
            context.prec = 64
            for index,row in enumerate(reader,2):
                if len(row) != len(header):
                    errors.append({"row":index,"code":"width"})
                    continue
                values = dict(zip(header,row))
                if where is not None and values[where["column"]] != where["value"]:
                    filtered += 1
                    continue
                amount = number(values[options["amount"]])
                if amount is None:
                    errors.append({"row":index,"code":"amount"})
                    continue
                if minimum is not None and amount < minimum:
                    filtered += 1
                    continue
                add(groups,values[options["group"]],amount)
                accepted += 1
            output = finish(groups)
    except (TypeError,ValueError):
        return fail("invalid_lines")
    except csv.Error:
        return fail("invalid_csv")
    return {"groups":output,"accepted":accepted,"filtered":filtered,"errors":errors,"error":None}
