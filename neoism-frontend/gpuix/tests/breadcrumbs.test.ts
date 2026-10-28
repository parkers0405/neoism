import {afterEach,expect,test} from "bun:test";
import {NativeEditor,coreCall} from "../src/native-core";
import type {BreadcrumbFrame} from "../src/editor/BreadcrumbsView";
const cleanups:Array<()=>void>=[];afterEach(()=>cleanups.splice(0).reverse().forEach(close=>close()));
function header(path:string,source="",vim=true){
 const editor=new NativeEditor(path,source,vim),{id}=coreCall<{id:number}>("breadcrumbs.new",{fontFamily:"Cascadia Code NF",fontWeight:400});
 cleanups.push(()=>{coreCall("breadcrumbs.dispose",{id});editor.dispose();});
 return {editor,frame:(args:Record<string,unknown>={})=>coreCall<BreadcrumbFrame>("breadcrumbs.frame",{id,editorId:editor.id,width:1000,scale:1,theme:"pastel_dark",...args})};
}
const labels=(frame:BreadcrumbFrame)=>frame.commands.flatMap(command=>command.kind==="text"?[command.text]:[]);
test("original code breadcrumbs use root-relative path components, file icons and exact reserved height",()=>{
 const host=header("/project/src/network/client.rs"),frame=host.frame({root:"/project"});
 expect(frame.segments).toEqual(["src","network","client.rs"]);expect(frame.height).toBe(26);
 expect(labels(frame).filter(label=>["src","network","client.rs","›"].includes(label))).toEqual(["src","›","network","›","client.rs"]);
 expect(frame.commands.some(command=>command.kind==="rect"&&command.rect[1]===25&&command.rect[3]===1)).toBe(true);
 expect(host.frame({root:"/project",scale:1.5}).height).toBe(39);
 // Path::strip_prefix is component-aware: a similarly named project is not
 // silently removed, and remote/editor path overrides use the actual root.
 expect(host.frame({root:"/proj"}).segments).toEqual(["/","project","src","network","client.rs"]);
 expect(host.frame({path:"/remote/work/sub/file.ts",root:"/remote/work"}).segments).toEqual(["sub","file.ts"]);
 expect(labels(host.frame({root:"/project",width:40}))).not.toContain("client.rs");
});
test("empty unnamed document has no ghost breadcrumb reservation",()=>{
 const host=header(""),frame=host.frame();expect(frame.height).toBe(0);expect(frame.segments).toEqual([]);expect(frame.commands).toEqual([]);
});
test("actual tree-sitter symbol tail is debounced, waits for motion and follows unsaved native source",async()=>{
 const source="mod net {\n  impl Client {\n    fn connect(&self) {\n      let value = 1;\n    }\n  }\n}\n";
 const host=header("/project/client.rs",source);host.editor.call("command",{command:"4"});
 const initial=host.frame({root:"/project"});expect(initial.tail).toBe("");expect(initial.nextTickMs).toBeGreaterThan(0);
 await Bun.sleep(165);expect(host.frame({root:"/project",scrollAnimating:true}).tail).toBe("");
 const settled=host.frame({root:"/project"});expect(settled.tail).toBe("net › Client › connect");expect(labels(settled)).toContain("net › Client › connect");expect(settled.nextTickMs).toBeNull();
 host.editor.call("replace-source",{text:source.replace("connect","renamed")});host.editor.call("command",{command:"4"});host.frame({root:"/project"});
 await Bun.sleep(165);expect(host.frame({root:"/project"}).tail).toBe("net › Client › renamed");
});
