import type {AppConfig} from "../config";
import {DeviceIcon} from "./DeviceIcon";

export function DevicePreviewButton(props: {config: AppConfig; selected: boolean; onClick: () => void}) {
  const c = props.config.theme, s = props.config.scale;
  return <div role="button" tabIndex={0} testId="device-preview-toggle" aria-label="Toggle device preview (Alt+Shift+D)"
    aria-pressed={props.selected} onClick={props.onClick} onKeyDown={event => {
      if (event.key === "enter" || event.key === "space") { event.preventDefault(); event.stopPropagation(); props.onClick(); }
    }} style={{display:"flex",alignItems:"center",justifyContent:"center",width:36*s,height:"100%",cursor:"pointer",
      borderLeftWidth:1,borderColor:c.border,backgroundColor:props.selected?c.surface:c.bg,hover:{backgroundColor:c.surface}}}>
    <DeviceIcon name="phone" size={15*s} color={props.selected?c.fg:c.dim}/>
  </div>;
}