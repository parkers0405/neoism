#!/usr/bin/env python3
"""Scan Neoism agent chats for profanity (dry-run by default).

Reads the local Turso/SQLite agent store and the composer prompt-history
file. Only session titles, message text/reasoning parts, and prompt-history
lines are inspected — tool dumps, diffs, and other JSON fields are ignored.

Usage:
  python3 scripts/scrub-agent-chats.py
  python3 scripts/scrub-agent-chats.py --limit 500
  python3 scripts/scrub-agent-chats.py --db /path/to/agent.turso.db
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sqlite3
import sys
from collections import Counter
from pathlib import Path
from typing import Iterable

# Longest first so "motherfucker" is not reported as "fuck".
REPLACEMENTS: list[tuple[str, str]] = [
    ("motherfucker", "mean-muffin"),
    ("motherfuckers", "mean-muffins"),
    ("motherfucking", "mean-muffin"),
    ("bullshit", "baloney"),
    ("horseshit", "horse-feathers"),
    ("asshole", "goober"),
    ("assholes", "goobers"),
    ("dumbass", "goofball"),
    ("jackass", "goofball"),
    ("smartass", "wiseacre"),
    ("badass", "superstar"),
    ("dipshit", "goofus"),
    ("jackshit", "diddly"),
    ("apeshit", "bananas"),
    ("shithead", "goofus"),
    ("shitheads", "goofuses"),
    ("shitbag", "goofus"),
    ("douchebag", "soap-bar"),
    ("goddamn", "gosh-darn"),
    ("goddamned", "gosh-darned"),
    ("goddammit", "gosh-darn-it"),
    ("goddamnit", "gosh-darn-it"),
    ("bastards", "rascal-cakes"),
    ("bastard", "rascal-cake"),
    ("bitches", "grumps"),
    ("bitching", "grumping"),
    ("bitchy", "grumpy"),
    ("bitch", "grump"),
    ("fucking", "fudging"),
    ("fuckers", "fudgers"),
    ("fucker", "fudger"),
    ("fucked", "fudged"),
    ("fucks", "fudges"),
    ("fuck", "fudge"),
    ("shitty", "stinky"),
    ("shits", "poops"),
    ("shitting", "pooping"),
    ("shit", "poop"),
    ("cunts", "cranky-cabbages"),
    ("cunt", "cranky-cabbage"),
    ("cocksucker", "lollipop-thief"),
    ("cocksuckers", "lollipop-thieves"),
    ("dickhead", "noodle-noggin"),
    ("dickheads", "noodle-noggins"),
    ("dicks", "noodles"),
    ("dick", "noodle"),
    ("pussies", "jellybeans"),
    ("pussy", "jellybean"),
    ("cock", "rooster"),
    ("twat", "pickle"),
    ("twats", "pickles"),
    ("slut", "sassy-pants"),
    ("sluts", "sassy-pantses"),
    ("whore", "drama-llama"),
    ("whores", "drama-llamas"),
    ("douche", "soap"),
    ("pissed", "steamed"),
    ("pissing", "sprinkling"),
    ("piss", "sprinkle"),
    ("crap", "crud"),
    ("crappy", "cruddy"),
    ("damn", "darn"),
    ("damned", "darned"),
    ("dammit", "darn-it"),
    ("hell", "heck"),
    ("ass", "booty"),
    ("wtf", "what-the-fudge"),
    ("stfu", "hush-now"),
    ("omfg", "oh-my-stars"),
    ("lmfao", "lol-so-hard"),
    ("niggers", "banana-breads"),
    ("nigger", "banana-bread"),
    ("niggas", "banana-breads"),
    ("nigga", "banana-bread"),
    ("faggots", "sprockets"),
    ("faggot", "sprocket"),
    ("fags", "sprockets"),
    ("fag", "sprocket"),
    ("retards", "goofuses"),
    ("retard", "goofus"),
    ("retarded", "goofy"),
    ("kikes", "bagels"),
    ("kike", "bagel"),
    ("spics", "sprockets"),
    ("spic", "sprocket"),
    ("chinks", "sprockets"),
    ("chink", "sprocket"),
    ("wetbacks", "sprockets"),
    ("wetback", "sprocket"),
    ("trannies", "sprockets"),
    ("tranny", "sprocket"),
]

REPLACEMENTS.sort(key=lambda item: len(item[0]), reverse=True)
WORD_MAP = {word.lower(): silly for word, silly in REPLACEMENTS}
WORD_RE = re.compile(
    r"\b(?:" + "|".join(re.escape(word) for word, _ in REPLACEMENTS) + r")\b",
    re.IGNORECASE,
)

DEFAULT_DB = Path.home() / ".local/state/neoism/agent.turso.db"
DEFAULT_HISTORY = Path.home() / ".local/share/neoism/agent_prompt_history"
CHAT_PART_TYPES = {"text", "reasoning"}
SNIPPET_RADIUS = 48


def open_db(path: Path) -> sqlite3.Connection:
    uri = path.resolve().as_uri() + "?mode=ro&immutable=1"
    con = sqlite3.connect(uri, uri=True, timeout=5)
    con.row_factory = sqlite3.Row
    return con


def cheap_hit(blob: str) -> bool:
    return bool(WORD_RE.search(blob))


def find_hits(text: str) -> list[re.Match[str]]:
    return list(WORD_RE.finditer(text))


def snippet(text: str, match: re.Match[str]) -> str:
    start = max(0, match.start() - SNIPPET_RADIUS)
    end = min(len(text), match.end() + SNIPPET_RADIUS)
    prefix = "..." if start else ""
    suffix = "..." if end < len(text) else ""
    chunk = text[start:end].replace("\n", "\\n")
    return f"{prefix}{chunk}{suffix}"


def iter_message_texts(payload: object) -> Iterable[tuple[str, str]]:
    if not isinstance(payload, dict):
        return
    parts = payload.get("parts")
    if not isinstance(parts, list):
        return
    for part in parts:
        if not isinstance(part, dict):
            continue
        if part.get("type") not in CHAT_PART_TYPES:
            continue
        text = part.get("text")
        if isinstance(text, str) and text:
            yield str(part.get("type")), text


def record_hits(
    *,
    source: str,
    session_id: str,
    extra: str,
    text: str,
    term_counts: Counter[str],
    session_counts: Counter[str],
    samples: list[dict[str, str]],
    sample_limit: int,
) -> int:
    matches = find_hits(text)
    if not matches:
        return 0
    session_counts[session_id] += len(matches)
    for match in matches:
        term = match.group(0).lower()
        term_counts[term] += 1
        if len(samples) < sample_limit:
            samples.append(
                {
                    "source": source,
                    "session_id": session_id,
                    "extra": extra,
                    "term": term,
                    "replacement": WORD_MAP.get(term, "?"),
                    "snippet": snippet(text, match),
                }
            )
    return len(matches)


def scan_sessions(con: sqlite3.Connection, args: argparse.Namespace) -> tuple[int, int]:
    cur = con.execute("SELECT id, info_json FROM sessions")
    scanned = 0
    hits = 0
    for row in cur:
        scanned += 1
        blob = row["info_json"] or ""
        if not cheap_hit(blob):
            continue
        try:
            info = json.loads(blob)
        except json.JSONDecodeError:
            continue
        title = info.get("title") if isinstance(info, dict) else None
        if not isinstance(title, str):
            continue
        hits += record_hits(
            source="session.title",
            session_id=row["id"],
            extra=title[:80],
            text=title,
            term_counts=args._term_counts,
            session_counts=args._session_counts,
            samples=args._samples,
            sample_limit=args.samples,
        )
    return scanned, hits


def scan_messages(con: sqlite3.Connection, args: argparse.Namespace) -> tuple[int, int, int]:
    sql = "SELECT id, session_id, message_json FROM messages"
    if args.limit:
        sql += f" LIMIT {int(args.limit)}"
    cur = con.execute(sql)
    scanned = 0
    parsed = 0
    hits = 0
    try:
        for row in cur:
            scanned += 1
            if scanned % 5000 == 0:
                print(f"  messages scanned {scanned:,} (hits so far {hits:,})", file=sys.stderr)
            blob = row["message_json"] or ""
            if not cheap_hit(blob):
                continue
            try:
                payload = json.loads(blob)
            except json.JSONDecodeError:
                continue
            parsed += 1
            for part_type, text in iter_message_texts(payload):
                hits += record_hits(
                    source=f"message.{part_type}",
                    session_id=row["session_id"],
                    extra=row["id"],
                    text=text,
                    term_counts=args._term_counts,
                    session_counts=args._session_counts,
                    samples=args._samples,
                    sample_limit=args.samples,
                )
    except sqlite3.DatabaseError as error:
        print(f"  stopped early: {error} after {scanned:,} messages", file=sys.stderr)
    return scanned, parsed, hits


def scan_prompt_history(path: Path, args: argparse.Namespace) -> tuple[int, int]:
    if not path.is_file():
        return 0, 0
    scanned = 0
    hits = 0
    for line_no, raw in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
        line = raw.strip()
        if not line:
            continue
        scanned += 1
        try:
            text = json.loads(line)
            if not isinstance(text, str):
                text = line
        except json.JSONDecodeError:
            text = line
        hits += record_hits(
            source="prompt_history",
            session_id="-",
            extra=f"line {line_no}",
            text=text,
            term_counts=args._term_counts,
            session_counts=args._session_counts,
            samples=args._samples,
            sample_limit=args.samples,
        )
    return scanned, hits


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, default=Path(os.environ.get("NEOISM_AGENT_STATE_DIR", DEFAULT_DB.parent)) / "agent.turso.db" if "NEOISM_AGENT_STATE_DIR" in os.environ else DEFAULT_DB)
    parser.add_argument("--history", type=Path, default=Path(os.environ.get("NEOISM_AGENT_PROMPT_HISTORY_FILE", DEFAULT_HISTORY)))
    parser.add_argument("--limit", type=int, default=0, help="scan only N messages (0 = all)")
    parser.add_argument("--samples", type=int, default=40, help="max snippets to print")
    parser.add_argument("--top-sessions", type=int, default=20)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    args._term_counts = Counter()
    args._session_counts = Counter()
    args._samples = []

    db = args.db
    if not db.is_file():
        print(f"no agent db at {db}", file=sys.stderr)
        return 1

    print(f"dry-run scan of {db} ({db.stat().st_size / (1024**3):.1f} GiB)")
    print("scope: session titles + message text/reasoning + prompt history")
    print("skipped: tool JSON, events, embeddings")

    hist_rows, hist_hits = scan_prompt_history(args.history, args)
    print(f"prompt history: {hist_rows:,} lines, {hist_hits:,} hits ({args.history})")

    con = open_db(db)
    try:
        session_rows, title_hits = scan_sessions(con, args)
        print(f"sessions: {session_rows:,} rows, {title_hits:,} title hits")
        msg_rows, parsed, msg_hits = scan_messages(con, args)
        print(f"messages: {msg_rows:,} rows, {parsed:,} JSON parsed after prefilter, {msg_hits:,} text hits")
    finally:
        con.close()

    print("\nterm counts:")
    if not args._term_counts:
        print("  (none)")
    else:
        for term, count in args._term_counts.most_common():
            print(f"  {term:20} {count:6}  -> {WORD_MAP.get(term, '?')}")

    print(f"\ntop {args.top_sessions} sessions by hit count:")
    ranked = [(sid, n) for sid, n in args._session_counts.most_common(args.top_sessions) if sid != "-"]
    if not ranked:
        print("  (none)")
    else:
        for sid, count in ranked:
            print(f"  {sid}  {count}")

    print(f"\nsamples ({len(args._samples)}):")
    for sample in args._samples:
        print(
            f"  [{sample['source']}] {sample['session_id']} {sample['extra']}\n"
            f"    {sample['term']} -> {sample['replacement']}\n"
            f"    {sample['snippet']}"
        )

    print(
        f"\nTOTAL hits: {sum(args._term_counts.values()):,}  "
        f"sessions touched: {sum(1 for sid in args._session_counts if sid != '-'):,}"
    )
    print("no writes performed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
