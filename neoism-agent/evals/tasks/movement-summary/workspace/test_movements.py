import unittest

from movements import summarize_movements


class PublicTests(unittest.TestCase):
    def test_single_sku(self):
        self.assertEqual(
            summarize_movements([{'sku': 'A', 'delta': 2}, {'sku': 'A', 'delta': 3}]),
            [{'sku': 'A', 'net': 5, 'movements': 2}],
        )

    def test_signed_generator(self):
        records = ({'sku': 'A', 'delta': delta} for delta in (5, -2))
        self.assertEqual(summarize_movements(records), [{'sku': 'A', 'net': 3, 'movements': 2}])


if __name__ == '__main__':
    unittest.main()
