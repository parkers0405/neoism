import { expect, test } from "bun:test";
import type { GpuixRenderer } from "@gpuix/native";
import { focusedNativeElement, focusNativeElement, invalidateNativeFocus } from "../src/native-focus";

function fixture() {
  let now = 1, owner: number | null = 1, reads = 0;
  const renderer = {
    getFocusedElementId() { reads++; return owner; },
    focusElement(id: number) { owner = id; },
  } as unknown as GpuixRenderer;
  return { renderer, clock: () => now, get reads() { return reads; },
    at(time: number) { now = time; }, nativeFocus(id: number | null) { owner = id; } };
}

test("mounted editor and terminal views share one bounded native focus observation", () => {
  const f = fixture();
  for (let time = 1; time < 1001; time += 16) {
    f.at(time);
    for (let view = 0; view < 3; view++) expect(focusedNativeElement(f.renderer, f.clock)).toBe(1);
  }
  expect(f.reads).toBeLessThanOrEqual(20);
});

test("focus commands and native focus/blur events bypass the fallback delay", () => {
  const f = fixture(); expect(focusedNativeElement(f.renderer, f.clock)).toBe(1);
  focusNativeElement(f.renderer, 2);
  expect(focusedNativeElement(f.renderer, f.clock)).toBe(2);
  f.nativeFocus(null); invalidateNativeFocus(f.renderer);
  expect(focusedNativeElement(f.renderer, f.clock)).toBe(null);
  f.nativeFocus(3); invalidateNativeFocus(f.renderer);
  expect(focusedNativeElement(f.renderer, f.clock)).toBe(3);
  expect(f.reads).toBe(4);
});


test("initial focus observation uses the nonblocking snapshot; explicit changes stay ordered", () => {
  let reads = 0, owner: number | null = null, snapshot: number | null = null;
  const renderer = {
    getFocusedElementId() { reads++; return owner; },
    getFocusedElementIdSnapshot() { return snapshot; },
    focusElement(id: number) { owner = id; },
  };
  expect(focusedNativeElement(renderer, () => 1)).toBe(null);
  expect(reads).toBe(0);
  focusNativeElement(renderer, 42);
  expect(focusedNativeElement(renderer, () => 2)).toBe(42);
  expect(reads).toBe(1);
  snapshot = 42;
  expect(focusedNativeElement(renderer, () => 60)).toBe(42);
  expect(reads).toBe(1);
  owner = null; invalidateNativeFocus(renderer);
  expect(focusedNativeElement(renderer, () => 61)).toBe(null);
  expect(reads).toBe(2);
});

test("a focus request before the first observation cannot be lost to an old snapshot", () => {
  let owner = 1;
  const renderer = {getFocusedElementId: () => owner, getFocusedElementIdSnapshot: () => 1, focusElement(id: number) { owner = id; }};
  focusNativeElement(renderer, 2);
  expect(focusedNativeElement(renderer, () => 1)).toBe(2);
});
