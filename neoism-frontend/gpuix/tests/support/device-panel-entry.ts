// The recording renderer supplies the optional API, so the preload may use
// the ordinary production binding without requiring an experimental rebuild.
await import("@gpuix/native");
process.env.NEOISM_GPUIX_DEVICE_PREVIEW=process.env.NEOISM_DEVICE_UI_TEST||"0";
await import("./device-panel-check");
export {};