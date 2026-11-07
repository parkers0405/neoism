# Silly words filter

Run this standalone Python 3 script inside Neoism's terminal. It filters standard input into standard output, without editing source files or installing anything. It does not yet filter Neoism's chat, editors, or UI automatically.

```sh
python3 scripts/silly-words.py --text 'This is some bullshit!'
# This is some beans!

some-command 2>&1 | python3 scripts/silly-words.py
python3 scripts/silly-words.py < notes.txt
```

The built-in list includes hundreds of explicit swear words, vulgar terms, slurs, inflections, and a selection of disguised spellings. It preserves uppercase and title case, and matches whole words rather than fragments inside innocent words or code identifiers. The list is deliberately broad: some entries have innocent meanings too. It is not an exhaustive or security-grade filter.

## Personal replacements

Pass `--replacements /path/to/words.json` to extend or override the defaults. Use `null` to allow a word through. For example, a personal JSON file could contain:

```json
{
  "fuck": "flibbertigibbet",
  "shit": "goose nuggets",
  "cracker": null,
  "sex": null,
  "my-extra-word": "noodle"
}
```

Replacements happen once, not recursively. Custom keys are literal words or phrases, not regular expressions.

## Scan without replacing

Use `--scan` to report matching words and their counts as JSON, without modifying the input file:

```sh
python3 scripts/silly-words.py --scan < notes.txt
python3 scripts/silly-words.py --scan --text 'DAMN damn dammit!'
```

The scan uses the same dictionary and personal overrides as the filter. It finds known entries, not previously unknown profanity. The broad list also includes mild language and context-dependent words, including anatomical terms and words with innocent meanings such as `balls`, `nuts`, `screw`, and `cracker`. Disable any unwanted matches using personal replacements set to `null`.

## Limitations

This is a line-oriented text pipe, not a terminal emulator or interactive command wrapper. Use it for plain-text command output, not interactive programs, binary streams, or full-screen terminal applications. Producers may buffer output when piped. ANSI formatting inserted inside a word, arbitrary spaced-out spellings, and words split across lines are not detected. It does not sanitize terminal control sequences. Copied output contains the silly replacements, but the original input and files stay unchanged. Do not pipe filtered commands back into a shell: changing command text can change its meaning.

## Tests

```sh
python3 -m unittest discover -s scripts -p test_silly_words.py
```