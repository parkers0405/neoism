import type {HistoryMessage,Usage} from "../daemon/protocol";

const hasTokens=(usage:Usage|null|undefined):usage is Usage=>!!usage&&!!(usage.input||usage.output||usage.total);
/** Match the shared pane: a stopped/failed zero-token turn must not erase
 * the last real context measurement, including after reopening a session. */
export function contextUsage(history:readonly HistoryMessage[],live?:Usage):Usage|undefined{
 if(hasTokens(live))return live;
 for(let index=history.length-1;index>=0;index--){const usage=history[index]!.usage;if(hasTokens(usage))return usage;}
 return undefined;
}
