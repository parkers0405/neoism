import { expect, test } from "bun:test";
import { commandBlockHeader, terminalDirectoryLabel, terminalDurationLabel } from "../src/terminal/presentation";

test("block context keeps the directory separate from its right-side duration", () => {
  expect(commandBlockHeader({ directory: "/home/user", duration: 25 }, "/home/user")).toBe("~");
  expect(commandBlockHeader({ directory: "/home/user/project", duration: 1253 }, "/home/user")).toBe("~/project");
  expect(commandBlockHeader({ directory: "/work", duration: 53 }, "/home/user")).toBe("/work");
  expect(commandBlockHeader({ directory: "", duration: 0 })).toBe("Terminal");
  expect(commandBlockHeader({ directory: "/work", duration: 5, background: true })).toBe("Shell output");
  expect(terminalDurationLabel(25)).toBe("25 ms");
  expect(terminalDurationLabel(1253)).toBe("1.3 s");
  expect(terminalDurationLabel(0)).toBe("0 ms");
});
test("home abbreviation respects path boundaries and the daemon's platform", () => {
  expect(terminalDirectoryLabel("/home/username/work", "/home/user")).toBe("/home/username/work");
  expect(terminalDirectoryLabel("/home/User/work", "/home/user")).toBe("/home/User/work");
  expect(terminalDirectoryLabel("/home/user/work", "/home/user/")).toBe("~/work");
  expect(terminalDirectoryLabel("c:\\Users\\Person\\project", "C:\\Users\\person")).toBe("~/project");
  expect(terminalDirectoryLabel("/", "/")).toBe("~");
});