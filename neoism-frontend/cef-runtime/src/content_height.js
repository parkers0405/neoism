// Executed only as neoism:host. No DOM wrapping, layout mutation, or privileged bridge.
(()=>{
  if(window.__neoismArtifactMeasure) window.__neoismArtifactMeasure();
  const epoch=__EPOCH__;
  let stopped=false, queued=false, ready=false, last=null;
  const number=v=>parseFloat(v)||0;
  // A normal-flow body's viewport min-height is just an empty page floor, not
  // content. Keep genuine viewport layouts (flex/grid/positioned children) bounded.
  const passiveFloor=(s,e,k)=>{
    if(e!==document.body||!['min-height','min-block-size'].includes(k))return false;
    const v=s.getPropertyValue(k);
    if(!/(?:\d|\.)\s*(?:dvh|svh|lvh|vh|vmin|vmax)\b|%/i.test(v))return false;
    if(!['block','flow-root'].includes(getComputedStyle(e).display))return false;
    const nodes=e.querySelectorAll('*');if(nodes.length>20000)return false;
    return !Array.from(nodes).some(n=>n.getClientRects().length&&['absolute','fixed','sticky'].includes(getComputedStyle(n).position));
  };
  // Viewport-dependent vertical sizing has no unique intrinsic fixed point. Reserve
  // the parent's maximum viewport instead of feeding viewport height back into itself.
  const dependent=()=>{
    const pattern=/(?:\d|\.)\s*(?:dvh|svh|lvh|vh|vmin|vmax)\b/i;
    const viewportPercent=e=>{
      if(e===document.body||e===document.documentElement)return true;
      const s=getComputedStyle(e);
      if(s.position==='fixed')return true;
      if(s.position!=='absolute')return false;
      // Percentages inside an authored containing block are not viewport-sized.
      for(let p=e.parentElement;p;p=p.parentElement){
        const ps=getComputedStyle(p);
        if(ps.position!=='static'||ps.transform!=='none'||ps.perspective!=='none'||ps.filter!=='none')return false;
      }
      return true;
    };
    const size=(s,e)=>{
      const cs=getComputedStyle(e);
      if(/^(SCRIPT|STYLE|TEMPLATE|NOSCRIPT|HEAD)$/.test(e.tagName)||!e.getClientRects().length||cs.display==='none'||cs.visibility==='hidden'||cs.visibility==='collapse'||cs.contentVisibility==='hidden')return false;
      return ['height','min-height','max-height','block-size','min-block-size','max-block-size','top','bottom'].some(k=>{
        if((k==='top'||k==='bottom')&&cs.position==='static')return false;
        if(passiveFloor(s,e,k))return false;
        const v=s.getPropertyValue(k);
        return pattern.test(v)||(/%/.test(v)&&viewportPercent(e));
      });
    };
    const rules=rs=>Array.from(rs).some(r=>{
      if(r.style){
        try{return Array.from(document.querySelectorAll(r.selectorText)).some(e=>size(r.style,e));}
        catch(_){return false;}
      }
      if(r.conditionText&&r.constructor.name==='CSSMediaRule'&&!matchMedia(r.conditionText).matches)return false;
      if(r.conditionText&&r.constructor.name==='CSSSupportsRule'&&!CSS.supports(r.conditionText))return false;
      return r.cssRules?rules(r.cssRules):false;
    });
    for(const sheet of document.styleSheets){try{if(rules(sheet.cssRules))return true;}catch(_){} }
    return Array.from(document.querySelectorAll('[style]')).some(e=>size(e.style,e));
  };
  // An explicitly sized body is authored content too. Only accept declared
  // vertical sizes; never mistake its computed viewport floor for authored height.
  const bodySized=body=>{
    const sized=s=>['height','min-height','block-size','min-block-size'].some(k=>{
      const v=s.getPropertyValue(k);return v && !passiveFloor(s,body,k) && !/^(auto|initial|inherit|unset|revert|normal)$/.test(v);
    });
    if(sized(body.style))return true;
    const rules=rs=>Array.from(rs).some(r=>{
      if(r.style){try{return body.matches(r.selectorText)&&sized(r.style)}catch(_){return false;}}
      if(r.conditionText && r.constructor.name==='CSSMediaRule' && !matchMedia(r.conditionText).matches)return false;
      return r.cssRules?rules(r.cssRules):false;
    });
    for(const sheet of document.styleSheets){try{if(rules(sheet.cssRules))return true;}catch(_){}}
    return false;
  };
  const measure=()=>{
    const body=document.body;if(!body)return 1;
    const root=document.querySelector('[data-neoism-artifact-root]')||body;
    let bottom=0, visited=0;
    const visit=e=>{
      if(++visited>20000)return;
      if(e.nodeType===Node.TEXT_NODE){
        if(!e.textContent.trim())return;
        const range=document.createRange();range.selectNodeContents(e);
        for(const r of range.getClientRects())bottom=Math.max(bottom,r.bottom+window.scrollY);
        return;
      }
      if(e.nodeType!==Node.ELEMENT_NODE||/^(SCRIPT|STYLE|TEMPLATE|NOSCRIPT|HEAD)$/.test(e.tagName))return;
      const s=getComputedStyle(e);
      if(s.display==='none'||s.visibility==='hidden'||s.visibility==='collapse'||s.contentVisibility==='hidden')return;
      const r=e.getBoundingClientRect();
      // Body's rectangle can be viewport-floored: never use it as content.
      if(e!==body && r.height>0){
        bottom=Math.max(bottom,r.bottom+(s.position==='fixed'?0:window.scrollY)+number(s.marginBottom));
      }
      // Internal scrolling/clipping belongs to this authored box, not the outer row.
      if(e!==body && /^(auto|scroll|hidden|clip)$/.test(s.overflowY))return;
      for(const child of e.childNodes)visit(child);
    };
    visit(root);
    if(root===body && bodySized(body))bottom=Math.max(bottom,body.getBoundingClientRect().bottom+window.scrollY);
    const bs=getComputedStyle(body);
    if(root===body && bodySized(body))bottom+=number(bs.marginBottom);
    else bottom+=number(bs.paddingBottom)+number(bs.marginBottom);
    bottom=Math.max(bottom,number(bs.paddingTop)+number(bs.paddingBottom)+number(bs.marginTop)+number(bs.marginBottom),1);
    if(dependent())return 640;
    return Math.min(8192,Math.ceil(bottom*64)/64);
  };
  const report=initial=>{
    if(stopped)return;
    const height=measure();
    if(initial||height!==last){last=height;console.debug((initial?'neoism-ready:':'neoism-height:')+epoch+':'+height);}
  };
  const schedule=()=>{
    if(stopped||!ready||queued)return;
    queued=true;requestAnimationFrame(()=>{queued=false;report(false)});
  };
  const ro=new ResizeObserver(schedule);
  const observe=()=>{
    ro.disconnect();let n=0;
    if(document.body)for(const e of [document.body,...document.body.querySelectorAll('*')]){
      if(++n>20000)break;ro.observe(e);
    }
  };
  const mo=new MutationObserver(records=>{if(records.some(r=>r.type==='childList'))observe();schedule()});
  mo.observe(document.documentElement,{subtree:true,childList:true,attributes:true,characterData:true});
  observe();
  document.addEventListener('load',schedule,true);
  if(document.fonts){document.fonts.addEventListener('loadingdone',schedule);document.fonts.ready.then(schedule);}
  window.__neoismArtifactMeasure=()=>{
    stopped=true;ro.disconnect();mo.disconnect();document.removeEventListener('load',schedule,true);
    if(document.fonts)document.fonts.removeEventListener('loadingdone',schedule);
  };
  requestAnimationFrame(()=>requestAnimationFrame(()=>{if(!stopped){ready=true;report(true);}}));
})();
