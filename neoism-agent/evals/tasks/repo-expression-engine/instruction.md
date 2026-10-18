# Safe expression engine

Private synthetic stdlib repository task; not an upstream issue or public benchmark. Replace the legacy calculator while keeping `engine.evaluate(expression, variables)` as the JSON-facing API and maintaining separate lexer/parser/engine responsibilities. Do not use eval, exec, compile, or a Python-AST evaluator.

Expression must be a string of at most 1024 characters, tokenizing to at most 128 tokens (excluding end marker). Variables must be an object with ASCII identifier keys `[A-Za-z_][A-Za-z0-9_]*` and finite JSON number values (not booleans); validate all entries, even unused ones. Bad input shape, variable entries, length or token limit returns `{"error":"invalid"}`. No input mutation.

Tokens: ASCII identifiers, decimal numbers matching `[0-9]+(\.[0-9]+)?|\.[0-9]+`, operators `+ - * / ^`, and parentheses. Only ASCII space/tab/CR/LF are whitespace. No scientific notation, trailing decimal point, calls, implicit multiplication, attribute access, indexing, or other syntax. Empty/illegal/incomplete expressions or leftover tokens return `{"error":"syntax"}`.

Grammar (highest precision contract): sum = product ((+|-) product)*; product = unary ((*|/) unary)*; unary = (+|-) unary | power; power = atom (^ unary)?; atom = number | identifier | (sum). Thus exponentiation is right associative, -2^2 is -4, 2^-2 is 0.25, and subtraction/division are left associative. Lookup identifiers case-sensitively, returning `{"error":"variable","name":identifier}` for an unknown variable. Evaluate left operand before right; the first encountered evaluation failure wins.

All numbers/variables are converted to Python float and operators use binary floating-point real arithmetic. Success is `{"value":number}`. Division by zero, complex-valued powers, overflow, and nonfinite literals/results return `{"error":"arithmetic"}`. Expressions are fully parsed before evaluation: syntax errors take precedence over evaluation failures. Malformed input validation takes precedence over parsing. Output need not preserve integer vs float representation. No side effects, secrets, network, or dependencies.
