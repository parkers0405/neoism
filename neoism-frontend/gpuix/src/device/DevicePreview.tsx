import {Show} from "solid-js";
import {DevicePanel} from "./DevicePanel";
import type {DevicePanelProps} from "./types";
export {DevicePreviewButton} from "./DevicePreviewButton";

export function DevicePreviewSlot(props: DevicePanelProps) {
  return <Show when={props.visible}><DevicePanel {...props}/></Show>;
}
export default DevicePreviewSlot;