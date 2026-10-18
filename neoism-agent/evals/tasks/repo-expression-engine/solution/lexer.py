"""Small explicit tokenizer, not Python syntax."""
import re

IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")
TOKEN = re.compile(r"[0-9]+(?:\.[0-9]+)?|\.[0-9]+|[A-Za-z_][A-Za-z0-9_]*|[+*/^()\-]")

def tokenize(text):
    tokens, index = [], 0
    while index < len(text):
        if text[index] in " \t\r\n":
            index += 1
            continue
        match = TOKEN.match(text, index)
        if match is None:
            raise SyntaxError("token")
        tokens.append(match.group())
        index = match.end()
        if len(tokens) > 128:
            raise ValueError("token limit")
    return tokens
