import {expect,test} from "bun:test";
import {toArrayBuffer,type Pointer} from "bun:ffi";
import {coreCall} from "../src/native-core";
import {WordmarkHosts,initialWordmarkSize,wordmarkImageOps,translateWordmarkOps,type WordmarkFrame} from "../src/ui/AnimatedWordmark";
import {CanvasImages,RasterSource,rasterKey,type CanvasOp,type CanvasRaster} from "../src/ui/canvas";
const create=(kind="agent")=>coreCall<{id:number}>("wordmark.new",{kind}).id;
const frame=(id:number,args:Record<string,unknown>={})=>coreCall<WordmarkFrame>("wordmark.frame",{id,width:360,height:58,fg:"#204060",seconds:0,...args});
const dispose=(id:number)=>coreCall("wordmark.dispose",{id});

test("wordmark keeps exact original tinted artwork and fractional image slices",()=>{
 const id=create();try{
  const first=frame(id,{reducedMotion:true}),image=first.image!;
  expect([image.width,image.height,image.len]).toEqual([1196,193,1196*193*4]);
  const pixels=new Uint8Array(toArrayBuffer(image.ptr as unknown as Pointer,0,image.len));let opaque=0,wrong=0;
  for(let i=0;i<pixels.length;i+=4){if(pixels[i+3]===255){opaque++;if(pixels[i]>32||pixels[i+1]>64||pixels[i+2]>96)wrong++;}}
  expect(opaque).toBeGreaterThan(1000);expect(wrong).toBe(0);expect(first.letters[0].rect).toEqual([0,0,60,58]);expect(first.animating).toBe(false);
  const ops=wordmarkImageOps(first,123,"agent");expect(ops).toHaveLength(36);
  expect(ops[0]).toEqual({k:"cl",b:first.letters[0].glow});expect(ops[4]).toEqual({k:"i",id:123,b:[0,0,360,58]});
  expect(ops[10]).toEqual({k:"i",id:123,b:[0,0,360,58]});
  expect(frame(id,{reducedMotion:true}).image?.id).toBe(image.id);
  const changed=frame(id,{reducedMotion:true,fg:"#a0b0c0"});expect(changed.image?.id).not.toBe(image.id);
  // The previous image remains pinned through a palette switch.
  expect(new Uint8Array(toArrayBuffer(image.ptr as unknown as Pointer,0,4))).toHaveLength(4);
 }finally{dispose(id);}
});
test("original shuffled bag visits all fifteen click effects once and avoids boundary repeats",()=>{
 const id=create();try{
  const initial=frame(id);expect(initial.effect).toBeNull();expect(initial.showOriginal).toBe(true);
  const effects:string[]=[];let animated=0;
  for(let i=0;i<31;i++){coreCall("wordmark.replay",{id});const next=frame(id);effects.push(next.effect!);expect(next.clickAnimating).toBe(true);expect(next.showOriginal).toBe(false);animated+=next.commands.length;}
  expect(new Set(effects.slice(0,15)).size).toBe(15);expect(new Set(effects.slice(15,30)).size).toBe(15);expect(effects[14]).not.toBe(effects[15]);expect(effects[29]).not.toBe(effects[30]);expect(animated).toBeGreaterThan(0);
  coreCall("wordmark.cancel",{id});expect(frame(id).effect).toBeNull();
 }finally{dispose(id);}
});
test("startup is opt in once after real layout and reduced motion suppresses animation",()=>{
 const id=create(),fresh=create();try{
  frame(id,{width:0,autoAnimate:true});expect(frame(id,{autoAnimate:true}).clickAnimating).toBe(true);
  coreCall("wordmark.cancel",{id});expect(frame(id,{autoAnimate:true}).clickAnimating).toBe(false);
  expect(frame(fresh,{autoAnimate:false}).clickAnimating).toBe(false);expect(frame(fresh,{autoAnimate:true}).clickAnimating).toBe(false);
  coreCall("wordmark.replay",{id:fresh});expect(frame(fresh,{autoAnimate:false}).clickAnimating).toBe(true);
  const reduced=frame(fresh,{reducedMotion:true,autoAnimate:true});expect(reduced.clickAnimating).toBe(false);expect(reduced.animating).toBe(false);expect(reduced.commands).toEqual([]);
 }finally{dispose(id);dispose(fresh);}
});
test("idle geometry uses original per-letter shimmer and Agent/splash halo policies",()=>{
 const agent=create(),splash=create("splash");try{
  const a=frame(agent),b=frame(splash);expect(a.letters[0].rect).toEqual([0,0,60,58]);
  const scale=1+Math.sin(.16*Math.PI*2)*.025;expect(a.letters[1].rect[2]).toBeCloseTo(60*scale,4);
  expect(a.letters[0].glow[2]).toBeCloseTo(60*1.04,4);expect(b.letters[0].glow[2]).toBeCloseTo(60*(1+.05*(.05+.13*.5)),4);
  expect(wordmarkImageOps(b,10,"splash")[0]).toEqual({k:"cl",b:b.letters[0].rect});
 }finally{dispose(agent);dispose(splash);}
});

