"""Legacy flat calculator tree builder."""
import math
from lexer import IDENTIFIER

class Parser:
    def __init__(self, tokens):
        self.tokens = tokens

    def parse(self):
        if not self.tokens or len(self.tokens) % 2 == 0:
            raise SyntaxError("incomplete")
        def atom(token):
            if IDENTIFIER.fullmatch(token):
                return ("variable",token)
            if token.isascii() and token.isdigit():
                return ("number",token)
            raise SyntaxError("atom")
        tree = atom(self.tokens[0])
        for index in range(1, len(self.tokens), 2):
            operator = self.tokens[index]
            if operator not in ("+","-","*","/"):
                raise SyntaxError("operator")
            tree = (operator,tree,atom(self.tokens[index+1]))
        return tree

def compute(node, variables):
    value = _compute(node, variables)
    if not math.isfinite(value):
        raise ArithmeticError("nonfinite")
    return value

def _compute(node, variables):
    if node[0] == "number":
        return float(node[1])
    if node[0] == "variable":
        return variables[node[1]]
    left, right = compute(node[1], variables), compute(node[2], variables)
    if node[0] == "+":
        return left + right
    if node[0] == "-":
        return left - right
    if node[0] == "*":
        return left * right
    return left / right
