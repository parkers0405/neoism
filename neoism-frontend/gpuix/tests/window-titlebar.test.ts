import {expect,test} from "bun:test";
import {observeTitlebarInset,windowTitlebarOptions} from "../src/chrome/window-titlebar";
import ts from "typescript";
import {readFileSync} from "node:fs";
import {readDisplayPin,frontendRoot,sha256,displayIdentity} from "../src/display-binding";

test("Windows requests native chrome while Mac and Linux keep their existing titlebars",()=>{
 expect(windowTitlebarOptions("win32")).toEqual({titlebarTransparent:false});
 for(const platform of ["darwin","linux","freebsd"]){
  expect(windowTitlebarOptions(platform)).toEqual({titlebarTransparent:true});
 }
 // No shared mutable options can leak a platform decision into another window.
 windowTitlebarOptions("win32").titlebarTransparent=true;
 expect(windowTitlebarOptions("win32").titlebarTransparent).toBe(false);
});

const source=(path:string)=>readFileSync(new URL(`../${path}`,import.meta.url),"utf8");
test("both real window creation paths pass platform policy directly to native init",()=>{
 for(const path of ["src/launch.ts","src/main.tsx"]){
  const file=ts.createSourceFile(path,source(path),ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
  const options:ts.ObjectLiteralExpression[]=[];
  const visit=(node:ts.Node)=>{
   if(ts.isCallExpression(node)&&ts.isPropertyAccessExpression(node.expression)&&node.expression.name.text==="initAsync"){
    const argument=node.arguments[0];
    if(argument&&ts.isObjectLiteralExpression(argument))options.push(argument);
   }
   ts.forEachChild(node,visit);
  };visit(file);
  expect(options.length).toBe(1);
  const properties=options[0]!.properties;
  // No later titlebar assignment/spread may override the tested platform policy.
  const spreads=properties.filter(ts.isSpreadAssignment);
  expect(spreads.map(spread=>spread.expression.getText(file))).toEqual(["windowTitlebarOptions(process.platform)"]);
  expect(properties.some(property=>property.name?.getText(file)==="titlebarTransparent")).toBe(false);
  expect(source(path)).toContain('import {windowTitlebarOptions} from "./chrome/window-titlebar"');
 }
});

test("production bundles both changed entrypoints and pins the native caption fix",()=>{
 expect(source("src/cli.ts")).toContain('import("./launch")');
 const build=source("scripts/build.ts");
 expect(build).toContain('entrypoints:["./src/main.tsx"]');
 expect(build).toContain('entrypoints: ["./src/cli.ts"]');
 expect(build).toContain('scripts/build-display-binding.ts');
 const pin=readDisplayPin();
 const patch=readFileSync(`${frontendRoot}/${pin.zedPatch}`,"utf8");
 expect(sha256(Buffer.from(patch))).toBe(pin.zedPatchSha256!);
 expect(pin.zedPatchedFiles?.["crates/gpui_windows/src/window.rs"]).toMatch(/^[a-f0-9]{64}$/);
 // false alone is insufficient: GPUI must actually set the Win32 caption style.
 expect(patch).toContain('+            if !hide_title_bar {\n+                dwstyle |= WS_CAPTION;\n+            }');
 expect(patch).toContain('crates/gpui_windows/src/window.rs');
 // A previously verified addon must not bypass compilation of the native fix.
 expect(displayIdentity(pin,"win32","x64",false)).not.toBe(displayIdentity({...pin,zedPatchSha256:"pre-caption-patch"},"win32","x64",false));
});

test("Mac chrome follows native fullscreen changes and stops observing on disposal",async()=>{
 let fullscreen=false,reads=0;const insets:number[]=[];
 const dispose=observeTitlebarInset({isFullscreen(){reads++;return fullscreen;}},"darwin",value=>insets.push(value));
 try{
  expect(insets).toEqual([80]);
  fullscreen=true;await Bun.sleep(120);expect(insets).toEqual([80,0]);
  fullscreen=false;await Bun.sleep(120);expect(insets).toEqual([80,0,80]);
 }finally{dispose();}
 const stopped=reads;await Bun.sleep(120);expect(reads).toBe(stopped);
});

test("other platforms reserve no traffic-light space or fullscreen polling",()=>{
 for(const platform of ["linux","win32"]){
  const insets:number[]=[];const dispose=observeTitlebarInset({isFullscreen(){throw new Error("must not query");}},platform,value=>insets.push(value));
  expect(insets).toEqual([0]);dispose();
 }
});
