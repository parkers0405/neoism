import {expect,test} from "bun:test";
import {assertWindowsGuiExecutable} from "../scripts/windows-executable";

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