test("App-owned wordmark hosts retain startup and shuffle state across tab roundtrips and dispose explicitly",()=>{
 const hosts=new WordmarkHosts(),id=hosts.acquire("agent:one","agent");frame(id,{autoAnimate:true});coreCall("wordmark.cancel",{id});hosts.release("agent:one");const same=hosts.acquire("agent:one","agent");expect(same).toBe(id);expect(frame(same,{autoAnimate:true}).clickAnimating).toBe(false);const other=hosts.acquire("terminal:two","splash");expect(other).not.toBe(id);expect(frame(other).image?.id).toBe(frame(id).image?.id);hosts.dispose();hosts.release("agent:one");expect(()=>frame(id)).toThrow("Unknown wordmark host");expect(()=>hosts.acquire("later","agent")).toThrow("closed");
});


test("pane paint translation moves every coordinate while preserving UVs, sizes, clips and GPU styles",()=>{
 const ops:CanvasOp[]=[{k:"cl",b:[-12,-7,30,20]},{k:"i",id:7,b:[-12,-7,30,20],s:[.5,0,.75,1]},{k:"p",c:123,p:[[-4,-8,12,3,0,10]]},{k:"l",c:1,w:2,p:[-5,-2,8,4]},{k:"r",b:[-9,-8,12,13],c:2,r:[1,2,3,4]},{k:"t",x:2,y:-3,t:"glyph",s:14,c:3,f:"Cascadia"},{k:"u"}];
 const before=JSON.stringify(ops),moved=translateWordmarkOps(ops,200,100);
 expect(moved[0]).toEqual({k:"cl",b:[188,93,30,20]});expect(moved[1]).toEqual({k:"i",id:7,b:[188,93,30,20],s:[.5,0,.75,1]});
 expect(moved[2]).toEqual({k:"p",c:123,p:[[196,92,212,103,200,110]]});expect(moved[3]).toEqual({k:"l",c:1,w:2,p:[195,98,208,104]});
 expect(moved[4]).toEqual({k:"r",b:[191,92,12,13],c:2,r:[1,2,3,4]});expect(moved[5]).toEqual({k:"t",x:202,y:97,t:"glyph",s:14,c:3,f:"Cascadia"});expect(moved[6]).toBe(ops[6]);expect(JSON.stringify(ops)).toBe(before);
});

test("original hover and click vectors can leave the logo while remaining inside the pane paint bounds",async()=>{
 const id=create(),clip=[-180,-120,900,700];try{
  frame(id,{clip,mouse:[210,10]});await Bun.sleep(85);
  const hovered=frame(id,{clip,mouse:[210,10]});expect(hovered.letters[3].rect[1]).toBeLessThan(0);
  const idleOps=wordmarkImageOps(hovered,1,"agent"),translated=translateWordmarkOps(idleOps,180,120);
  const before=idleOps.find(op=>op.k==="cl"&&op.b[1]<0) as Extract<CanvasOp,{k:"cl"}>;expect(before).toBeDefined();
  expect(translated.some(op=>op.k==="cl"&&op.b[1]===before.b[1]+120)).toBe(true);
  let escapedLogo=false,polygons=0,outsidePane=0;
  for(let i=0;i<15;i++){
   coreCall("wordmark.replay",{id});const next=frame(id,{clip,mouse:[210,10]});
   const vectors=next.commands.flatMap(command=>command.kind==="polygon"?[{k:"p" as const,c:0,p:[command.points.flat()]}]:[]);
   for(const command of vectors)for(let n=0;n<command.p[0].length;n+=2){const [x,y]=command.p[0].slice(n,n+2);if(x<0||x>360||y<0||y>58)escapedLogo=true;}
   for(const command of translateWordmarkOps(vectors,180,120))if(command.k==="p")for(const points of command.p)for(let n=0;n<points.length;n+=2){if(points[n]<-.001||points[n]>900.001||points[n+1]<-.001||points[n+1]>700.001)outsidePane++;}
   polygons+=vectors.length;
  }
  expect(polygons).toBeGreaterThan(0);expect(escapedLogo).toBe(true);expect(outsidePane).toBe(0);
 }finally{dispose(id);}
});


