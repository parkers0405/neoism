/** Ctrl +/-: apply the new size locally at once (like the desktop, which
 * resizes in the same frame), then persist the daemon's global appearance
 * preference in the background. Writes are serialized so the last size wins. */
export class FontZoomController{
 private pending=Promise.resolve();
 constructor(private readonly current:()=>number,private readonly write:(size:number)=>Promise<void>,private readonly apply:(size:number)=>Promise<void>){}
 /** Resolves once every queued preference write has finished. */
 settled(){return this.pending;}
 change(delta:1|-1){
  const before=this.current(),size=Math.max(7,Math.min(42,before+delta));if(size===before)return Promise.resolve();
  const applied=this.apply(size);
  this.pending=this.pending.then(()=>this.write(size)).catch(()=>{});
  return applied;
 }
}
