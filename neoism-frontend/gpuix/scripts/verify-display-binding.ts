import {createRequire} from "node:module";
const path=process.argv[2];if(!path)throw new Error("Pass the candidate native .node artifact");
const binding=createRequire(import.meta.url)(path);
if(typeof binding.GpuixRenderer!=="function"||typeof binding.GpuixRenderer.prototype.requestFrameWake!=="function"||typeof binding.GpuixRenderer.prototype.setFrameWakeCallback!=="function")throw new Error("The candidate addon has no native display frame API");
if(typeof binding.testSupportEnabled!=="function"||binding.testSupportEnabled()!==false)throw new Error("The candidate addon enables test-support or cannot prove its production display pacing");
if(typeof binding.inputCaretGeometrySupported!=="function"||binding.inputCaretGeometrySupported()!==true)throw new Error("The candidate native text editor filters out caretGeometry events");
console.log("Native GPUIX addon verified: display-frame API and test-support disabled (no window created).");
