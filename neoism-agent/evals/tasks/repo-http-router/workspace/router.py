"""Registration-order dispatcher; route patterns need upgrading."""
from patterns import compile_path, match, method
from query import split_target

def dispatch(routes, request):
    try:
        if not isinstance(routes, list) or not isinstance(request, dict):
            raise ValueError("shape")
        wanted = method(request.get("method"))
        parts, query = split_target(request.get("target"))
        for route in routes:
            if not isinstance(route, dict) or not isinstance(route.get("handler"), str):
                raise ValueError("route")
            verb = method(route.get("method"))
            pattern = compile_path(route.get("path"))
            params = match(pattern, parts)
            if verb == wanted and params is not None:
                return {"status":200,"handler":route["handler"],"params":params,"query":query}
        return {"status":404}
    except (ValueError, TypeError):
        return {"error":"invalid"}
