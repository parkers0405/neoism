"""Run with: python3 -m unittest discover -s scripts -p test_silly_words.py"""

import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path


spec = importlib.util.spec_from_file_location("silly_words", Path(__file__).with_name("silly-words.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class WordFilterTests(unittest.TestCase):
    def setUp(self):
        self.filter = module.WordFilter(module.default_replacements()).filter

    def test_words_case_and_punctuation(self):
        self.assertEqual(self.filter("Shit! FUCK, piss; cum."), "Beans! FUDGE, apple juice; frosting.")

    def test_innocent_words_and_identifiers(self):
        text = "class classic cumulative circumstance Scunthorpe shell assistant shit_count"
        self.assertEqual(self.filter(text), text)

    def test_compounds_variants_and_disguises(self):
        self.assertEqual(self.filter("motherfucking sh1t b*tch"), "fudge beans wafflehead")

    def test_slurs(self):
        self.assertEqual(self.filter("nigger faggots"), "silly goose silly goose")

    def test_unicode_boundaries(self):
        text = "cafeshit shit\u00e9 sh\u0131t"
        self.assertEqual(self.filter(text), text)

    def test_replacements_are_not_filtered_recursively(self):
        word_filter = module.WordFilter({"shit": "fuck", "fuck": "fudge"})
        self.assertEqual(word_filter.filter("shit fuck"), "fuck fudge")

    def test_empty_dictionary(self):
        self.assertEqual(module.WordFilter({}).filter("unchanged"), "unchanged")

    def test_full_dictionary_in_lower_upper_and_title_case(self):
        for word, replacement in module.default_replacements().items():
            for text in (word, word.upper(), word.title()):
                with self.subTest(word=text):
                    expected = replacement
                    if text.isupper():
                        expected = replacement.upper()
                    elif text.istitle():
                        expected = replacement.capitalize()
                    self.assertEqual(self.filter(text), expected)

    def test_scan_from_stdin_reports_counts_without_replacing(self):
        result = subprocess.run(
            [sys.executable, str(Path(module.__file__)), "--scan"],
            input="DAMN damn dammit! class cumulative\nclusterfuck sh1tty\n",
            text=True, capture_output=True, check=True,
        )
        self.assertEqual(json.loads(result.stdout), {
            "damn": 2, "dammit": 1, "clusterfuck": 1, "sh1tty": 1,
        })


if __name__ == "__main__":
    unittest.main()