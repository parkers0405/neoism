import { expect, test } from "bun:test";
import { ShellEvents, type ShellEvent } from "../src/terminal/shell-events";

const encode = (text: string) => new TextEncoder().encode(text);
test("shell lifecycle and UTF-8 directory markers survive every packet boundary and both terminators", () => {
  const bytes = encode("\x1b]7;file://localhost/home/a%20b/notes\x1b\\\x1b]133;A\x07\x1b]133;B\x1b\\echo hello\r\n\x1b]133;C\x07hello\r\n\x1b]133;D;17\x1b\\");
  for (let split = 0; split <= bytes.length; split++) {
    const observer = new ShellEvents(), events: ShellEvent[] = [];
    observer.feed(bytes.subarray(0, split), (_, event) => events.push(event));
    observer.feed(bytes.subarray(split), (_, event) => events.push(event));
    expect(events).toEqual([{ kind: "directory", path: "/home/a b/notes" }, { kind: "prompt" }, { kind: "input" }, { kind: "start" }, { kind: "end", status: 17 }]);
  }
});
test("metadata observer bounds strings, ignores nested control payloads, and resumes after cancellation", () => {
  const observer = new ShellEvents(), events: ShellEvent[] = [];
  observer.feed(encode("\x1bP\x1b]133;D;99\x07\x1b\\\x1b]" + "x".repeat(9000) + "\x07\x1b]133;C\x18\x1b]7;file://bad/%ZZ\x07\x1b]633;B\x07"), (_, event) => events.push(event));
  expect(events).toEqual([{ kind: "input" }]);
  observer.feed(encode("\x1b]133;"), () => {}); observer.reset();
  observer.feed(encode("C\x07\x1b]133;D\x07"), (_, event) => events.push(event));
  expect(events).toEqual([{ kind: "input" }, { kind: "end" }]);
});
test("event positions let native VT feed reach each boundary before metadata capture", () => {
  const observer = new ShellEvents(), bytes = encode("before\x1b]133;C\x07output\x1b]133;D;0\x07after");
  const segments: string[] = []; let start = 0;
  observer.feed(bytes, (end) => { segments.push(new TextDecoder().decode(bytes.subarray(start, end))); start = end; });
  segments.push(new TextDecoder().decode(bytes.subarray(start)));
  expect(segments).toEqual(["before\x1b]133;C\x07", "output\x1b]133;D;0\x07", "after"]);
});

test("bounded clear observation survives all splits without interpreting application CSI", () => {
  const bytes = encode("\x1b[H\x1b[2J\x1b[3J\x1bc\x1b[?2J\x1b[0J\x1b[2;3J\x1b[" + "2".repeat(100) + "J\x1b[2\x18J\x1bP\x1b[3J\x1b\\\x1b]133;B\x07");
  for (let split = 0; split <= bytes.length; split++) {
    const observer = new ShellEvents(), events: ShellEvent[] = [];
    observer.feed(bytes.subarray(0, split), (_, event) => events.push(event));
    observer.feed(bytes.subarray(split), (_, event) => events.push(event));
    expect(events).toEqual([{ kind: "clear", mode: "screen" }, { kind: "clear", mode: "history" }, { kind: "clear", mode: "reset" }, { kind: "input" }]);
  }
});