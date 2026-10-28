import { expect, test } from "bun:test";
import { coreCall } from "../src/native-core";
type Frame = { top: number; offset: number; selected: number; animating: boolean };
const entries = Array.from({ length: 100 }, (_, n) => ({ name: `${n}.txt`, path: `${n}.txt`, dir: false, depth: 0 }));
function tree() {
  const { id } = coreCall<{ id: number }>("tree.new", { scale: 1 });
  const call = (op: string, args: Record<string, unknown> = {}) => coreCall<Frame>(`tree.${op}`, { id, height: 260, ...args });
  call("layout", { entries });
  return { call, dispose: () => coreCall("dispose", { id }) };
}
test("sidebar Vim counts, gg/G, and half-page motions use FileTree selection", () => {
  const { call, dispose } = tree();
  try {
    expect(call("key", { key: "j" }).selected).toBe(1);
    call("key", { key: "3" }); expect(call("key", { key: "j" }).selected).toBe(4);
    call("key", { key: "g" }); expect(call("key", { key: "g" }).selected).toBe(0);
    expect(call("key", { key: "G" }).selected).toBe(99);
    expect(call("key", { key: "u", ctrl: true }).selected).toBe(95);
    call("key", { key: "5" }); expect(call("key", { key: "G" }).selected).toBe(4);
  } finally { dispose(); }
});
test("sidebar wheel preserves row lag and clamps at the bottom", () => {
  const { call, dispose } = tree();
  try {
    const frame = call("scroll", { delta: -80 });
    expect(frame.top).toBe(3); expect(frame.offset).toBeGreaterThan(0); expect(frame.offset).toBeLessThanOrEqual(78.001); // f32 spring sampling can round a few ulps above 3 rows.
    expect(frame.animating).toBe(true);
    expect(call("scroll", { delta: -100000 }).top).toBe(91);
  } finally { dispose(); }
});
test("host relists retain an offscreen wheel viewport while keyboard selection still reveals", () => {
  const { call, dispose } = tree();
  try {
    const before = call("scroll", {delta:-1000});
    expect(before.top).toBeGreaterThan(20); expect(before.selected).toBe(0);
    expect(call("layout", {entries}).top).toBe(before.top);
    expect(call("layout", {entries:[...entries,{name:"new.txt",path:"new.txt",dir:false,depth:0}]}).top).toBe(before.top);
    expect(call("key", {key:"home"}).top).toBe(0);
    call("scroll", {delta:-1000});
    expect(call("layout", {entries:entries.slice(0,12)}).top).toBe(3);
    expect(call("layout", {entries:[]}).top).toBe(0);
  } finally { dispose(); }
});
