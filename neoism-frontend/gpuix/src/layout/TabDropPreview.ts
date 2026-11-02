import {WorkspaceLayout,type LayoutFrame} from "./WorkspaceLayout";

/** A target tab strip always means adoption, never a top-edge split. */
export function tabDropPoint(frame:LayoutFrame,source:number,x:number,y:number,tabHeight:number){
  const pane=frame.panes.find(p=>x>=p.rect[0]&&x<p.rect[0]+p.rect[2]&&y>=p.rect[1]&&y<p.rect[1]+tabHeight);
  if(pane&&!pane.tabs.some(tab=>tab.leaf===source))return {x:pane.rect[0]+pane.rect[2]/2,y:pane.rect[1]+pane.rect[3]/2};
  return {x,y};
}

/** Solve a disposable copy so hovering never saves or mutates the real tree.
 * Hit testing stays in the committed geometry to avoid oscillating targets as
 * the live panes move beneath the pointer. Re-solve only when the zone changes. */
export class TabDropPreview {
  private base?:LayoutFrame;
  private key?:string;
  private frame?:LayoutFrame;
  clear(){this.base=undefined;this.key=undefined;this.frame=undefined;}
  update(controller:WorkspaceLayout,source:number,x:number,y:number,tabHeight:number):LayoutFrame|undefined{
    const point=tabDropPoint(controller.frame,source,x,y,tabHeight);
    const zone=controller.dropPreview(point.x,point.y,source);
    if(zone.target===null){this.clear();return;}
    const key=`${source}:${zone.target}:${zone.placement}`;
    if(this.base===controller.frame&&this.key===key)return this.frame;
    const copy=new WorkspaceLayout(controller.snapshot());
    try{
      const viewport=controller.measuredViewport;
      copy.layout(viewport.width,viewport.height,viewport.scale);
      this.frame=copy.drop(source,point.x,point.y);
      this.base=controller.frame;this.key=key;
      return this.frame;
    }finally{copy.dispose();}
  }
}
