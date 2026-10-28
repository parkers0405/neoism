import {expect,test} from "bun:test";
import {coreCall,NativeEditor} from "../src/native-core";
import type {GitReviewFrame} from "../src/git/GitReviewController";
const root="/fixture/repo",path="src/main.ts";
const patch="diff --git a/src/main.ts b/src/main.ts\n--- a/src/main.ts\n+++ b/src/main.ts\n@@ -1,2 +1,2 @@\n-const value = 1;\n+const value = 2;\n console.log(value);\n";
const file={path,status:"Modified",additions:1,deletions:1,staged:false};
function review(extra:Record<string,unknown>={}){const{id}=coreCall<{id:number}>("gitreview.new",{root,path,mode:"working",fontFamily:"Cascadia Code NF",fontWeight:400,fontSize:14,theme:"pastel_dark",...extra});return{call:(op:string,args:Record<string,unknown>={})=>coreCall<GitReviewFrame>(`gitreview.${op}`,{id,...args}),dispose:()=>coreCall("gitreview.dispose",{id})};}
function load(r:ReturnType<typeof review>){r.call("reply",{message:{ChangedFiles:{files:[file,{...file,path:"other.ts"}],branch:"feature",upstream:null,error:null}}});return r.call("reply",{message:{FileDiffs:{diffs:[{path,patch},{path:"other.ts",patch:patch.replaceAll("src/main.ts","other.ts")}]}}});}
const center=(rect:number[])=>({x:rect[0]+rect[2]/2,y:rect[1]+rect[3]/2});
test("canonical repository review paints real patches and retains original split/fold/navigation and root-scoped controls",()=>{
 const r=review();try{
  let frame=load(r);expect(frame.files.map(file=>[file.path,file.loaded])).toEqual([[path,true],["other.ts",true]]);expect(frame.commands.some(command=>command.kind==="text"&&command.text.includes("value"))).toBe(true);expect(frame.effectiveSplit).toBe(true);
  const mode=frame.toolbar.find(hit=>hit.id==="Unified")!;expect(mode).toBeDefined();frame=r.call("pointer",{kind:"down",...center(mode.rect)});expect(frame.split).toBe(false);expect(frame.effectiveSplit).toBe(false);expect(frame.editHits.length).toBeGreaterThan(0);
  frame=r.call("key",{key:"tab"});expect(frame.split).toBe(true);
  const header=frame.headers.find(hit=>hit.path===path)!;frame=r.call("pointer",{kind:"down",x:header.rect[0]+header.rect[2]/2,y:header.rect[1]+header.rect[3]/2});expect(frame.files.find(file=>file.path===path)?.collapsed).toBe(true);
  frame=r.call("reveal",{path});expect(frame.files.find(file=>file.path===path)?.collapsed).toBe(false);frame=r.call("navigate-file",{next:true});expect(frame.path).toBe("other.ts");
  frame=r.call("reveal",{path});const stage=frame.toolbar.find(hit=>hit.id==="Stage")!;expect(stage.enabled).toBe(true);frame=r.call("pointer",{kind:"down",...center(stage.rect)});expect(frame.actions).toEqual([{kind:"stage",root,path,unstage:false}]);
  // Original file headers deliberately have no mutation controls. Restore is
  // attached to the exact contiguous change run revealed by hovering its body.
  expect(frame.fileActions).toEqual([]);expect(frame.commands.some(command=>command.kind==="text"&&command.text==="Restore")).toBe(false);
  const changedRow=frame.editHits.find(hit=>hit.path===path&&hit.line===0)!;
  frame=r.call("pointer",{kind:"move",...center(changedRow.rect)});
  const restore=frame.commands.find(command=>command.kind==="text"&&command.text==="Restore");expect(restore?.kind).toBe("text");if(restore?.kind!=="text")throw new Error("Original hovered change-run Restore was not painted");
  frame=r.call("pointer",{kind:"down",x:restore.x+restore.width/2,y:restore.y+restore.size/2});
  expect(frame.actions).toEqual([{kind:"block",root,path,restore:true,block:{old_start:0,new_start:0,old_lines:["const value = 1;"],new_lines:["const value = 2;"]}}]);
  const narrow=r.call("layout",{width:340,height:480});expect(narrow.effectiveSplit).toBe(false);expect(narrow.headers.every(hit=>hit.rect[0]>=0&&hit.rect[0]+hit.rect[2]<=340.1)).toBe(true);
 }finally{r.dispose();}
});
test("review working column attaches exact canonical document and edits live bytes without changing read-only baseline",async()=>{
 const r=review(),editor=new NativeEditor(root+"/"+path,"const value = 2;\nconsole.log(value);\n",true);try{
  let frame=load(r);r.call("reply",{message:{BaseFile:{path,text:"const value = 1;\nconsole.log(value);\n"}}});const hit=frame.editHits.find(hit=>hit.path===path)!;expect(hit).toBeDefined();frame=r.call("pointer",{kind:"down",...center(hit.rect)});expect(frame.edit).toMatchObject({root,path});
  frame=r.call("attach",{editorID:editor.id,line:frame.edit!.line,column:0});expect(frame.editingPath).toBe(root+"/"+path);expect(frame.cursorQuad?.length).toBe(4);
  r.call("key",{key:"i",char:"i"});r.call("key",{key:"/",char:"/"});r.call("key",{key:"escape"});expect(editor.text().startsWith("/const")).toBe(true);
  // Reconstruct actual painted rows by geometry. Syntax spans and clip passes
  // intentionally split or repeat text; they are not whole-line text nodes.
  const paintedRows=(frame:GitReviewFrame,left:number,right:number)=>{
   const rows=new Map<number,Map<number,string>>();
   for(const command of frame.commands){if(command.kind!=="text"||command.x<left||command.x>=right)continue;const y=Math.round(command.y*10),x=Math.round(command.x*10);let row=rows.get(y);if(!row)rows.set(y,row=new Map());if((row.get(x)?.length||0)<command.text.length)row.set(x,command.text);}
   return [...rows.values()].map(row=>[...row].sort(([a],[b])=>a-b).map(([,text])=>text).join(""));
  };
  const workingLeft=hit.rect[0],workingRight=hit.rect[0]+hit.rect[2],deadline=Date.now()+3000;
  do{frame=r.call("frame");if(paintedRows(frame,workingLeft,workingRight).includes("/const value = 2;")&&!frame.animating)break;await Bun.sleep(10);}while(Date.now()<deadline);
  expect(frame.dirty).toBe(true);expect(paintedRows(frame,workingLeft,workingRight)).toContain("/const value = 2;");
  expect(paintedRows(frame,0,workingLeft).some(row=>row.endsWith("const value = 1;"))).toBe(true);
  const wrong=new NativeEditor("/elsewhere/main.ts","x",true);try{expect(()=>r.call("attach",{editorID:wrong.id})).toThrow("another review source");}finally{wrong.dispose();}
  r.call("navigate-file",{next:true});expect(r.call("frame").editingPath).toBeNull();expect(editor.text().startsWith("/const")).toBe(true);
 }finally{r.dispose();editor.dispose();}
});
test("historical commit remains immutable with original metadata/copy while PR inbox and create form emit original daemon intents",()=>{
 const detail={sha:"abc123def",author:"Ada",email:"a@example.test",timestamp:0,offset_minutes:330,message:"subject\n\nUnicode αβ body",files:[file],diffs:[{path,patch}]};const commit=review({mode:"commit",detail});try{
  let frame=commit.call("frame");expect(frame.readOnly).toBe(true);expect(frame.editHits).toEqual([]);expect(frame.toolbar.some(hit=>["Stage","StageAll","Commit"].includes(hit.id))).toBe(false);expect(frame.commands.some(command=>command.kind==="text"&&command.text.includes("+05:30"))).toBe(true);
  frame=commit.call("key",{key:"i",char:"i"});expect(frame.actions).toEqual([]);expect(frame.edit).toBeNull();
 }finally{commit.dispose();}
 const inbox=review({mode:"pull-request-inbox"});try{const frame=inbox.call("frame");expect(frame.actions.some(action=>action.kind==="pull-request"&&"List"in(action.request as object))).toBe(true);}finally{inbox.dispose();}
 const create=review({mode:"pull-request-create",detail:{branch:"feature",head:"abc123",defaultBranch:"main"}});try{
  create.call("text",{text:"My new pull request"});const frame=create.call("key",{key:"enter",ctrl:true});expect(frame.actions).toContainEqual({kind:"pull-request",root,request:{Create:{title:"My new pull request",body:"",base:"main",draft:false,expected_branch:"feature",expected_head:"abc123"}}});
 }finally{create.dispose();}
});