test("window-owned wordmark texture stays uploaded across font-generation canvas replacement",()=>{
 const hosts=new WordmarkHosts(),id=hosts.acquire("splash:welcome","splash"),uploads:number[]=[],drops:number[]=[];
 const renderer={putPaintImage:(key:number)=>{uploads.push(key);},dropPaintImage:(key:number)=>{drops.push(key);}};
 const before=new CanvasImages(renderer),after=new CanvasImages(renderer);
 try{
  const first=frame(id,{autoAnimate:true});coreCall("wordmark.cancel",{id});
  hosts.pinImage("splash:welcome",renderer,first.image!);before.use(RasterSource.wordmark,first.image!);before.commit();
  expect(uploads).toHaveLength(1);before.dispose();hosts.release("splash:welcome");expect(drops).toEqual([]);
  const retained=hosts.acquire("splash:welcome","splash"),next=frame(retained,{width:390,height:63,autoAnimate:true});
  expect(retained).toBe(id);expect(next.clickAnimating).toBe(false);expect(next.image?.id).toBe(first.image?.id);
  hosts.pinImage("splash:welcome",renderer,next.image!);after.use(RasterSource.wordmark,next.image!);after.commit();
  expect(uploads).toHaveLength(1);hosts.dispose();expect(drops).toEqual([]);after.dispose();expect(drops).toHaveLength(1);
 }finally{before.dispose();after.dispose();hosts.dispose();}
});

test("warm splash pane bounds survive generation swaps and recompute the original menu at the new scale",()=>{
 const hosts=new WordmarkHosts(),context="welcome:one",viewport={width:900,height:600};
 const id=coreCall<{id:number}>("splash.new",{fontFamily:"Cascadia Code NF",fontWeight:400}).id;
 try{
  hosts.rememberSplashViewport(context,viewport);viewport.width=1;hosts.rememberSplashViewport(context,{width:0,height:0});
  expect(hosts.splashViewport(context)).toEqual({width:900,height:600});
  const warm=hosts.splashViewport(context)!;
  const next=coreCall<{commands:unknown[];wordmark:number[]}>("splash.frame",{id,...warm,cols:Math.floor(warm.width/(15*.6)),rows:Math.floor(warm.height/(15*1.3)),cellHeight:15*1.3,scale:15/14,theme:"pastel_dark"});
  expect(next.commands.length).toBeGreaterThan(0);expect(next.wordmark[2]).toBeGreaterThan(0);
  const initial=initialWordmarkSize({width:next.wordmark[2],height:next.wordmark[3]});
  expect(initial.width).toBe(next.wordmark[2]);expect(initial.height).toBe(next.wordmark[3]);expect(initialWordmarkSize()).toEqual({x:0,y:0,width:0,height:0});
  hosts.dispose();expect(hosts.splashViewport(context)).toBeUndefined();
 }finally{coreCall("splash.dispose",{id});hosts.dispose();}
});


test("splash paint leases precede canvas ownership, evict after 512 images and leave the separate logo pin alive",()=>{
 const hosts=new WordmarkHosts(),id=hosts.acquire("welcome:leases","splash"),uploads:number[]=[],drops:number[]=[],order:string[]=[];
 const renderer={putPaintImage:(key:number)=>{order.push("upload");uploads.push(key);},dropPaintImage:(key:number)=>{drops.push(key);}};
 const glyphHost=coreCall<{id:number}>("treeglyphs.new",{family:"Cascadia Code NF",weight:400,fonts:[]}).id;
 try{
  const logo=frame(id).image!;hosts.pinImage("welcome:leases",renderer,logo);
  const glyph=coreCall<{raster:CanvasRaster}>("treeglyphs.row",{id:glyphHost,width:60,scale:1,name:"a",icon:"",chevron:"",labelColor:"#ffffff",iconColor:"#ffffff",chevronColor:"#ffffff"}).raster;
  // IDs are opaque canvas resource handles; reuse real FFI glyph pixels with
  // distinct handles to exercise the bounded cache without giant textures.
  const raster={...glyph,id:900_000};order.length=0;
  const canvas=new CanvasImages(renderer,(source,image)=>{order.push("retain");hosts.pinPaintImage(renderer,source,image);});
  canvas.use(RasterSource.painter,raster);canvas.commit();
  expect(order).toEqual(["retain","upload"]);canvas.dispose();expect(drops).toEqual([]);
  for(let index=1;index<512;index++)hosts.pinPaintImage(renderer,RasterSource.painter,{...raster,id:raster.id+index});
  expect(drops).toEqual([]);
  // Touch the first entry: the least recently used second one is evicted.
  hosts.pinPaintImage(renderer,RasterSource.painter,raster);
  hosts.pinPaintImage(renderer,RasterSource.painter,{...raster,id:raster.id+512});
  expect(drops).toEqual([rasterKey(RasterSource.painter,raster.id+1)]);
  expect(drops).not.toContain(rasterKey(RasterSource.wordmark,logo.id));
  const before=uploads.length;hosts.pinPaintImage(renderer,RasterSource.painter,raster);expect(uploads).toHaveLength(before);
  // A source namespace is part of the key, even for an identical raster ID.
  hosts.pinPaintImage(renderer,RasterSource.paintImage,raster);expect(uploads.at(-1)).toBe(rasterKey(RasterSource.paintImage,raster.id));
  hosts.dispose();expect(new Set(drops).size).toBe(new Set(uploads).size);expect(drops.length).toBe(uploads.length);
 }finally{hosts.dispose();coreCall("treeglyphs.dispose",{id:glyphHost});}
});
