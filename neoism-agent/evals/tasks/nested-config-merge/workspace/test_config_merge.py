import unittest

from config_merge import merge_config


class PublicTests(unittest.TestCase):
    def test_flat_override(self):
        self.assertEqual(merge_config({'a': 1}, {'a': 2, 'b': 3}), {'a': 2, 'b': 3})

    def test_nested_keys_survive(self):
        self.assertEqual(
            merge_config({'ui': {'size': 12, 'theme': 'dark'}}, {'ui': {'size': 14}}),
            {'ui': {'size': 14, 'theme': 'dark'}},
        )


if __name__ == '__main__':
    unittest.main()
