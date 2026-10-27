import type {Page} from "playwright-core"
import fs from "node:fs/promises"
import {constants} from "node:fs"
import os from "node:os"
import path from "node:path"
import {spawn} from "node:child_process"
import {randomUUID} from "node:crypto"
import {stateDirectory} from "./bridge.ts"
interface Frame {time:number;image:Buffer}
async function ffmpeg(args:string[]):Promise<void> {
  await new Promise<void>((resolve,reject)=> {
    const child=spawn(process.env.NEOISM_FFMPEG ?? "ffmpeg",args,{stdio:["ignore","ignore","pipe"]})
    let error=""
    const timer=setTimeout(()=>{child.kill("SIGKILL");reject(new Error("Video encoding timed out"))},60000)
    child.stderr.on("data",value=>{error=(error+value).slice(-3000)})
    child.once("error",error=>{clearTimeout(timer);reject(error)})
    child.once("exit",code=>{clearTimeout(timer);code === 0 ? resolve() : reject(new Error(`ffmpeg failed (${code}): ${error}`))})
  })
}
/** Bounded screenshot sampler; never presented as compositor capture or audio. */
export class Recorder {
  private frames:Frame[]=[]
  private bytes=0
  private timer:NodeJS.Timeout|undefined
  private inFlight:Promise<void>=Promise.resolve()
  private active=false
  private total=0
  private dropped=0
  private failure:string|undefined
  readonly started=Date.now()
  constructor(readonly page:Page,readonly rolling:boolean,readonly fps=4,readonly seconds=30) {}
  async start():Promise<void> {
    await ffmpeg(["-version"])
    this.active=true;await this.capture()
    if(this.failure){this.active=false;throw new Error(this.failure)}
    this.schedule()
  }
  private schedule():void {
    if(!this.active)return
    this.timer=setTimeout(()=>{this.inFlight=this.capture().finally(()=>this.schedule())},Math.round(1000/this.fps))
  }
  private async capture():Promise<void> {
    try {
      const image=await this.page.screenshot({type:"png",timeout:3000}),time=Date.now()
      if(image.length>64*1024*1024)throw new Error("A screenshot exceeds the recording memory limit")
      if(!this.rolling && (this.bytes+image.length>64*1024*1024 || this.frames.length>=1000)) {this.failure="Recording reached its 64MB/1000-frame limit; stop to save the captured portion";this.active=false;return}
      this.frames.push({image,time});this.bytes+=image.length;this.total++
      while(this.frames.length>1 && (this.bytes>64*1024*1024 || this.frames.length>1000 || (this.rolling && this.frames[0]!.time<time-this.seconds*1000))) {
        this.bytes-=this.frames.shift()!.image.length;this.dropped++
      }
    }catch(error){this.failure=error instanceof Error?error.message:String(error);this.active=false}
  }
  status():unknown {return {active:this.active,mode:this.rolling?"rolling":"recording",capture:"screenshot sampling",audio:false,targetFps:this.fps,frames:this.frames.length,totalFrames:this.total,droppedFrames:this.dropped,bytes:this.bytes,elapsedMs:Date.now()-this.started,error:this.failure}}
  async halt():Promise<void>{this.active=false;if(this.timer)clearTimeout(this.timer);await this.inFlight}
  async cancel():Promise<void>{await this.halt();this.frames=[];this.bytes=0}
  async save(seconds?:number):Promise<unknown> {
    const cutoff=seconds === undefined?0:Date.now()-seconds*1000, frames=this.frames.filter(frame=>frame.time>=cutoff)
    if(!frames.length)throw new Error("No video frames captured in the requested interval")
    const directory=path.join(stateDirectory(),"recordings");await fs.mkdir(directory,{recursive:true,mode:0o700})
    const temporary=await fs.mkdtemp(path.join(os.tmpdir(),"neoism-video-")),output=path.join(directory,`${randomUUID()}.webm`)
    try {
      const lines=["ffconcat version 1.0"]
      for(let i=0;i<frames.length;i++) {
        const file=`frame-${i}.png`;await fs.writeFile(path.join(temporary,file),frames[i]!.image,{mode:0o600})
        lines.push(`file '${file}'`,`duration ${Math.max(0.02,((frames[i+1]?.time ?? (frames[i]!.time+1000/this.fps))-frames[i]!.time)/1000)}`)
      }
      lines.push(`file 'frame-${frames.length-1}.png'`)
      await fs.writeFile(path.join(temporary,"frames.txt"),lines.join("\n"))
      const encoded=path.join(temporary,"video.webm")
      await ffmpeg(["-hide_banner","-loglevel","error","-f","concat","-safe","1","-i",path.join(temporary,"frames.txt"),"-vf","pad=ceil(iw/2)*2:ceil(ih/2)*2","-c:v","libvpx-vp9","-threads","2","-deadline","realtime","-cpu-used","8","-pix_fmt","yuv420p","-an","-n",encoded])
      await fs.copyFile(encoded,output,constants.COPYFILE_EXCL);await fs.chmod(output,0o600)
      return {path:output,frames:frames.length,durationMs:frames[frames.length-1]!.time-frames[0]!.time,capture:"screenshot sampling",audio:false}
    }finally{await fs.rm(temporary,{recursive:true,force:true})}
  }
}
