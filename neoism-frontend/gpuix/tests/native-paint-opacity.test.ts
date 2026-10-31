import {expect,test} from "bun:test";
import {mkdtempSync,readFileSync,writeFileSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {frontendRoot,readDisplayPin} from "../src/display-binding";

test("actual pinned GPUI cached-scene matcher repaints hidden and fractional-alpha scenes when ancestors change opacity",async()=>{
 const pin=readDisplayPin(),patch=readFileSync(join(frontendRoot,pin.zedPatch!),"utf8");
 const section=patch.split("diff --git ").find(section=>section.includes("/crates/gpui/src/view.rs "))!;
 expect(section).toBeDefined();
 const added=section.split("\n").filter(line=>(line.startsWith("+")&&!line.startsWith("+++"))||line.startsWith(" ")).map(line=>line.slice(1)).join("\n");
 const matcher=added.match(/impl ViewElementCacheKey \{[\s\S]*?\n\}/)?.[0];expect(matcher).toBeDefined();
 // Execute the exact GPUI cache comparison in Rust, substituting only the
 // unrelated geometry/text types. Alpha is baked into cached GPUI primitives;
 // reusing a zero-alpha range after a parent is revealed loses every glyph.
 const code=`#[derive(PartialEq)]struct Bounds<T>(T);#[derive(PartialEq)]struct ContentMask<T>(T);type Pixels=u32;type TextStyle=u32;
 struct ViewElementCacheKey{bounds:Bounds<Pixels>,content_mask:ContentMask<Pixels>,text_style:TextStyle,opacity:f32}
 ${matcher}
 #[test]fn staged_scene_alpha_is_never_reused_after_reveal(){
  for original in [0.0_f32,0.25,0.5,1.0]{
   let mut scene=ViewElementCacheKey{bounds:Bounds(100),content_mask:ContentMask(100),text_style:14,opacity:original};
   let mut painted_alpha=original;
   for next in [1.0_f32,0.0,0.5,0.25,1.0,1.0]{
    if !scene.matches(&Bounds(100),&ContentMask(100),&14,next){painted_alpha=next;scene.opacity=next;}
    assert_eq!(painted_alpha,next,"stale cached glyph alpha");
    assert!(scene.matches(&Bounds(100),&ContentMask(100),&14,next));
    assert!(!scene.matches(&Bounds(101),&ContentMask(100),&14,next));
    assert!(!scene.matches(&Bounds(100),&ContentMask(101),&14,next));
    assert!(!scene.matches(&Bounds(100),&ContentMask(100),&15,next));
   }
  }
 }`;
 // Verify the production prepaint path actually supplies inherited alpha to
 // that matcher, and Div propagates alpha while traversing child prepaints.
 expect(section).toContain("&bounds, &content_mask, &text_style, window.element_opacity");
 const div=patch.split("diff --git ").find(section=>section.includes("/crates/gpui/src/elements/div.rs "))!;
 expect(div).toContain("+                window.with_element_opacity(style.opacity, |window| {");
 const directory=mkdtempSync(join(tmpdir(),"neoism-native-paint-opacity-"));
 try{
  const source=join(directory,"paint-opacity.rs"),binary=join(directory,process.platform==="win32"?"paint-opacity.exe":"paint-opacity");writeFileSync(source,code);
  for(const command of [["rustc","--edition=2021","--test",source,"-o",binary],[binary]]){
   const child=Bun.spawn(command,{stdout:"pipe",stderr:"pipe"}),[status,out,err]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
   expect({status,err}).toEqual({status:0,err:""});if(command[0]===binary)expect(out).toContain("1 passed");
  }
 }finally{rmSync(directory,{recursive:true,force:true});}
},15000);
