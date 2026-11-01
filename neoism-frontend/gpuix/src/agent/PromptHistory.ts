import type {ComposerDraft} from "../model";
/** One navigation cursor per composer; the sent list belongs to the model. */
export class PromptHistory {
 private entries:readonly ComposerDraft[]=[];
 private index:number|undefined;
 private draft:ComposerDraft|undefined;
 reset(){this.index=undefined;this.draft=undefined;this.entries=[];}
 previous(entries:readonly ComposerDraft[],draft:ComposerDraft):ComposerDraft|undefined{
  if(this.index===undefined){if(!entries.length)return;this.entries=entries.slice();this.draft=draft;this.index=this.entries.length;}
  this.index=Math.max(0,this.index-1);return this.entries[this.index];
 }
 next():ComposerDraft|undefined{
  if(this.index===undefined)return;
  if(this.index+1<this.entries.length)return this.entries[++this.index];
  const draft=this.draft;this.reset();return draft;
 }
}
