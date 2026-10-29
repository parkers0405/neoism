import {WordmarkHosts} from "./AnimatedWordmark";
import {CanvasImages,RasterSource,type CanvasRaster} from "./canvas";
import {createTreeGlyphHost} from "./TreeRowGlyphs";
import type {AppConfig} from "../config";
import {coreCall} from "../native-core";
import {GitController} from "../git/GitController";
import {GitReviewController} from "../git/GitReviewController";
import {NativeComposer,type ComposerFrame} from "../agent/NativeComposer";
import type {WorkspaceModel,Surface} from "../model";
import {AgentChipHosts} from "../agent/AgentChipLabel";

export interface CachedAgentComposer {native?:NativeComposer;viewport:{width:number;height:number};frame?:ComposerFrame}
export interface CachedTreeViewport {showHidden:boolean;height:number;frame:{top:number;offset:number;selected:number;cursorOffset:number;animating:boolean}}

/** Window resources outlive font/theme component generations, but not the session. */
export class AppVisualState {
 readonly agentChips=new AgentChipHosts();
 readonly wordmarks=new WordmarkHosts();
 readonly trees=new Map<string,number>();
 readonly treeViewports=new Map<string,CachedTreeViewport>();
 readonly treeWidths=new Map<string,number>();
 private gitPanels=new Map<string,GitController>();
 private gitReviews=new Map<string,{controller:GitReviewController;reveal?:number}>();
 private agentComposers=new Map<string,{surfaceId:string;state:CachedAgentComposer}>();
 agentComposer(surfaceId:string,config:AppConfig,initialViewport?:{width:number;height:number}):CachedAgentComposer{
  const style=JSON.stringify([config.fontFamily,config.fontWeight,config.scale,config.themeName]),key=`${surfaceId}\0${style}`;
  const existing=this.agentComposers.get(key);if(existing)return existing.state;
  const previous=[...this.agentComposers.values()].find(entry=>entry.surfaceId===surfaceId)?.state;
  const state:CachedAgentComposer={viewport:initialViewport&&initialViewport.width>0&&initialViewport.height>0?{...initialViewport}:previous?.viewport||{width:0,height:0}};
  this.agentComposers.set(key,{surfaceId,state});return state;
 }
 pruneAgentComposers(surfaces:readonly Surface[]){const ids=new Set(surfaces.filter(surface=>surface.kind==="agent").map(surface=>surface.id));for(const[key,entry]of this.agentComposers)if(!ids.has(entry.surfaceId)){entry.state.native?.dispose();this.agentComposers.delete(key);}}
 gitPanel(workspaceId:string,model:WorkspaceModel,config:AppConfig){
  let controller=this.gitPanels.get(workspaceId);
  if(!controller){controller=new GitController({model,config});this.gitPanels.set(workspaceId,controller);}
  controller.configure(config);return controller;
 }
 gitReview(surface:Surface,model:WorkspaceModel,config:AppConfig,onClose:()=>void){
  if(!surface.git)throw new Error("Missing Git review descriptor");
  let entry=this.gitReviews.get(surface.id);
  if(!entry){entry={controller:new GitReviewController({model,config,descriptor:surface.git,git:this.gitPanel(surface.workspaceId,model,config),workspaceRoot:surface.git.root,onClose}),reveal:surface.gitReveal};this.gitReviews.set(surface.id,entry);}
  entry.controller.options.onClose=onClose;entry.controller.configure(config);
  if(entry.reveal!==surface.gitReveal){entry.reveal=surface.gitReveal;if(surface.git.path)entry.controller.reveal(surface.git.path);}
  return entry.controller;
 }
 pruneGitReviews(surfaces:readonly Surface[]){const ids=new Set(surfaces.map(surface=>surface.id));for(const[id,entry]of this.gitReviews){if(!ids.has(id)){entry.controller.dispose();this.gitReviews.delete(id);continue;}const surface=surfaces.find(surface=>surface.id===id);if(surface?.git&&entry.reveal!==surface.gitReveal){entry.reveal=surface.gitReveal;if(surface.git.path)entry.controller.reveal(surface.git.path);}}}
 applyGitReply(root:string,reply:unknown){for(const entry of this.gitReviews.values())if(entry.controller.descriptor.root===root)entry.controller.call("reply",{message:{PullRequest:{reply}}});}
 private glyphHosts=new Map<string,ReturnType<typeof createTreeGlyphHost>>();
 private rasterPins=new Map<string,CanvasImages>();
 treeGlyphHost(config:AppConfig){const key=JSON.stringify([config.fontFamily,config.fontWeight,config.iconFontFamily]);let host=this.glyphHosts.get(key);if(!host){host=createTreeGlyphHost(config);this.glyphHosts.set(key,host);}return host;}
 pinTreeRaster(renderer:object,key:string,raster:CanvasRaster){let images=this.rasterPins.get(key);if(!images)images=new CanvasImages(renderer);this.rasterPins.delete(key);this.rasterPins.set(key,images);images.use(RasterSource.tree,raster);images.commit();while(this.rasterPins.size>512){const oldest=this.rasterPins.keys().next().value!;this.rasterPins.get(oldest)!.dispose();this.rasterPins.delete(oldest);}}
 private reveals=new Map<string,Map<string,number>>();
 treeReveals(workspaceId:string,notes:boolean,root:string|undefined){
  const key=JSON.stringify([workspaceId,notes,root]);let value=this.reveals.get(key);
  if(!value){value=new Map();this.reveals.set(key,value);}return value;
 }
 dispose(){this.agentChips.dispose();for(const entry of this.agentComposers.values())entry.state.native?.dispose();this.agentComposers.clear();for(const entry of this.gitReviews.values())entry.controller.dispose();this.gitReviews.clear();for(const controller of this.gitPanels.values())controller.dispose();this.gitPanels.clear();this.wordmarks.dispose();for(const id of this.trees.values())coreCall("dispose",{id});this.trees.clear();this.treeViewports.clear();this.reveals.clear();this.treeWidths.clear();for(const images of this.rasterPins.values())images.dispose();this.rasterPins.clear();for(const host of this.glyphHosts.values())host.dispose();this.glyphHosts.clear();}
}
