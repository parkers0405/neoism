import unittest

from note_text import normalize_text


class PublicTests(unittest.TestCase):
    def test_simple_note(self):
        self.assertEqual(normalize_text('hello'), 'hello\n')

    def test_preserve_indentation(self):
        self.assertEqual(normalize_text('  code  \n'), '  code\n')

    def test_empty(self):
        self.assertEqual(normalize_text(''), '')


if __name__ == '__main__':
    unittest.main()
