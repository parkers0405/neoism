import type {StyleDesc} from "@gpuix/solid";

// Original outline icons on the same 24-unit, round-stroke grid as the
// reference controls. These are vector shapes, not font-dependent glyphs.
const paths={
  back:'<path d="m14 6-6 6 6 6"/>',
  close:'<path d="m6 6 12 12M18 6 6 18"/>',
  chevron:'<path d="m7 10 5 5 5-5"/>',
  home:'<path d="m3 10 9-7 9 7M5 9v12h14V9M9 21v-8h6v8"/>',
  phone:'<rect x="6.5" y="2.5" width="11" height="19" rx="2"/><path d="M10 18.5h4"/>',
  box:'<path d="m12 3 8.5 4.8v8.4L12 21l-8.5-4.8V7.8L12 3Zm-8.5 4.8L12 13l8.5-5.2M12 13v8M7.8 5.4l8.5 5"/>',
  reset:'<path d="M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5"/>',
  rotate:'<path d="M3 4v5h5M3.7 8A8.5 8.5 0 1 1 4 16M12 7v5l3 2"/>',
  camera:'<path d="M8 5 6 8H3v12h18V8h-3l-2-3H8Z"/><circle cx="12" cy="13" r="3.2"/>',
  sun:'<circle cx="12" cy="12" r="3.5"/><path d="M12 2v2m0 16v2M2 12h2m16 0h2M5 5l1.5 1.5m11 11L19 19M5 19l1.5-1.5m11-11L19 5"/>',
  moon:'<path d="M20.5 14.5A8.8 8.8 0 0 1 9.5 3.5a8.8 8.8 0 1 0 11 11Z"/>',
  settings:'<path d="M3 6h7m4 0h7M3 12h3m4 0h11M3 18h11m4 0h3"/><path d="M10 3v6M6 9v6m8 0v6"/>',
  more:'<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',
  keyboard:'<rect x="2.5" y="5" width="19" height="14" rx="2"/><path d="M6 9h.1M10 9h.1M14 9h.1M18 9h.1M6 12h.1M10 12h.1M14 12h.1M18 12h.1M7 15h10"/>',
  connect:'<path d="m9 15 6-6m-7 3-2 2a3.5 3.5 0 0 0 5 5l2-2m-2-10 2-2a3.5 3.5 0 0 1 5 5l-2 2"/>',
  check:'<path d="m5 12 4 4L19 6"/>',
  front:'<rect x="6" y="2.5" width="12" height="19" rx="2"/><path d="M10 5h4"/>',
  rear:'<rect x="6" y="2.5" width="12" height="19" rx="2"/><circle cx="10" cy="7" r="1.2"/><circle cx="14" cy="7" r="1.2"/><circle cx="10" cy="11" r="1.2"/>',
  closed:'<rect x="8" y="3" width="9" height="18" rx="1.5"/><path d="M5 4v16"/>',
  book:'<path d="m12 5-8-2v16l8 2 8-2V3l-8 2Zm0 0v16"/>',
  open:'<rect x="2.5" y="4" width="19" height="16" rx="1.5"/><path d="M12 4v16"/>',
  square:'<rect x="5" y="5" width="14" height="14" rx="1.5"/>',
  disconnect:'<path d="m9 15-3 3a3.5 3.5 0 0 1-5-5l3-3m11-1 3-3a3.5 3.5 0 0 1 5 5l-3 3M3 3l18 18"/>',
};
export type DeviceIconName=keyof typeof paths;
const sources=Object.fromEntries(Object.entries(paths).map(([name,body])=>[name,`data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`)}`])) as Record<DeviceIconName,string>;
export function DeviceIcon(props:{name:DeviceIconName;size?:number;color:string;style?:StyleDesc}) {
  return <svg src={sources[props.name]} style={{width:props.size||16,height:props.size||16,color:props.color,flexShrink:0,pointerEvents:"none",...props.style}}/>;
}