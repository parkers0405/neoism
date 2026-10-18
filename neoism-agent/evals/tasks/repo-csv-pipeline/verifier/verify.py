import unittest
from candidate_client import call


def options(**changes):
    result = {"group": "team", "amount": "amount", "where": None, "min_amount": None}
    result.update(changes)
    return result


def group(key, count, total):
    return {"key": key, "count": count, "total": total}


class Acceptance(unittest.TestCase):
    def report(self, lines, opts=None, iterator=None):
        args = [lines, options() if opts is None else opts]
        response = call("pipeline", "summarize", args, iterator=iterator)
        self.assertEqual(response["args_after"], args)
        return response["result"]

    def fatal(self, lines, code, opts=None):
        self.assertEqual(self.report(lines, opts), {"groups": [], "accepted": 0, "filtered": 0, "errors": [], "error": {"code": code}})

    def test_header_only(self):
        self.assertEqual(self.report(["team,amount\n"]), {"groups": [], "accepted": 0, "filtered": 0, "errors": [], "error": None})

    def test_group_and_sort(self):
        r = self.report(["team,amount\n", "z,1.2\n", "a,2\n", "z,3\n"])
        self.assertEqual(r["groups"], [group("a", 1, "2.00"), group("z", 2, "4.20")])
        self.assertEqual(r["accepted"], 3)

    def test_round_once(self):
        self.assertEqual(self.report(["team,amount\n", "a,.005\n", "a,.005\n"])["groups"], [group("a", 2, "0.01")])

    def test_half_up_and_negative_zero(self):
        r = self.report(["team,amount\n", "z,-0.004\n", "n,-1.005\n", "p,1.005\n"])
        self.assertEqual(r["groups"], [group("n", 1, "-1.01"), group("p", 1, "1.01"), group("z", 1, "0.00")])

    def test_filter_before_amount(self):
        opts = options(where={"column": "status", "value": "paid"})
        r = self.report(["team,amount,status\n", "a,garbage,skip\n", "a,2,paid\n"], opts)
        self.assertEqual(r, {"groups": [group("a", 1, "2.00")], "accepted": 1, "filtered": 1, "errors": [], "error": None})

    def test_inclusive_minimum(self):
        r = self.report(["team,amount\n", "a,-1\n", "a,0\n", "a,.001\n"], options(min_amount=" 0 "))
        self.assertEqual(r["groups"], [group("a", 2, "0.00")])
        self.assertEqual((r["accepted"], r["filtered"]), (2, 1))

    def test_width_before_filter_and_error_order(self):
        r = self.report(["team,amount,status\n", "a,x\n", "a,bad,yes\n", "a,2,no\n", "a,3,yes,extra\n"], options(where={"column": "status", "value": "yes"}))
        self.assertEqual(r["errors"], [{"row": 2, "code": "width"}, {"row": 3, "code": "amount"}, {"row": 5, "code": "width"}])
        self.assertEqual((r["accepted"], r["filtered"]), (0, 1))

    def test_decimal_grammar_and_precision(self):
        lines = ["team,amount\n"] + ["a," + value + "\n" for value in ["NaN", "Infinity", "1e2", "1_0", "1.", "0.0000001"]]
        lines += ["a,999999999999999999.999999\n", "a,0.000001\n"]
        r = self.report(lines)
        self.assertEqual(r["errors"], [{"row": i, "code": "amount"} for i in range(2, 8)])
        self.assertEqual(r["groups"], [group("a", 2, "1000000000000000000.00")])

    def test_quoted_multiline_and_record_numbers(self):
        r = self.report(["team,amount\n", '"a,\n', 'b",1.005\n', "bad\n"])
        self.assertEqual(r["groups"], [group("a,\nb", 1, "1.01")])
        self.assertEqual(r["errors"], [{"row": 3, "code": "width"}])

    def test_bom_exact_keys_and_escaping(self):
        r = self.report(['\ufeffteam,amount\n', '"a""b", 2.5 \n', ' a,1\n'])
        self.assertEqual(r["groups"], [group(" a", 1, "1.00"), group('a"b', 1, "2.50")])

    def test_generator(self):
        r = self.report(["team,amount\n", "z,.005\n", "z,.005\n"], iterator="generator")
        self.assertEqual(r["groups"], [group("z", 2, "0.01")])

    def test_invalid_header(self):
        for lines in [[], ["team,team\n"], ["team,\n"], ["team,other\n"], ["\n"]]:
            with self.subTest(lines=lines):
                self.fatal(lines, "invalid_header")

    def test_fatal_csv_is_atomic(self):
        self.fatal(["team,amount\n", "a,1\n", "bad\n", '"unterminated'], "invalid_csv")

    def test_bad_lines_atomic(self):
        for lines in [None, 7, "team,amount", {}, ["team,amount\n", "a,1\n", 3]]:
            with self.subTest(lines=lines):
                self.fatal(lines, "invalid_lines")

    def test_invalid_options_first(self):
        for opts in [{}, options(min_amount="NaN"), options(where={"column": "x", "value": 1}), options(extra=True)]:
            with self.subTest(opts=opts):
                self.fatal(None, "invalid_options", opts)

    def test_blank_records_and_empty_group(self):
        r = self.report(["team,amount\n", "\n", ",+1\n"])
        self.assertEqual(r["groups"], [group("", 1, "1.00")])
        self.assertEqual(r["errors"], [{"row": 2, "code": "width"}])


if __name__ == "__main__":
    unittest.main()
