import { createMemo, Show } from "solid-js";
import type { AppConfig } from "../config";
import { providerLogoAtlasBase64, providerLogos } from "../design/generated";

export function providerLogoIndex(model: string | null | undefined): number | undefined {
  const slash = model?.trim().indexOf("/") ?? -1;
  return slash < 0 ? undefined : providerLogos[model!.trim().slice(0, slash).trim().toLowerCase()];
}
const rgb = (color: string) => [1,3,5].map(offset => parseInt(color.slice(offset,offset+2),16));
const hex = (channels: number[]) => "#" + channels.map(c => c.toString(16).padStart(2,"0")).join("");
const luminance = (channels: number[]) => channels.map(c => c/255).map(c => c <= 0.04045 ? c/12.92 : ((c+0.055)/1.055)**2.4).reduce((sum,c,i) => sum+c*[0.2126,0.7152,0.0722][i],0);
/** Same composer_color/is_dark/readable_accent policy as the original Rust. */
export function providerLogoColor(index: number, theme: AppConfig["theme"]): string {
  const background = rgb(theme.bg), dark = background.reduce((sum,c,i) => sum+c*[0.299,0.587,0.114][i],0) < 128;
  if (index === 1) return "#d97757";
  if (index === 0 || index === 2) return dark ? "#ffffff" : "#000000";
  if (dark) return theme.blue;
  const bg = luminance(background), contrast = (channels: number[]) => { const l=luminance(channels); return (Math.max(l,bg)+0.05)/(Math.min(l,bg)+0.05); };
  let shade = rgb(theme.blue); if (contrast(shade) >= 2.6) return theme.blue;
  for (let i=0;i<12;i++) { shade=shade.map(c => Math.floor(c*0.82)); if (contrast(shade)>=2.6) return hex(shade); }
  return theme.fg;
}
export function providerLogoSvg(index: number) {
  const x=index*64;
  return `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="64" height="64" viewBox="${x} 0 64 64"><defs><mask id="provider" maskUnits="userSpaceOnUse" x="${x}" y="0" width="64" height="64" style="mask-type:alpha"><image x="0" y="0" width="448" height="64" xlink:href="data:image/png;base64,${providerLogoAtlasBase64}"/></mask></defs><rect x="${x}" y="0" width="64" height="64" fill="white" mask="url(#provider)"/></svg>`;
}
/** Exact 17px original atlas mark, centered against the chip's em box. */
export function ProviderLogo(props: { model?: string | null; config: AppConfig }) {
  const index=createMemo(()=>providerLogoIndex(props.model));
  return <Show when={index() !== undefined}><svg testId="composer-provider-logo" source={providerLogoSvg(index()!)} style={{ width:17*props.config.scale,height:17*props.config.scale,flexShrink:0,color:providerLogoColor(index()!,props.config.theme),pointerEvents:"none" }} /></Show>;
}
