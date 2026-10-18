"""Validated JSON facade for the expression language."""
import math
from lexer import IDENTIFIER, tokenize
from parser import Parser, compute

def evaluate(expression, variables):
    if not isinstance(expression, str) or len(expression) > 1024 or not isinstance(variables, dict):
        return {"error": "invalid"}
    converted = {}
    try:
        for key, value in variables.items():
            if not isinstance(key, str) or not IDENTIFIER.fullmatch(key) or type(value) not in (int, float):
                raise ValueError("variable")
            converted[key] = float(value)
            if not math.isfinite(converted[key]):
                raise ValueError("finite")
        tokens = tokenize(expression)
    except (ValueError, OverflowError):
        return {"error": "invalid"}
    except SyntaxError:
        return {"error": "syntax"}
    try:
        tree = Parser(tokens).parse()
    except (SyntaxError, RecursionError):
        return {"error": "syntax"}
    try:
        return {"value": compute(tree, converted)}
    except KeyError as error:
        return {"error": "variable", "name": error.args[0]}
    except (ArithmeticError, ValueError):
        return {"error": "arithmetic"}
