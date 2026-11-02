import type {HistoryMessage,Usage,Provider,ProviderState} from "../daemon/protocol";

const hasTokens=(usage:Usage|null|undefined):usage is Usage=>!!usage&&!!(usage.input||usage.output||usage.total);
/** Match the shared pane: a stopped/failed zero-token turn must not erase
 * the last real context measurement, including after reopening a session. */
export function contextUsage(history:readonly HistoryMessage[],live?:Usage):Usage|undefined{
 if(hasTokens(live))return live;
 for(let index=history.length-1;index>=0;index--){const usage=history[index]!.usage;if(hasTokens(usage))return usage;}
 return undefined;
}

export const compactTokenCount=(n:number)=>n>=1_000_000?`${(n/1_000_000).toFixed(1)}m`:n>=10000?`${(n/1000).toFixed(1)}k`:String(n);

/** One measurement and denominator for the detail rail and composer. Session
 * limits take precedence over catalog limits (e.g. account-specific windows). */
export function contextMetrics(history:readonly HistoryMessage[],live:Usage|undefined,providerState:ProviderState|undefined,providers:readonly Provider[],selectedModel:string){
 const usage=contextUsage(history,live),slash=selectedModel.indexOf("/");
 const catalog=providers.find(provider=>provider.id===selectedModel.slice(0,slash))?.models?.find(model=>model.id===selectedModel.slice(slash+1));
 const candidate=providerState?.context_limit||catalog?.context_limit||usage?.context_limit;
 const limit=candidate&&Number.isFinite(candidate)&&candidate>0?candidate:undefined;
 const total=usage?.total??(usage?.input||0)+(usage?.output||0);
 const percent=limit?Math.round(total/limit*100):undefined;
 return {usage,limit,total,percent,
  label:usage?`${compactTokenCount(total)}${percent!==undefined?` (${percent}%)`:""}`:undefined,
  accessibleLabel:usage?`Context usage: ${total.toLocaleString("en-US")} tokens${limit?` of ${limit.toLocaleString("en-US")} (${percent}%)`:"; context limit unknown"}`:undefined};
}
