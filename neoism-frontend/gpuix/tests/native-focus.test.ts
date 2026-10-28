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
