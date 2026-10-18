"""Trusted acceptance checks; candidate code runs only through JSON RPC."""
import unittest

from candidate_client import call


def normalize_text(text):
    response = call('note_text', 'normalize_text', [text])
    if response['args_after'] != [text]:
        raise AssertionError('Candidate changed its input')
    return response['result']


class TextAcceptance(unittest.TestCase):
    def test_empty_and_ascii_blank(self):
        for text in ('', ' ', '\t', '\n\r\n', ' \t\r\n\t \r '):
            with self.subTest(text=text):
                self.assertEqual(normalize_text(text), '')

    def test_final_newline(self):
        for text in ('hello', 'hello\n', 'hello\n\n'):
            with self.subTest(text=text):
                self.assertEqual(normalize_text(text), 'hello\n')

    def test_mixed_line_endings(self):
        self.assertEqual(normalize_text('one\r\ntwo\rthree\nfour'), 'one\ntwo\nthree\nfour\n')

    def test_trailing_ascii_whitespace(self):
        self.assertEqual(normalize_text('one \t\n two\t \nthree  '), 'one\n two\nthree\n')

    def test_leading_and_internal_spacing(self):
        self.assertEqual(normalize_text('  alpha  beta\n\tgamma\t delta'), '  alpha  beta\n\tgamma\t delta\n')

    def test_blank_runs(self):
        self.assertEqual(normalize_text('\n \t\nA\n\n \t\n\nB\n\n'), 'A\n\nB\n')

    def test_unicode_whitespace_is_content(self):
        self.assertEqual(normalize_text('\u00a0\nA\u00a0 \t\n\u2003'), '\u00a0\nA\u00a0\n\u2003\n')

    def test_non_cr_lf_separators_are_literal(self):
        self.assertEqual(normalize_text('a\u2028b\u2029c\vd\fe'), 'a\u2028b\u2029c\vd\fe\n')

    def test_unicode_and_punctuation(self):
        self.assertEqual(normalize_text('café 😀\t\r\n# 标题  '), 'café 😀\n# 标题\n')

    def test_idempotence(self):
        for text in ('\r A \r\n\r\n B\t\n', '', '\tX\rY\n\n', '\u00a0\n\nZ'):
            with self.subTest(text=text):
                result = normalize_text(text)
                self.assertEqual(normalize_text(result), result)


if __name__ == '__main__':
    unittest.main()
