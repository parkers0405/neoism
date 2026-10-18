"""Trusted acceptance tests; candidate calls cross JSON RPC only."""
import unittest
from candidate_client import call

class Acceptance(unittest.TestCase):
    def evaluate(self, text, variables=None):
        args=[text,{} if variables is None else variables]
        reply=call("engine","evaluate",args)
        self.assertEqual(reply["args_after"],args)
        return reply["result"]
    def test_precedence(self):
        self.assertEqual(self.evaluate("2+3*4-8/2"),{"value":10})
    def test_parentheses(self):
        self.assertEqual(self.evaluate("(2+3)*(4-1)"),{"value":15})
    def test_associativity(self):
        self.assertEqual(self.evaluate("20-6-3"),{"value":11})
        self.assertEqual(self.evaluate("24/4/2"),{"value":3})
    def test_power_right_associative(self):
        self.assertEqual(self.evaluate("2^3^2"),{"value":512})
    def test_unary_power_binding(self):
        for text,value in [("-2^2",-4),("(-2)^2",4),("2^-2",0.25),("--2",2),("3*-2",-6),("+-+2",-2)]:
            with self.subTest(text=text):
                self.assertEqual(self.evaluate(text),{"value":value})
    def test_decimals_whitespace(self):
        self.assertEqual(self.evaluate(" \t.5 + 1.25\n"),{"value":1.75})
    def test_variables_case(self):
        self.assertEqual(self.evaluate("x + X * _rate",{"x":1,"X":4,"_rate":2}),{"value":9})
        self.assertEqual(self.evaluate("missing+1"),{"error":"variable","name":"missing"})
    def test_arithmetic_failures(self):
        for text in ["1/0","0^-1","(-1)^.5","10^10000"]:
            with self.subTest(text=text):
                self.assertEqual(self.evaluate(text),{"error":"arithmetic"})
    def test_illegal_syntax(self):
        for text in ["", "1+", "()", "2(3)", "1 2", "2**3", "1.", "1e2", "x.y", "f(2)", "[1]", "1%2", "2\u00a0+3"]:
            with self.subTest(text=text):
                self.assertEqual(self.evaluate(text),{"error":"syntax"})
    def test_no_python_execution(self):
        for text in ['__import__("os")', "lambda:1", "x[0]", "open(1)"]:
            with self.subTest(text=text):
                self.assertEqual(self.evaluate(text),{"error":"syntax"})
    def test_invalid_inputs(self):
        for text,variables in [(1,{}),("1",[]),("1",{"unused":True}),("1",{"bad-key":2}),("1",{"x":None}),(" "*1025,{})]:
            with self.subTest(text=text,variables=variables):
                self.assertEqual(self.evaluate(text,variables),{"error":"invalid"})
    def test_token_limit(self):
        self.assertEqual(self.evaluate("+"*128+"1"),{"error":"invalid"})
        self.assertEqual(self.evaluate("+"*127+"1"),{"value":1})
    def test_error_priority(self):
        self.assertEqual(self.evaluate("unknown + )"),{"error":"syntax"})
        self.assertEqual(self.evaluate("unknown+1/0"),{"error":"variable","name":"unknown"})
        self.assertEqual(self.evaluate("1/0+unknown"),{"error":"arithmetic"})
    def test_arithmetic_properties(self):
        for a,b,c in [(2,3,4),(-5,2,7),(0,-3,2),(8,1,-2)]:
            values={"a":a,"b":b,"c":c}
            self.assertEqual(self.evaluate("a*(b+c)-a*b",values),{"value":a*c})

    def test_nonfinite_computation(self):
        self.assertEqual(self.evaluate("9"*400),{"error":"arithmetic"})
        self.assertEqual(self.evaluate("x*x",{"x":1e300}),{"error":"arithmetic"})
        self.assertEqual(self.evaluate("(-2)^3"),{"value":-8})
    def test_null_variables_and_validation_priority(self):
        self.assertEqual(call("engine","evaluate",["1",None])["result"],{"error":"invalid"})
        self.assertEqual(self.evaluate("1 + )",{"unused":True}),{"error":"invalid"})

if __name__ == "__main__":
    unittest.main()
