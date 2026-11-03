export type TimelineFrame={commands:{kind:string;text?:string;x?:number;y?:number}[];reason:string|null;preparing:boolean};
const geometryMotion=new Set(["tool_motion","text_reveal","tool_expansion","timeline_inertia","timeline_interaction_settle"]);
const geometry=(frame:TimelineFrame)=>JSON.stringify(frame.commands.filter(command=>command.kind==="text").map(({text,x,y})=>[text,x,y]));

// Frame metadata is sampled AFTER rendering, but expansion height is sampled
// BEFORE cold text preparation. A long frame can paint old geometry and report
// no remaining animation. Require matching fresh paints, not merely reason=null.
export async function settledTimelineFrame<T extends TimelineFrame>(render:()=>T,options:{label:string;ready:(frame:T)=>boolean;now?:()=>number;sleep?:(ms:number)=>Promise<unknown>;budgetMs?:number}){
 const now=options.now??(()=>performance.now()),sleep=options.sleep??(ms=>Bun.sleep(ms));
 let value=render(),previous:string|undefined;
 // Synchronous native preparation is measured by the caller/hook watchdog;
 // this deadline bounds subsequent convergence, including confirmation paints.
 const deadline=now()+(options.budgetMs??1000);
 for(;;){
  const calm=!value.preparing&&!geometryMotion.has(value.reason||"");
  const snapshot=geometry(value);
  const ready=calm&&options.ready(value);
  if(ready&&snapshot===previous)return value;
  previous=ready?snapshot:undefined;
  if(now()>=deadline)throw new Error(`Composition ${options.label} did not converge: reason=${value.reason}, preparing=${value.preparing}, rendered=${snapshot}`);
  // Confirm a calm frame immediately. Moving/preparing frames need real time
  // to advance; never wait for unrelated decorative or syntax-colour animation.
  if(!calm)await sleep(16);
  else await sleep(0);
  value=render();
 }
}