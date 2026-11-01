import {launch} from "@gpuix/native/automation";
import {mkdir} from "node:fs/promises";
import {fileURLToPath} from "node:url";
import {join} from "node:path";
import {spawnSync} from "node:child_process";
import {readFileSync} from "node:fs";

const cwd=fileURLToPath(new URL("..",import.meta.url)),output=join(cwd,"artifacts/device-preview");
await mkdir(output,{recursive:true});
console.log("Opening native DevicePanel capture fixture...");
const app=await launch({command:process.execPath,args:["--hot","scripts/preview-device-panel.tsx"],cwd,env:{...process.env,NEOISM_GPUIX_DEVICE_PREVIEW:"1",GPUIX_BACKGROUND:"1"}});
const settle=async()=>{await app.call("mouseMove",{x:10,y:40});await Bun.sleep(350);};
const capture=async(name:string)=>{await settle();await app.screenshot({path:join(output,name)});console.log(`Captured ${name}`);};
const pixels=(name:string)=>{
  const path=join(output,name),png=readFileSync(path),width=png.readUInt32BE(16),height=png.readUInt32BE(20);
  const decoded=spawnSync(process.env.NEOISM_DEVICE_FFMPEG||"ffmpeg",["-hide_banner","-loglevel","error","-i",path,"-f","rawvideo","-pix_fmt","rgba","pipe:1"],{maxBuffer:width*height*4+65536});
  if(decoded.status!==0||decoded.stdout.length!==width*height*4)throw new Error(`Could not validate native screenshot pixels: ${name}`);
  return {width,height,at:(x:number,y:number)=>Array.from(decoded.stdout.subarray((Math.floor(y)*width+Math.floor(x))*4,(Math.floor(y)*width+Math.floor(x))*4+4))};
};
const model=async(id:string,label:string)=>{
  await app.getByTestId("device-model-picker").click();
  await app.getByTestId(`device-model:${id}`).click();
  const deadline=Date.now()+3000;
  while(await app.getByTestId("device-model-picker").textContent()!==label){
    if(Date.now()>deadline)throw new Error(`Model selection did not settle: ${label}`);
    await Bun.sleep(20);
  }
  await settle();
};
try{
  await app.getByTestId("device-preview-panel").waitFor({timeoutMs:60_000});
  await app.getByTestId("device-viewport").waitFor({timeoutMs:30_000});
  await capture("panel-phone.png");
  await app.getByTestId("device-model-picker").click();await capture("panel-model-menu.png");
  const normal=pixels("panel-phone.png"),popup=pixels("panel-model-menu.png");
  const sample=[normal.width*.42,normal.height*.76];
  if(JSON.stringify(normal.at(sample[0],sample[1]))!==JSON.stringify(popup.at(sample[0],sample[1])))throw new Error("The model menu obscured or changed the device behind its card");
  const swatch=await app.getByTestId("device-finish:burgundy").bounds(),panel=await app.getByTestId("device-preview-panel").bounds();
  const scale=popup.height/panel.height;
  const swatchColor=popup.at((swatch.x+swatch.width/2-panel.x)*scale,(swatch.y+swatch.height/2-panel.y)*scale);
  if(swatchColor.slice(0,3).some((value,index)=>Math.abs(value-[67,37,40][index])>2))throw new Error(`Native screenshot swatch mismatch: ${JSON.stringify({color:swatchColor,swatch,panel,image:{width:popup.width,height:popup.height}})}`);
  await app.getByTestId("device-model:iphone-18-pro-max").click();await capture("panel-pro-max.png");
  await app.getByTestId("device-more").click();await capture("panel-actions-menu.png");
  await app.getByTestId("device-back").click();await capture("panel-phone-back.png");
  await model("iphone-duo","iPhone Duo");await capture("panel-duo.png");
  await app.getByTestId("device-hinge-closed").click();await capture("panel-duo-closed.png");
  await model("ipad-pro-13-m5","iPad Pro 13-inch (M5)");
  await app.getByTestId("device-keyboard-toggle").click();await capture("panel-keyboard.png");
  console.log(`Captured the production DevicePanel in a native GPUI window: ${output}`);
}finally{await app.close();}