#!/usr/bin/env python3
"""Scan (default) or rewrite Neoism agent chats that contain cuss words.

Only session titles, message text/reasoning parts, and the composer
prompt-history file are touched. Tool dumps, diffs, events, and embeddings
are left alone.

Usage:
  python3 scripts/scrub-agent-chats.py
  python3 scripts/scrub-agent-chats.py --apply
  python3 scripts/scrub-agent-chats.py --apply --skip-backup
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sqlite3
import sys
import time
from collections import Counter
from datetime import datetime
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

# Leave these whole phrases alone even when they contain a mapped word.
KEEP_PHRASES = [
    "piss-desktop",
]

REPLACEMENTS.sort(key=lambda item: len(item[0]), reverse=True)
WORD_MAP = {word.lower(): silly for word, silly in REPLACEMENTS}
WORD_RE = re.compile(
    r"\b(?:" + "|".join(re.escape(word) for word, _ in REPLACEMENTS) + r")\b",
    re.IGNORECASE,
)
KEEP_RE = re.compile(
    "|".join(re.escape(phrase) for phrase in KEEP_PHRASES),
    re.IGNORECASE,
)

DEFAULT_DB = Path.home() / ".local/state/neoism/agent.turso.db"
DEFAULT_HISTORY = Path.home() / ".local/share/neoism/agent_prompt_history"
CHAT_PART_TYPES = {"text", "reasoning"}
SNIPPET_RADIUS = 48


def keep_spans(text: str) -> list[tuple[int, int]]:
    return [(match.start(), match.end()) for match in KEEP_RE.finditer(text)]


def overlaps(span: tuple[int, int], keeps: list[tuple[int, int]]) -> bool:
    start, end = span
    return any(start < keep_end and end > keep_start for keep_start, keep_end in keeps)


def find_hits(text: str) -> list[re.Match[str]]:
    keeps = keep_spans(text)
    return [
        match
        for match in WORD_RE.finditer(text)
        if not overlaps((match.start(), match.end()), keeps)
    ]


def replace_text(text: str) -> tuple[str, int]:
    keeps = keep_spans(text)
    count = 0

    def sub(match: re.Match[str]) -> str:
        nonlocal count
        if overlaps((match.start(), match.end()), keeps):
            return match.group(0)
        count += 1
        return WORD_MAP.get(match.group(0).lower(), match.group(0))

    return WORD_RE.sub(sub, text), count


def cheap_hit(blob: str) -> bool:
    return bool(WORD_RE.search(blob))


def snippet(text: str, match: re.Match[str]) -> str:
    start = max(0, match.start() - SNIPPET_RADIUS)
    end = min(len(text), match.end() + SNIPPET_RADIUS)
    prefix = "..." if start else ""
    suffix = "..." if end < len(text) else ""
    chunk = text[start:end].replace("\n", "\\n")
    return f"{prefix}{chunk}{suffix}"


def iter_message_texts(payload: object) -> Iterable[tuple[int, str, str]]:
    if not isinstance(payload, dict):
        return
    parts = payload.get("parts")
    if not isinstance(parts, list):
        return
    for index, part in enumerate(parts):
        if not isinstance(part, dict):
            continue
        if part.get("type") not in CHAT_PART_TYPES:
            continue
        text = part.get("text")
        if isinstance(text, str) and text:
            yield index, str(part.get("type")), text


def dump_json(payload: object) -> str:
    return json.dumps(payload, ensure_ascii=False, separators=(",", ":"))


def open_db(path: Path, *, write: bool) -> sqlite3.Connection:
    if write:
        con = sqlite3.connect(str(path), timeout=30)
    else:
        uri = path.resolve().as_uri() + "?mode=ro&immutable=1"
        con = sqlite3.connect(uri, uri=True, timeout=5)
    con.row_factory = sqlite3.Row
    con.execute("PRAGMA busy_timeout=30000")
    return con


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


def process_sessions(
    con: sqlite3.Connection, args: argparse.Namespace, *, write: bool
) -> tuple[int, int, int]:
    cur = con.execute("SELECT id, info_json FROM sessions")
    scanned = 0
    hits = 0
    rows_written = 0
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
        if write:
            new_title, count = replace_text(title)
            if count:
                info["title"] = new_title
                con.execute(
                    "UPDATE sessions SET info_json = ? WHERE id = ?",
                    (dump_json(info), row["id"]),
                )
                rows_written += 1
    return scanned, hits, rows_written


def process_messages(
    con: sqlite3.Connection, args: argparse.Namespace, *, write: bool
) -> tuple[int, int, int, int]:
    sql = "SELECT id, session_id, message_json FROM messages"
    if args.limit:
        sql += f" LIMIT {int(args.limit)}"
    cur = con.execute(sql)
    scanned = 0
    parsed = 0
    hits = 0
    rows_written = 0
    try:
        for row in cur:
            scanned += 1
            if scanned % 5000 == 0:
                print(
                    f"  messages scanned {scanned:,} (hits so far {hits:,})",
                    file=sys.stderr,
                )
            blob = row["message_json"] or ""
            if not cheap_hit(blob):
                continue
            try:
                payload = json.loads(blob)
            except json.JSONDecodeError:
                continue
            parsed += 1
            changed = False
            for index, part_type, text in iter_message_texts(payload):
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
                if write:
                    new_text, count = replace_text(text)
                    if count:
                        payload["parts"][index]["text"] = new_text
                        changed = True
            if write and changed:
                con.execute(
                    "UPDATE messages SET message_json = ? WHERE session_id = ? AND id = ?",
                    (dump_json(payload), row["session_id"], row["id"]),
                )
                rows_written += 1
    except sqlite3.DatabaseError as error:
        print(f"  stopped early: {error} after {scanned:,} messages", file=sys.stderr)
    return scanned, parsed, hits, rows_written


def process_prompt_history(
    path: Path, args: argparse.Namespace, *, write: bool
) -> tuple[int, int, int]:
    if not path.is_file():
        return 0, 0, 0
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    scanned = 0
    hits = 0
    lines_written = 0
    out_lines: list[str] = []
    for line_no, raw in enumerate(lines, 1):
        line = raw.strip()
        if not line:
            out_lines.append(raw)
            continue
        scanned += 1
        encoded = True
        try:
            text = json.loads(line)
            if not isinstance(text, str):
                text = line
                encoded = False
        except json.JSONDecodeError:
            text = line
            encoded = False
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
        new_text, count = replace_text(text)
        if write and count:
            lines_written += 1
            out_lines.append(json.dumps(new_text, ensure_ascii=False) if encoded else new_text)
        else:
            out_lines.append(raw if not write else (json.dumps(text, ensure_ascii=False) if encoded else text))
    if write and lines_written:
        path.write_text("\n".join(out_lines) + "\n", encoding="utf-8")
    return scanned, hits, lines_written


def backup_db(path: Path) -> Path:
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    dest = path.with_name(f"{path.name}.scrub-{stamp}.bak")
    print(f"copying {path} -> {dest} ({path.stat().st_size / (1024**3):.1f} GiB)")
    started = time.monotonic()
    shutil.copy2(path, dest)
    wal = Path(str(path) + "-wal")
    shm = Path(str(path) + "-shm")
    if wal.is_file():
        shutil.copy2(wal, Path(str(dest) + "-wal"))
    if shm.is_file():
        shutil.copy2(shm, Path(str(dest) + "-shm"))
    print(f"backup finished in {time.monotonic() - started:.1f}s")
    return dest


def parse_args() -> argparse.Namespace:
    default_db = DEFAULT_DB
    if "NEOISM_AGENT_STATE_DIR" in os.environ:
        default_db = Path(os.environ["NEOISM_AGENT_STATE_DIR"]) / "agent.turso.db"
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, default=default_db)
    parser.add_argument(
        "--history",
        type=Path,
        default=Path(os.environ.get("NEOISM_AGENT_PROMPT_HISTORY_FILE", DEFAULT_HISTORY)),
    )
    parser.add_argument("--limit", type=int, default=0, help="scan only N messages (0 = all)")
    parser.add_argument("--samples", type=int, default=40, help="max snippets to print")
    parser.add_argument("--top-sessions", type=int, default=20)
    parser.add_argument("--apply", action="store_true", help="write replacements")
    parser.add_argument(
        "--skip-backup",
        action="store_true",
        help="do not copy the 14GiB DB before --apply",
    )
    return parser.parse_args()


def print_report(args: argparse.Namespace) -> None:
    print("\nterm counts:")
    if not args._term_counts:
        print("  (none)")
    else:
        for term, count in args._term_counts.most_common():
            print(f"  {term:20} {count:6}  -> {WORD_MAP.get(term, '?')}")

    print(f"\ntop {args.top_sessions} sessions by hit count:")
    ranked = [
        (sid, n)
        for sid, n in args._session_counts.most_common(args.top_sessions)
        if sid != "-"
    ]
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


def main() -> int:
    args = parse_args()
    args._term_counts = Counter()
    args._session_counts = Counter()
    args._samples = []
    write = args.apply
    started = time.monotonic()

    db = args.db
    if not db.is_file():
        print(f"no agent db at {db}", file=sys.stderr)
        return 1

    mode = "APPLY" if write else "dry-run"
    print(f"{mode} scan of {db} ({db.stat().st_size / (1024**3):.1f} GiB)")
    print("scope: session titles + message text/reasoning + prompt history")
    print("skipped: tool JSON, events, embeddings")
    print("kept: " + ", ".join(KEEP_PHRASES))

    if write and not args.skip_backup:
        backup_db(db)

    hist_rows, hist_hits, hist_written = process_prompt_history(
        args.history, args, write=write
    )
    print(
        f"prompt history: {hist_rows:,} lines, {hist_hits:,} hits, "
        f"{hist_written:,} lines rewritten ({args.history})"
    )

    try:
        con = open_db(db, write=write)
    except sqlite3.OperationalError as error:
        print(f"cannot open db ({error}). Quit Neoism first.", file=sys.stderr)
        return 1
    try:
        if write:
            con.execute("BEGIN")
        session_rows, title_hits, titles_written = process_sessions(
            con, args, write=write
        )
        print(
            f"sessions: {session_rows:,} rows, {title_hits:,} title hits, "
            f"{titles_written:,} rows rewritten"
        )
        msg_rows, parsed, msg_hits, msgs_written = process_messages(
            con, args, write=write
        )
        print(
            f"messages: {msg_rows:,} rows, {parsed:,} JSON parsed after prefilter, "
            f"{msg_hits:,} text hits, {msgs_written:,} rows rewritten"
        )
        if write:
            con.commit()
    except sqlite3.OperationalError as error:
        print(f"db write failed ({error}). Quit Neoism first.", file=sys.stderr)
        return 1
    finally:
        con.close()

    print_report(args)
    elapsed = time.monotonic() - started
    if write:
        print(f"writes committed in {elapsed:.1f}s")
    else:
        print(f"no writes performed ({elapsed:.1f}s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
