import {lazy, Show} from "solid-js";
import type {DevicePanelProps} from "./types";

// Development entry. Packaging replaces this entire module with an enabled
// entry or an inert stub before traversing device dependencies.
export const devicePreviewEnabled = process.env.NEOISM_GPUIX_DEVICE_PREVIEW === "1";
const Preview = lazy(() => import("./DevicePreview"));
export function DevicePreviewSlot(props: DevicePanelProps) {
  return <Show when={devicePreviewEnabled && props.visible}><Preview {...props}/></Show>;
}
export {DevicePreviewButton} from "./DevicePreviewButton";