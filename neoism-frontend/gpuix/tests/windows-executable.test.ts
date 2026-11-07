import {expect,test} from "bun:test";
import {assertWindowsGuiExecutable,configureWindowsGuiExecutable} from "../scripts/windows-executable";

function image(subsystem:number,magic=0x20b){
 const bytes=new Uint8Array(256),view=new DataView(bytes.buffer);
 view.setUint16(0,0x5a4d,true);view.setUint32(0x3c,128,true);
 view.setUint32(128,0x4550,true);view.setUint16(152,magic,true);view.setUint16(220,subsystem,true);
 return bytes;
}
test("only GUI PE32/PE32+ launchers can be published",()=>{
 for(const magic of [0x10b,0x20b])expect(()=>assertWindowsGuiExecutable(image(2,magic))).not.toThrow();
 expect(()=>assertWindowsGuiExecutable(image(3))).toThrow("subsystem 2");
 expect(()=>assertWindowsGuiExecutable(image(2,0))).toThrow();
 for(const bytes of [new Uint8Array(),image(2).slice(0,220),new Uint8Array(256)])expect(()=>assertWindowsGuiExecutable(bytes)).toThrow();
 const corrupt=image(2);new DataView(corrupt.buffer).setUint32(0x3c,0xffffffff,true);
 expect(()=>assertWindowsGuiExecutable(corrupt)).toThrow();
});

test("GUI configuration preserves the entire image and Bun payload except the subsystem field",()=>{
 for(const magic of [0x10b,0x20b]){
  const bytes=image(3,magic);bytes.set([0xde,0xad,0xbe,0xef],252);
  const before=bytes.slice();configureWindowsGuiExecutable(bytes);
  expect(()=>assertWindowsGuiExecutable(bytes)).not.toThrow();
  before[220]=2;expect(bytes).toEqual(before);
  configureWindowsGuiExecutable(bytes);expect(bytes).toEqual(before);
 }
 // Buffer views need their own byte offset; never mutate the prefix/suffix.
 const wrapped=new Uint8Array(300).fill(0xaa);wrapped.set(image(3),12);
 configureWindowsGuiExecutable(wrapped.subarray(12,268));
 expect(wrapped[12+220]).toBe(2);expect(wrapped[0]).toBe(0xaa);expect(wrapped[299]).toBe(0xaa);
 for(const bytes of [image(7),image(2,0),new Uint8Array(12)]){
  const before=bytes.slice();expect(()=>configureWindowsGuiExecutable(bytes)).toThrow();expect(bytes).toEqual(before);
 }
});