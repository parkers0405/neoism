import {describe,expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {devicePreviewEnabled,devicePreviewPlugin} from "../scripts/device-preview-build";

describe("optional device preview build",()=>{
  test("is strictly opt-in",()=>{
    expect(devicePreviewEnabled({})).toBe(false);
    expect(devicePreviewEnabled({NEOISM_GPUIX_DEVICE_PREVIEW:"0"})).toBe(false);
    expect(devicePreviewEnabled({NEOISM_GPUIX_DEVICE_PREVIEW:"1"})).toBe(true);
    expect(()=>devicePreviewEnabled({NEOISM_GPUIX_DEVICE_PREVIEW:"true"})).toThrow("must be 0 or 1");
  });
  test("default package removes the feature before traversing native/video dependencies",async()=>{
    const result=await Bun.build({entrypoints:[fileURLToPath(new URL("../src/device/feature.tsx",import.meta.url))],target:"bun",plugins:[devicePreviewPlugin(false)]});
    expect(result.success).toBe(true);
    expect(result.outputs).toHaveLength(1);
    const source=await result.outputs[0].text();
    expect(source).toContain("devicePreviewEnabled = false");
    for(const forbidden of ["ffmpeg","DevicePanel","DeviceSession","DevicePreviewButton.tsx","iphone-18-pro","ipad-pro-13-m5","deviceFinishes","bun:ffi","node:child_process"])expect(source).not.toContain(forbidden);
  });
  test("enabled package includes the actual panel, native viewport and lazy decoder",async()=>{
    const result=await Bun.build({entrypoints:[fileURLToPath(new URL("../src/device/feature.tsx",import.meta.url))],target:"bun",plugins:[devicePreviewPlugin(true)],external:["@gpuix/*","solid-js"]});
    expect(result.success).toBe(true);
    const source=(await Promise.all(result.outputs.map(output=>output.text()))).join("\n");
    for(const included of ["device-preview-panel","createDeviceViewport","hitDeviceViewport","device_viewport","ffmpeg","iphone-18-pro","iphone-18-pro-max","iphone-duo","ipad-pro-13-m5"])expect(source).toContain(included);
  });
});
