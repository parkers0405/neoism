import unittest
from pipeline import summarize

class PublicTests(unittest.TestCase):
    def test_header_only(self):
        options = {"group":"team","amount":"amount","where":None,"min_amount":None}
        self.assertEqual(summarize(["team,amount\n"],options)["accepted"],0)

    def test_simple(self):
        options = {"group":"team","amount":"amount","where":None,"min_amount":None}
        self.assertEqual(summarize(["team,amount\n","blue,1.50\n"],options)["groups"],[{"key":"blue","count":1,"total":"1.50"}])

    def test_round_after_summing(self):
        options = {"group":"team","amount":"amount","where":None,"min_amount":None}
        report = summarize(["team,amount\n","blue,.005\n","blue,.005\n"],options)
        self.assertEqual(report["groups"],[{"key":"blue","count":2,"total":"0.01"}])

if __name__ == "__main__":
    unittest.main()
