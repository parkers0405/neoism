/** Replace the feature entry before dependency traversal, so normal packages
 * contain neither device transports nor the experimental panel. */
export function devicePreviewEnabled(env: NodeJS.ProcessEnv = process.env): boolean {
  const value = env.NEOISM_GPUIX_DEVICE_PREVIEW;
  if (value !== undefined && value !== "0" && value !== "1") throw new Error("NEOISM_GPUIX_DEVICE_PREVIEW must be 0 or 1");
  return value === "1";
}

export function devicePreviewPlugin(enabled = devicePreviewEnabled()) {
  return {
    name: "optional-device-preview",
    setup(build: { onLoad: (options: { filter: RegExp }, callback: () => { loader: "tsx"; contents: string }) => void }) {
      build.onLoad({ filter: /[/\\]device[/\\]feature\.tsx$/ }, () => ({
        loader: "tsx",
        contents: enabled
          ? 'export {DevicePreviewSlot,DevicePreviewButton} from "./DevicePreview"; export const devicePreviewEnabled = true;'
          : 'export const devicePreviewEnabled = false; export const DevicePreviewSlot = () => null; export const DevicePreviewButton = () => null;',
      }));
    },
  };
}