"""Simulation batch entry point."""
from cache_schema import valid_config, valid_operation
from cache_policy import Cache

def simulate(config, operations):
    result = {"entries":[],"capacity":None,"results":[],"error":None}
    if not valid_config(config):
        result["error"] = {"code":"invalid_config","index":None}
        return result
    result["capacity"] = config["capacity"]
    if not isinstance(operations,list):
        result["error"] = {"code":"invalid_operation","index":None}
        return result
    cache = Cache(config)
    previous = 0
    for index,row in enumerate(operations):
        if not valid_operation(row,previous):
            result["error"] = {"code":"invalid_operation","index":index}
            return result
        previous = row["now"]
        result["results"].append(cache.apply(row))
        result["entries"] = cache.snapshot()
        result["capacity"] = cache.capacity
    return result
