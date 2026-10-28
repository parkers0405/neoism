import { revealProgress } from "../design/animation";
const presentedPanels=new WeakMap<object,Set<string>>();
/** App generations share their visual scope; actual panel closes reset only
 * that panel's short opening slide. Font changes keep the settled state. */
export function clearHiddenTreePanels(scope:object,visible:string|undefined){const panels=presentedPanels.get(scope);if(panels)for(const panel of panels)if(panel!==visible)panels.delete(panel);}
export function beginTreePanel(scope:object,panel:string){let panels=presentedPanels.get(scope);if(!panels){panels=new Set();presentedPanels.set(scope,panels);}const opening=!panels.has(panel);panels.add(panel);return opening;}

/** Include spring displacement and two overscan rows around the painted area. */
export function visibleTreeRange(total: number, height: number, rowHeight: number, top: number, offset: number): [number, number] {
  const first = top - offset / rowHeight;
  return [Math.max(0, Math.floor(first) - 2), Math.min(total, Math.ceil(first + height / rowHeight) + 2)];
}

/** Continue the same cubic reveal when a row enters the viewport mid-animation. */
export function treeReveal(elapsed: number, index: number, duration: number, stagger: number) {
  const delay = Math.min(index, 40) * stagger;
  return { opacity: revealProgress(elapsed, Math.min(index, 40), duration, stagger),
    delay: Math.max(0, delay - elapsed) / 1000, duration: Math.max(0, duration - Math.max(0, elapsed - delay)) / 1000 };
}
