/** Summarize opt-in compositor feedback, separately from application draw counts. */
export function summarizePresentation(log:string){
 const reports:unknown[]=[];let label:string|undefined,times:number[]=[],refreshes:number[]=[],discarded=0;
 for(const line of log.split("\n")){
  if(line.startsWith("[neoism-measure-start]")){label=line.slice(23).trim();times=[];refreshes=[];discarded=0;}
  else if(label&&line.startsWith("[gpui-presented]")){
   const time=/time_ns=(\d+)/.exec(line),refresh=/refresh_ns=(\d+)/.exec(line);
   if(time)times.push(Number(time[1])/1e6);if(refresh&&Number(refresh[1])>0)refreshes.push(Number(refresh[1])/1e6);
  }else if(label&&line.startsWith("[gpui-discarded]"))discarded++;
  else if(label&&line.startsWith("[neoism-measure-end]")){
   refreshes.sort((a,b)=>a-b);const interval=refreshes[Math.floor(refreshes.length/2)];
   const gaps=times.slice(1).map((time,i)=>time-times[i]),sorted=[...gaps].sort((a,b)=>a-b);
   reports.push({label,presented:times.length,discarded,refreshHz:interval?1000/interval:null,
    presentedFps:times.length>1?(times.length-1)*1000/(times.at(-1)!-times[0]):null,
    missedRefreshes:interval?gaps.reduce((sum,gap)=>sum+Math.max(0,Math.round(gap/interval)-1),0):null,
    intervalP99Ms:sorted[Math.floor((sorted.length-1)*.99)]??null,intervalMaxMs:sorted.at(-1)??null});label=undefined;
  }
 }
 return reports;
}
if(import.meta.main){const path=process.argv[2];if(!path)throw new Error("Pass the benchmark log path");console.log(JSON.stringify(summarizePresentation(await Bun.file(path).text()),null,2));}
