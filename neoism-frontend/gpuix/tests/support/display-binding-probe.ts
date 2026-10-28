import * as addon from "@gpuix/native";
const selected=addon as typeof addon&{__neoismDisplayBinding:{artifact:string;sha256:string};__neoismDisplayBindingEmbedded:boolean;testSupportEnabled:()=>boolean};
const prototype=addon.GpuixRenderer.prototype as unknown as {requestFrameWake?:unknown};
if(typeof prototype.requestFrameWake!=="function")throw new Error("Selected binding has no native display frame API");
if(typeof selected.testSupportEnabled!=="function"||selected.testSupportEnabled()!==false)throw new Error("Selected binding is not a production display build");
if(selected.__neoismDisplayBinding?.sha256!==process.env.NEOISM_EXPECT_BINDING)throw new Error("Source and compiled frontend selected different binding bytes");
if(selected.__neoismDisplayBindingEmbedded!==(process.env.NEOISM_EXPECT_EMBEDDED==="1"))throw new Error("Compiled frontend did not embed the verified addon");
console.log(JSON.stringify({artifact:selected.__neoismDisplayBinding.artifact,sha256:selected.__neoismDisplayBinding.sha256,embedded:selected.__neoismDisplayBindingEmbedded}));
