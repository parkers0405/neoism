import{expect,test}from"bun:test";
import{toArrayBuffer,type Pointer}from"bun:ffi";
import{coreCall}from"../src/native-core";
import type{TreeGitDecoration}from"../src/ui/TreeRowGlyphs";
const newHost=()=>coreCall<{id:number}>("treeglyphs.new",{family:"Cascadia Code NF",weight:400,fonts:["Symbols Nerd Font Mono"]}).id;
const file=(path:string,status:string)=>({path,status,additions:0,deletions:0,staged:status==="Staged"||status==="Mixed"});
const snapshot=(workspace_root:string,repo_root:string|null,files:ReturnType<typeof file>[])=>({workspace_root,repo_root,files,branch:"main",upstream:null,error:null});
interface Row{raster:{id:number;ptr:number;len:number;width:number;height:number;scale:number};label:string;labelX:number;gitMarker:string|null;gitColor:number[];gitMarkerX:number;gitMarkerWidth:number;gitTrailingWidth:number;labelColor:number[];iconColor:number[]}
const base={name:"VeryLongFileNameThatNeedsTruncation.ts",width:240,scale:1,chevron:"",icon:"\uf15b",labelColor:"#00ff00",iconColor:"#ff0000",chevronColor:"#0000ff",theme:"pastel_dark"};
test("native tree Git projection reuses canonical status badges, ancestor aggregation and clearing without filesystem access",()=>{
 const id=newHost(),project=(value:ReturnType<typeof snapshot>)=>coreCall<TreeGitDecoration[]>("treeglyphs.git-project",{id,snapshot:value,theme:"pastel_dark",scale:1});
 try{
  const rows=project(snapshot("/nonexistent-host/repo","/nonexistent-host/repo",[file("src/m.rs","Modified"),file("src/s.rs","Staged"),file("src/mixed.rs","Mixed"),file("src/new.rs","Added"),file("src/deleted.rs","Deleted"),file("src/renamed.rs","Renamed"),file("src/untracked.rs","Untracked"),file("src/conflict.rs","Conflict")]));
  const map=new Map(rows.map(row=>[row.path,row]));expect([...map.keys()]).toHaveLength(9);
  for(const[path,marker]of[["m.rs","M"],["s.rs","S"],["mixed.rs","M*"],["new.rs","A"],["deleted.rs","D"],["renamed.rs","R"],["untracked.rs","?"],["conflict.rs","!"]]){const row=map.get(`src/${path}`)!;expect(row.marker).toBe(marker);expect(row.directory).toBe(false);expect(row.markerWidth).toBeGreaterThan(0);expect(row.trailingWidth).toBeCloseTo(row.markerWidth+8);expect(row.color[3]).toBe(255);}
  expect(map.get("src")?.status).toBe("Conflict");expect(map.get("src")?.directory).toBe(true);
  expect(project(snapshot("/nonexistent-host/repo","/nonexistent-host/repo",[]))).toEqual([]);expect(project(snapshot("/nonexistent-host/repo",null,[file("src/m.rs","Modified")]))).toEqual([]);
 }finally{coreCall("treeglyphs.dispose",{id});}
});
test("Git decorations anchor repository paths at the selected host workspace subdirectory on Unix and Windows",()=>{
 const id=newHost();try{
  for(const[repo,root]of[["/remote/repo","/remote/repo/sub"],["C:\\host\\repo","C:\\host\\repo\\sub"]]){
   const rows=coreCall<TreeGitDecoration[]>("treeglyphs.git-project",{id,theme:"pastel_dark",snapshot:snapshot(root,repo,[file("sub/nested/changed.rs","Mixed"),file("outside.rs","Conflict")])});
   expect(rows.map(row=>row.path).sort()).toEqual(["nested","nested/changed.rs"]);expect(rows.every(row=>row.marker==="M*")).toBe(true);
  }
  const rows=coreCall<TreeGitDecoration[]>("treeglyphs.git-project",{id,snapshot:snapshot("/remote/repo","/remote/repo",[file("src\\literal","Untracked"),file("src/literal","Deleted")])});
  expect(rows.map(row=>row.path).sort()).toEqual(["src","src/literal","src\\literal"]);
 }finally{coreCall("treeglyphs.dispose",{id});}
});
test("tree Git raster colors labels and directory icons while retaining filetype icon and right marker room beside peers",()=>{
 const id=newHost(),row=(extra:Record<string,unknown>={})=>coreCall<Row>("treeglyphs.row",{id,...base,...extra});
 try{
  const clean=row(),modified=row({gitStatus:"Modified"}),withPeer=row({gitStatus:"Modified",trailingWidth:55}),directory=row({gitStatus:"Modified",directory:true});
  expect(modified.gitMarker).toBe("M");expect(modified.labelColor).toEqual(modified.gitColor);expect(modified.iconColor).toEqual([255,0,0,255]);expect(directory.iconColor).toEqual(directory.gitColor);
  expect(modified.gitMarkerX+modified.gitMarkerWidth).toBeCloseTo(base.width-4);expect(withPeer.gitMarkerX).toBe(modified.gitMarkerX);expect(withPeer.label.length).toBeLessThan(modified.label.length);expect(withPeer.label).toEndWith("…");
  expect(clean.gitMarker).toBeNull();expect(clean.labelColor).toEqual([0,255,0,255]);expect(row({gitStatus:"Modified"}).raster.id).toBe(modified.raster.id);expect(row({gitStatus:"Added"}).raster.id).not.toBe(modified.raster.id);expect(row({gitStatus:"Modified",theme:"tokyo_night"}).raster.id).not.toBe(modified.raster.id);
  const pixels=new Uint8Array(toArrayBuffer(modified.raster.ptr as unknown as Pointer,0,modified.raster.len));let markerInk=0;
  for(let y=0;y<modified.raster.height;y++)for(let x=Math.floor(modified.gitMarkerX*2);x<modified.raster.width;x++){const at=(y*modified.raster.width+x)*4;if(pixels[at+3]>150)markerInk++;}
  expect(markerInk).toBeGreaterThan(3);
 }finally{coreCall("treeglyphs.dispose",{id});}
});
