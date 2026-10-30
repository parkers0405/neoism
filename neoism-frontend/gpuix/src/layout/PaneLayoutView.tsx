import { createEffect, createMemo, createSignal, For, onCleanup, onMount, Show, untrack, useContext, type Accessor, type JSX } from "solid-js";
import { GenerationRetiring } from "../ui/AppGeneration";
import { motion, useGpuixRequired, type EventPayload, type StyleDesc, type HostElement } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { dimensions as d, namedIcons, extensionIcons } from "../design/generated";
import { WorkspaceLayout, type DropPreview, type LayoutDivider, type LayoutPane, type LayoutTab } from "./WorkspaceLayout";
import { inactiveTabIcon, tabChrome } from "./tab-chrome";
import neoismIcon from "../../../shared/assets/icons/neoism.png" with { type: "file" };
import { readFileSync } from "node:fs";
import { easeOutCubicBezier } from "../design/animation";
import { paneHostKey } from "./pane-host";
import { HoverTabTitle } from "./HoverTabTitle";
import { verticalTabScroll,consumeTabScroll } from "./tab-scroll";
import { TabSlide } from "./tab-slide";

const sameTab = (a: LayoutTab, b: LayoutTab) => a.leaf === b.leaf && a.surfaceId === b.surfaceId && a.title === b.title && a.kind === b.kind;
const samePane = (a: LayoutPane, b: LayoutPane) => a.leaf === b.leaf && a.focused === b.focused && a.surfaceId === b.surfaceId && a.rect.every((n, i) => n === b.rect[i]) && a.tabs.length === b.tabs.length && a.tabs.every((tab, i) => sameTab(tab, b.tabs[i]));
const sameIds = (a: readonly number[], b: readonly number[]) => a.length === b.length && a.every((n, i) => n === b[i]);
const sameKeys = (a: readonly string[], b: readonly string[]) => a.length === b.length && a.every((key, i) => key === b[i]);
let cachedNeoismIcon: Buffer | undefined;

/** Paint the canonical solver's panes; no second layout engine in the host. */
/** Measured tab title/icon widths, kept across remounts (bounded by distinct titles). */
const measuredTabWidths = new Map<string, number>();
/** Last laid-out pane area, used to size a workspace layout before its first pass. */
let lastPaneArea: { width: number; height: number; scale: number } | undefined;

