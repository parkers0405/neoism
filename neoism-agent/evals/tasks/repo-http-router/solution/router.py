"""JSON-facing dispatcher."""
from patterns import compile_path, match, method, rank
from query import split_target

def dispatch(routes, request):
    try:
        if not isinstance(routes, list) or not isinstance(request, dict):
            raise ValueError("shape")
        wanted = method(request.get("method"))
        parts, query = split_target(request.get("target"))
        compiled = []
        for route in routes:
            if not isinstance(route, dict) or not isinstance(route.get("handler"), str):
                raise ValueError("route")
            compiled.append((method(route.get("method")), compile_path(route.get("path")), route["handler"]))
    except (ValueError, TypeError):
        return {"error": "invalid"}
    hits, allowed = [], set()
    for index, (verb, pattern, handler) in enumerate(compiled):
        params = match(pattern, parts)
        if params is None:
            continue
        allowed.add(verb)
        if verb == wanted:
            hits.append((rank(pattern), -index, handler, params))
    if hits:
        _, _, handler, params = max(hits, key=lambda hit: (hit[0], hit[1]))
        return {"status": 200, "handler": handler, "params": params, "query": query}
    if allowed:
        return {"status": 405, "allowed": sorted(allowed)}
    return {"status": 404}
