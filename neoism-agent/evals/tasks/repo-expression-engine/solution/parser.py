"""Recursive descent into a small, non-executable expression tree."""
import math
from lexer import IDENTIFIER

class Parser:
    def __init__(self, tokens):
        self.tokens = tokens
        self.index = 0

    def peek(self):
        return self.tokens[self.index] if self.index < len(self.tokens) else None

    def take(self):
        token = self.peek()
        if token is None:
            raise SyntaxError("end")
        self.index += 1
        return token

    def parse(self):
        node = self.sum()
        if self.peek() is not None:
            raise SyntaxError("trailing")
        return node

    def sum(self):
        node = self.product()
        while self.peek() in ("+", "-"):
            op = self.take()
            node = (op, node, self.product())
        return node

    def product(self):
        node = self.unary()
        while self.peek() in ("*", "/"):
            op = self.take()
            node = (op, node, self.unary())
        return node

    def unary(self):
        if self.peek() in ("+", "-"):
            return ("unary", self.take(), self.unary())
        return self.power()

    def power(self):
        node = self.atom()
        if self.peek() == "^":
            self.take()
            node = ("^", node, self.unary())
        return node

    def atom(self):
        token = self.take()
        if token == "(":
            node = self.sum()
            if self.take() != ")":
                raise SyntaxError("parenthesis")
            return node
        if IDENTIFIER.fullmatch(token):
            return ("variable", token)
        if token[0] in "0123456789.":
            return ("number", token)
        raise SyntaxError("atom")

def finite(value):
    if not isinstance(value, (int, float)) or not math.isfinite(value):
        raise ArithmeticError("non-real or infinite")
    return value

def compute(node, variables):
    kind = node[0]
    if kind == "number":
        return finite(float(node[1]))
    if kind == "variable":
        if node[1] not in variables:
            raise KeyError(node[1])
        return variables[node[1]]
    if kind == "unary":
        value = compute(node[2], variables)
        return value if node[1] == "+" else -value
    left = compute(node[1], variables)
    right = compute(node[2], variables)
    if kind == "+":
        value = left + right
    elif kind == "-":
        value = left - right
    elif kind == "*":
        value = left * right
    elif kind == "/":
        value = left / right
    else:
        value = left ** right
    return finite(value)