export function PaneLayoutView(props: {
  controller: WorkspaceLayout; config: AppConfig;
  renderSurface: (surfaceId: string, leaf: number, focused: Accessor<boolean>) => JSX.Element;
  onFocus?: (surfaceId: string, leaf: number) => void;
  onClose: (surfaceId: string, leaf: number) => void;
  onNewWelcome?: (leaf: number) => void;
  modified?: (surfaceId: string) => boolean;
  keyboardTab?:number;
  keyboardNewPane?:number;
  registerTabHost?:(leaf:number,host:HostElement|undefined)=>void;
  registerNewHost?:(leaf:number,host:HostElement|undefined)=>void;
  onTabScroll?:()=>void;
  onContentBounds?:(bounds:{x:number;y:number;width:number;height:number})=>void;
  onTabLayout?:(leaf:number,rect:{x:number;y:number;width:number;height:number})=>void;
  onNewLayout?:(leaf:number,rect:{x:number;y:number;width:number;height:number})=>void;
  empty?: JSX.Element;
}) {
  const [frame, setFrame] = createSignal(props.controller.frame);
  // While a replacement generation settles, keep this one's last geometry: the
  // shared controller is being laid out at the replacement's scale.
  const retiring = useContext(GenerationRetiring);
  createEffect(() => {
    const controller = props.controller;
    // A workspace's layout is unsized until its first layout pass; the pane
    // area is the same window region, so seed it and skip a 0x0 first frame.
    const scale = props.config.scale;
    if (lastPaneArea?.scale === scale && !untrack(retiring)) controller.layout(lastPaneArea.width, lastPaneArea.height, scale);
    setFrame(controller.frame); onCleanup(controller.subscribe(next => { if (!untrack(retiring)) setFrame(next); }));
  });
  const config = props.config, s = config.scale, c = config.theme,renderer=useGpuixRequired();
  const chrome = tabChrome(s).buffer;
  const paneCount = createMemo(() => frame().panes.length);
  const paneHosts = createMemo(() => { const next=frame();return new Map(next.panes.map(pane=>[paneHostKey(next,pane),pane])); });
  const paneIds = createMemo(() => [...paneHosts().keys()], [], { equals: sameKeys });
  const [origin, setOrigin] = createSignal({ x: 0, y: 0 });
  type ResizeDrag = { kind: "resize"; divider: LayoutDivider; x: number; y: number };
  type TabDrag = { kind: "tab"; leaf: number; x: number; y: number; grab: number };
  const strips = new Map<string,{pane:()=>LayoutPane;widths:Map<number,number>;scroll:()=>number;slide:TabSlide<number>}>();
  const tabGeometry=new Map<number,ReturnType<typeof tabChrome>["buffer"]>();
  const [drag, setDrag] = createSignal<ResizeDrag | TabDrag>();
  const [dragPoint,setDragPoint]=createSignal({x:0,y:0});
  const [preview, setPreview] = createSignal<DropPreview>();
  let armed: TabDrag | undefined;
  const focus = (leaf: number) => {
    const next = props.controller.focus(leaf);
    if (next.focusedSurfaceId) props.onFocus?.(next.focusedSurfaceId, leaf);
  };
  const stripFor=(leaf:number)=>[...strips.values()].find(strip=>strip.pane().tabs.some(tab=>tab.leaf===leaf));
  const localPointer=(strip:NonNullable<ReturnType<typeof stripFor>>,x:number)=>x-origin().x-strip.pane().rect[0]-(paneCount()>1?s:0)-strip.scroll();
  // Follow the original BufferTabs drag-center crossing policy. The native
  // canonical tree owns each actual move and retains the focused leaf.
  const reorderInStrip=(active:TabDrag,x:number,y:number)=>{
    const strip=stripFor(active.leaf);
    if(!strip)return false;
    const pane=strip.pane(),border=paneCount()>1?s:0,left=origin().x+pane.rect[0]+border,top=origin().y+pane.rect[1]+border;
    if(x<left||x>left+pane.rect[2]-2*border||y<top||y>top+chrome.height+36*s)return false;
    for(let pass=0;pass<pane.tabs.length;pass++){
      // Reordering publishes signals inside a batched pointer event. Read the
      // canonical result immediately, rather than a possibly stale render memo.
      const tabs=props.controller.frame.panes.find(pane=>pane.tabs.some(tab=>tab.leaf===active.leaf))?.tabs||[],index=tabs.findIndex(tab=>tab.leaf===active.leaf);
      if(index<0)break;
      const width=strip.widths.get(active.leaf)||chrome.width,start=tabs.slice(0,index).reduce((sum,tab)=>sum+(strip.widths.get(tab.leaf)||chrome.width),0);
      const center=x-left-strip.scroll()-active.grab+width*.5;
      const target=center>start+width?tabs[index+1]:center<start?tabs[index-1]:undefined;
      if(!target)break;
      props.controller.reorder(active.leaf,target.leaf,center<start);
    }
    return true;
  };
  const move = (event: EventPayload) => {
    const x = event.x || 0, y = event.y || 0;
    setDragPoint({x,y});
    if (!drag() && armed && Math.hypot(x - armed.x, y - armed.y) >= 5 * s) {
      const strip=stripFor(armed.leaf);strip?.slide.begin(armed.leaf,localPointer(strip,x),armed.grab);
      setDrag(armed); armed = undefined;
    }
    const active = drag(); if (!active) return;
    if (active.kind === "resize") {
      const horizontal = active.divider.axis === "horizontal";
      const pixels = horizontal ? x - active.x : y - active.y;
      props.controller.resize(active.divider.path, active.divider.gap, active.divider.ratio + pixels / active.divider.available);
    } else {
      const strip=stripFor(active.leaf);if(strip)strip.slide.move(localPointer(strip,x));
      if(reorderInStrip(active,x,y))setPreview(undefined);
      else setPreview(props.controller.dropPreview(x - origin().x, y - origin().y));
    }
  };
  const finish = (event: EventPayload) => {
    const active = drag();
    const source=active?.kind==="tab"?stripFor(active.leaf):undefined;
    if(source)source.slide.move(localPointer(source,event.x||0));
    if (active?.kind === "tab"&&!reorderInStrip(active,event.x||0,event.y||0)) {
      const next = props.controller.drop(active.leaf, (event.x || 0) - origin().x, (event.y || 0) - origin().y);
      if (next.focusedSurfaceId && next.focusedLeaf != null) props.onFocus?.(next.focusedSurfaceId, next.focusedLeaf);
    }
    source?.slide.end();
    armed = undefined; setDrag(undefined); setPreview(undefined);
  };
  const text: StyleDesc = { fontFamily: config.fontFamily, fontSize: d.tabFontSize * s,
    fontWeight: config.fontWeight, whiteSpace: "nowrap", userSelect: "none", pointerEvents: "none", color: c.fg };
  const row: StyleDesc = { display: "flex", flexDirection: "row", alignItems: "center" };
  const tabIcon=(tab:LayoutTab)=>tab.kind==="terminal"?{glyph:"\uf489",color:c.accent}:tab.kind==="welcome"?{glyph:"\uf015",color:c.accent}:tab.kind==="agent"?{glyph:"n",color:c.accent}:tab.kind==="git"?{glyph:"\uf126",color:c.accent}:tab.kind==="page"?{glyph:"\uf12e",color:c.accent}:namedIcons[tab.title.toLowerCase()]||extensionIcons[tab.title.split(".").pop()?.toLowerCase()||""]||{glyph:"\uf15b",color:"#c8c8c8"};
  function Pane(p: { hostKey: string }) {
    const initial = paneHosts().get(p.hostKey)!;
    const pane = createMemo(() => paneHosts().get(p.hostKey) || initial, initial, { equals: samePane });
    const paneFocused=createMemo(()=>pane().focused);
    const [bodyBounds,setBodyBounds]=createSignal<{x:number;y:number;width:number;height:number}>();
    createEffect(()=>{const bounds=bodyBounds();if(paneFocused()&&bounds)props.onContentBounds?.(bounds);});
    const widths=new Map<number,number>();let scrollX=0,scrollHost:HostElement|undefined;
    const [widthRevision,setWidthRevision]=createSignal(0),[slideRevision,setSlideRevision]=createSignal(0);
    const slide=new TabSlide<number>({reducedMotion:config.reducedMotion,onChange:()=>setSlideRevision(value=>value+1)});
    strips.set(p.hostKey,{pane,widths,scroll:()=>scrollX,slide});onCleanup(()=>{strips.delete(p.hostKey);slide.dispose();});
    createEffect(()=>{widthRevision();slide.layout(pane().tabs.map(tab=>({id:tab.leaf,width:widths.get(tab.leaf)||chrome.width})));});
    let newTabHost:HostElement|undefined;
    createEffect(()=>{const leaf=pane().leaf;if(newTabHost)props.registerNewHost?.(leaf,newTabHost);onCleanup(()=>props.registerNewHost?.(leaf,undefined));});
    const tabIds = createMemo(() => pane().tabs.map((tab) => tab.leaf), [], { equals: sameIds });
    function Tab(t: { leaf: number }) {
      const initialTab = pane().tabs.find((tab) => tab.leaf === t.leaf)!;
      const tab = createMemo(() => pane().tabs.find((tab) => tab.leaf === t.leaf) || initialTab, initialTab, { equals: sameTab });
      const selected = createMemo(() => t.leaf === pane().leaf);
      const [hovered, setHovered] = createSignal(false), [closeHovered, setCloseHovered] = createSignal(false);
      // Remounted tabs (workspace switch, font zoom) start at their measured
      // width instead of an estimate, so the strip never shifts a frame later.
      const measured = (kind: string, value: string) => `${kind}\0${config.fontFamily}\0${chrome.fontSize}\0${value}`;
      const [titleWidth, setTitleWidthSignal] = createSignal(measuredTabWidths.get(measured("title", initialTab.title)) ?? [...initialTab.title].length * chrome.fontSize * 0.6);
      const [iconWidth, setIconWidthSignal] = createSignal(measuredTabWidths.get(measured("icon", tabIcon(initialTab).glyph)) ?? chrome.iconSize);
      const setTitleWidth = (width: number) => { measuredTabWidths.set(measured("title", untrack(tab).title), width); setTitleWidthSignal(width); };
      const setIconWidth = (width: number) => { measuredTabWidths.set(measured("icon", untrack(icon).glyph), width); setIconWidthSignal(width); };
      const geometry = createMemo(() => tabChrome(s, tab().kind === "agent" || tab().kind === "welcome" || /\.md(?:own)?$/i.test(tab().title) ? Math.min(titleWidth(), 140 * s) : titleWidth(), iconWidth(), true).buffer);
      createEffect(()=>{tabGeometry.set(t.leaf,geometry());});
      const modified = () => !!tab().surfaceId && !!props.modified?.(tab().surfaceId!);
      const icon = () => tabIcon(tab());
      const slideOffset=()=>{slideRevision();return slide.offset(t.leaf);};
      let image: HostElement | undefined,tabHost:HostElement|undefined,revealQueued=false,disposed=false,suppressTabPress=false;
      const reveal=()=>{
        if(revealQueued)return;revealQueued=true;
        queueMicrotask(()=>{
          revealQueued=false;if(disposed||drag()?.kind==="tab"||!selected()||!tabHost?.mounted)return;
          tabHost.root?.mutations.flushMutations();tabHost.scrollIntoView();
          // The final tab and its inline add button travel together. Revealing
          // the button after a new tab's layout keeps repeated clicks usable.
          if(pane().tabs.at(-1)?.leaf===t.leaf&&newTabHost?.mounted)newTabHost.scrollIntoView();
        });
      };
      createEffect(()=>{if(selected())reveal();});
      onCleanup(()=>{disposed=true;widths.delete(t.leaf);tabGeometry.delete(t.leaf);props.registerTabHost?.(t.leaf,undefined);});
      onMount(() => queueMicrotask(() => { if (!image?.mounted) return; image.root?.mutations.flushMutations(); image.setImage(cachedNeoismIcon ||= readFileSync(neoismIcon)); }));
      return <div ref={host=>{tabHost=host;props.registerTabHost?.(t.leaf,host);}} tabIndex={0} role="tab" testId={`pane-tab:${t.leaf}`} aria-label={tab().title} aria-selected={selected()} data-keyboard-focused={props.keyboardTab===t.leaf}
        onLayout={event=>{const width=event.width||geometry().width;if(widths.get(t.leaf)!==width){widths.set(t.leaf,width);setWidthRevision(value=>value+1);}props.onTabLayout?.(t.leaf,{x:event.x||0,y:event.y||0,width:width,height:event.height||0});if(selected())reveal();}}
        onMouseEnter={() => setHovered(true)} onMouseLeave={() => setHovered(false)}
        onMouseDown={(event) => {
          if(suppressTabPress||event.button!==undefined&&event.button!==0||!pane().tabs.some(tab=>tab.leaf===t.leaf))return;
          focus(t.leaf);const border=paneCount()>1?s:0;
          const left=origin().x+pane().rect[0]+border+scrollX+slide.position(t.leaf);
          armed={kind:"tab",leaf:t.leaf,x:event.x||0,y:event.y||0,grab:Math.max(0,Math.min(widths.get(t.leaf)||geometry().width,(event.x||0)-left))};
        }}
        onMouseMove={move} onMouseUp={finish}
        style={{ position: "relative", left:slideOffset(),width: geometry().width, height: chrome.height, flexShrink: 0, cursor: "pointer", borderTopLeftRadius: selected() || hovered() ? 3 * s : 0,
          borderTopRightRadius: selected() || hovered() ? 3 * s : 0, backgroundColor: selected() ? c.bg : undefined,opacity:drag()?.kind==="tab"&&(drag() as TabDrag).leaf===t.leaf ? 0 : 1 }}>
        <motion.div initial={false} animate={{ opacity: (hovered()||props.keyboardTab===t.leaf) && !selected() ? 1 : 0 }} transition={{ duration: 0.15, ease: easeOutCubicBezier }}
          style={{ position: "absolute", left: 0, right: 0, top: 0, height: chrome.height, borderTopLeftRadius: 3 * s, borderTopRightRadius: 3 * s, backgroundColor: c.hover + "b8", pointerEvents: "none" }} />
        <text onLayout={(event) => { if (event.width) setTitleWidth(event.width); }} style={{ ...text, position: "absolute", opacity: 0, left: 0, top: 0 }}>{tab().title}</text>
        <Show when={tab().kind === "agent"} fallback={<text testId={`pane-tab-icon:${t.leaf}`} onLayout={(event) => { if (event.width) setIconWidth(Math.max(chrome.iconSize, event.width)); }}
          style={{ ...text, position: "absolute", left: chrome.padding, top: 0, lineHeight: chrome.height, fontFamily: config.iconFontFamily, fontSize: chrome.iconSize, color: selected() ? icon().color : inactiveTabIcon(icon().color) }}>{icon().glyph}</text>}>
          <img ref={image} testId={`pane-tab-icon:${t.leaf}`} objectFit="contain" alt="Neoism" style={{ position: "absolute", left: chrome.padding, top: (chrome.height - chrome.iconSize) / 2, width: chrome.iconSize, height: chrome.iconSize, pointerEvents: "none" }} />
        </Show>
        <div style={{position:"absolute",left:chrome.padding+geometry().iconWidth+chrome.iconGap,top:0,pointerEvents:"none"}}><HoverTabTitle label={tab().title} width={geometry().titleWidth} naturalWidth={titleWidth()} height={chrome.height} scale={s} hovered={hovered()} reducedMotion={config.reducedMotion} style={{...text,lineHeight:chrome.height,color:selected()?c.fg:c.muted}}/></div>
        <div role="button" aria-label={`Close ${tab().title}`} testId={`pane-close:${t.leaf}`}
          onMouseEnter={() => setCloseHovered(true)} onMouseLeave={() => setCloseHovered(false)}
          onMouseDown={() => { suppressTabPress=true;queueMicrotask(()=>{suppressTabPress=false;});armed = undefined; const id = tab().surfaceId; if (id) props.onClose(id, t.leaf); }}
          style={{ position: "absolute", left: (geometry().closeCenter || 0) - chrome.closeHitSize / 2, top: (chrome.height - chrome.closeHitSize) / 2, width: chrome.closeHitSize, height: chrome.closeHitSize, cursor: "pointer", ...row, justifyContent: "center" }}>
          <Show when={modified()} fallback={<text style={{ ...text, color: closeHovered() ? c.red : c.muted, fontSize: chrome.fontSize * (closeHovered() ? 1.16 : 1), opacity: selected() || hovered() || closeHovered() ? 1 : 0 }}>×</text>}>
            <div testId="unsaved-buffer" aria-label="Unsaved changes" style={{ width: 12 * s, height: 12 * s, borderRadius: 6 * s, backgroundColor: c.yellow + "38", ...row, justifyContent: "center", pointerEvents: "none" }}><div style={{ width: 8 * s, height: 8 * s, borderRadius: 4 * s, backgroundColor: c.yellow }} /></div>
          </Show>
        </div>
      </div>;
    }
    return <div testId={`pane:${pane().leaf}`}
      style={{ position: "absolute", left: pane().rect[0], top: pane().rect[1], width: pane().rect[2], height: pane().rect[3],
        display: "flex", flexDirection: "column", overflow: "hidden", minWidth: 0, minHeight: 0,
        borderRadius: paneCount() > 1 ? d.frameRadius * s : 0,
        borderWidth: paneCount() > 1 ? s : 0, borderColor: pane().focused ? c.border : c.surface }}>
      <div role="tablist" aria-label="Pane tabs" testId={`pane-tabs:${pane().leaf}`}
        style={{ position:"relative", height: chrome.height, flexShrink: 0, backgroundColor: c.surface, overflow: "hidden", minWidth: 0,
          borderTopLeftRadius: 6 * s, borderTopRightRadius: 6 * s, borderBottomWidth: 1, borderColor: c.border }}>
        <div ref={host=>scrollHost=host} testId={`pane-tab-scroll:${pane().leaf}`} onScroll={event=>{consumeTabScroll(event);const content=pane().tabs.reduce((sum,tab)=>sum+(widths.get(tab.leaf)||tabGeometry.get(tab.leaf)?.width||chrome.width),0)+(props.onNewWelcome?chrome.newWidth:0);const next=verticalTabScroll(event,scrollX,Math.max(0,pane().rect[2]-(paneCount()>1?2*s:0)),content);if(next!==undefined&&next!==scrollX&&scrollHost){consumeTabScroll(event,true);scrollX=next;renderer.scrollTo?.(scrollHost.id,next,0);props.onTabScroll?.();}}} onScrollChange={event=>{scrollX=event.scrollX||0;const active=drag();if(active?.kind==="tab"&&pane().tabs.some(tab=>tab.leaf===active.leaf))slide.move(dragPoint().x-origin().x-pane().rect[0]-(paneCount()>1?s:0)-scrollX);props.onTabScroll?.();}} style={{...row,position:"absolute",left:0,top:0,right:0,height:chrome.height,overflowX:"scroll",overflowY:"hidden",minWidth:0}}>
          <For each={tabIds()}>{(leaf) => <Tab leaf={leaf} />}</For>
          <Show when={props.onNewWelcome}><div ref={host=>{newTabHost=host;}} tabIndex={0} role="button" testId={`pane-new-welcome:${pane().leaf}`} aria-label="New Home" data-keyboard-focused={props.keyboardNewPane===pane().leaf} onClick={() => { focus(pane().leaf); props.onNewWelcome?.(pane().leaf); }}
            onLayout={event=>props.onNewLayout?.(pane().leaf,{x:event.x||0,y:event.y||0,width:event.width||0,height:event.height||0})}
            style={{ ...row, justifyContent: "center", width: chrome.newWidth, height: chrome.height, flexShrink: 0, cursor: "pointer",backgroundColor:props.keyboardNewPane===pane().leaf?c.accent+"1a":undefined, hover: { backgroundColor: c.accent + "0f" } }}>
            <text style={{ ...text, fontFamily: config.iconFontFamily, fontSize: chrome.iconSize, color: c.muted }}>{chrome.newIcon}</text>
          </div></Show>
        </div>
      </div>
      <div testId={`pane-content:${pane().leaf}`} onLayout={event=>setBodyBounds({x:event.x||0,y:event.y||0,width:event.width||0,height:event.height||0})} onMouseDown={() => {if(!paneFocused())focus(pane().leaf);}} style={{ display: "flex", flexDirection: "column", flexGrow: 1, minHeight: 0, minWidth: 0, position: "relative" }}>
        <Show when={pane().surfaceId ? `${pane().leaf}:${pane().surfaceId}` : undefined} keyed>{(_identity) => props.renderSurface(pane().surfaceId!, pane().leaf, paneFocused)}</Show>
      </div>
    </div>;
  }
  function DraggedTab(){
    const active=()=>drag() as TabDrag,tab=()=>frame().leaves.find(tab=>tab.leaf===active().leaf)!;
    const geometry=()=>tabGeometry.get(active().leaf)||chrome;
    let image:HostElement|undefined;
    onMount(()=>queueMicrotask(()=>{if(!image?.mounted)return;image.root?.mutations.flushMutations();image.setImage(cachedNeoismIcon ||= readFileSync(neoismIcon));}));
    return <div testId="buffer-tab-drag-ghost" style={{position:"absolute",left:dragPoint().x-active().grab,top:dragPoint().y-chrome.height/2,width:geometry().width,height:chrome.height,backgroundColor:c.bg,borderTopLeftRadius:3*s,borderTopRightRadius:3*s,pointerEvents:"none",overflow:"hidden"}}>
      <Show when={tab().kind==="agent"} fallback={<text style={{...text,position:"absolute",left:chrome.padding,top:0,lineHeight:chrome.height,fontFamily:config.iconFontFamily,fontSize:chrome.iconSize,color:tabIcon(tab()).color}}>{tabIcon(tab()).glyph}</text>}>
        <img ref={image} objectFit="contain" alt="Neoism" style={{position:"absolute",left:chrome.padding,top:(chrome.height-chrome.iconSize)/2,width:chrome.iconSize,height:chrome.iconSize,pointerEvents:"none"}}/>
      </Show>
      <text style={{...text,position:"absolute",left:chrome.padding+geometry().iconWidth+chrome.iconGap,top:0,lineHeight:chrome.height,width:geometry().titleWidth,textOverflow:"ellipsis"}}>{tab().title}</text>
      <Show when={!!tab().surfaceId&&props.modified?.(tab().surfaceId!)} fallback={<text style={{...text,position:"absolute",left:(geometry().closeCenter||0)-chrome.closeHitSize/2,top:0,width:chrome.closeHitSize,lineHeight:chrome.height,textAlign:"center",color:c.muted}}>×</text>}>
        <div aria-label="Unsaved changes" style={{position:"absolute",left:(geometry().closeCenter||0)-6*s,top:(chrome.height-12*s)/2,width:12*s,height:12*s,borderRadius:6*s,backgroundColor:c.yellow+"38",...row,justifyContent:"center",pointerEvents:"none"}}><div style={{width:8*s,height:8*s,borderRadius:4*s,backgroundColor:c.yellow}}/></div>
      </Show>
    </div>;
  }
  return <div testId="pane-layout" onMouseMove={move} onMouseUp={finish}
    onLayout={(event) => { setOrigin((old) => old.x === event.x && old.y === event.y ? old : { x: event.x || 0, y: event.y || 0 }); if (!retiring()) { lastPaneArea = { width: event.width || 0, height: event.height || 0, scale: s }; props.controller.layout(lastPaneArea.width, lastPaneArea.height, s); } }}
    style={{ position: "relative", flexGrow: 1, minWidth: 0, minHeight: 0, overflow: "hidden" }}>
    <Show when={frame().panes.length} fallback={props.empty}>
      <For each={paneIds()}>{(hostKey) => <Pane hostKey={hostKey} />}</For>
      <For each={frame().dividers.map((d) => `${d.path.join(".")}:${d.gap}`)}>{(key) => {
        const first = frame().dividers.find((d) => `${d.path.join(".")}:${d.gap}` === key)!;
        const divider = () => frame().dividers.find((d) => `${d.path.join(".")}:${d.gap}` === key) || first;
        return <div role="separator" testId={`pane-divider:${key}`}
          aria-orientation={divider().axis === "horizontal" ? "vertical" : "horizontal"}
          onMouseDown={(event) => { if (event.button !== undefined && event.button !== 0) return; setDrag({ kind: "resize", divider: divider(), x: event.x || 0, y: event.y || 0 }); }}
          onMouseMove={move} onMouseUp={finish}
          style={{ position: "absolute", left: divider().rect[0], top: divider().rect[1], width: divider().rect[2], height: divider().rect[3],
            cursor: divider().axis === "horizontal" ? "col-resize" : "row-resize", hover: { backgroundColor: c.border } }} />;
      }}</For>
      <Show when={preview()?.rect}>{(rect) => <div style={{ position: "absolute", left: rect()[0], top: rect()[1], width: rect()[2], height: rect()[3],
        backgroundColor: c.blue + "33", borderColor: c.blue, borderWidth: 2 * s, borderRadius: 8 * s, pointerEvents: "none" }} />}</Show>
    </Show>
    <Show when={drag()}><anchored fill="window" deferred={true} priority={4} occlude={true}
      style={{ backgroundColor: "transparent" }}>
      <div testId="pane-drag-layer" onMouseMove={move} onMouseUp={finish}
        style={{ position: "absolute", left: 0, top: 0, width: "100%", height: "100%", pointerEvents: "auto",
          cursor: drag()?.kind === "resize" ? (drag() as ResizeDrag).divider.axis === "horizontal" ? "col-resize" : "row-resize" : "grabbing" }} />
      <Show when={drag()?.kind==="tab"&&frame().leaves.some(tab=>tab.leaf===(drag() as TabDrag).leaf)}><DraggedTab/></Show>
    </anchored></Show>
  </div>;
}
