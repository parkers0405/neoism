import {expect,test} from "bun:test";
import {HomeTerminalShortcut} from "../src/commands/home-terminal-shortcut";

test("Home Space H opens a terminal only while the plain leader is armed",()=>{
 const shortcut=new HomeTerminalShortcut();
 expect(shortcut.key({key:"h"},0)).toBeUndefined();
 expect(shortcut.key({key:"space"},10)).toBe("pending");
 expect(shortcut.key({key:"h"},100)).toBe("split-terminal");
 expect(shortcut.key({key:"h"},101)).toBeUndefined();
 for(const cancel of [{key:"escape"},{key:"j"},{key:"h",modifiers:{ctrl:true}}]){
  shortcut.key({key:"space"},200);shortcut.key(cancel,210);
  expect(shortcut.key({key:"h"},220)).toBeUndefined();
 }
 shortcut.key({key:" "},300);expect(shortcut.key({key:"h"},1101)).toBeUndefined();
 shortcut.key({key:"space"},1200);shortcut.reset();expect(shortcut.key({key:"h"},1201)).toBeUndefined();
});
