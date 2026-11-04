const {createHash}=require("node:crypto");
const {readFileSync,writeFileSync}=require("node:fs");
const {join,resolve}=require("node:path");
const hash=value=>createHash("sha256").update(value).digest("hex");
/** Checked, idempotent compatibility patch for the exact installed release. */
function patchSolidHost(root=resolve(__dirname,"..")){
 const pin=JSON.parse(readFileSync(join(root,"native/solid-host-pin.json"),"utf8"));
 const base=join(root,"node_modules/@gpuix/solid"),version=JSON.parse(readFileSync(join(base,"package.json"),"utf8")).version;
 if(version!==pin.version)throw new Error("The GPUIX Solid host patch needs review for the installed version");
 const path=join(base,"dist/host.js"),source=readFileSync(path,"utf8"),digest=hash(source);
 if(digest===pin.patchedSha256)return false;
  if(digest!==pin.originalSha256&&digest!==pin.previousPatchedSha256&&digest!==pin.focusPatchedSha256&&digest!==pin.devicePatchedSha256)throw new Error("The GPUIX Solid host has unrecognized edits; refusing to overwrite it");
 const anchor="    removeFromParent(node);\n    const index = anchor ? parent.children.indexOf(anchor) : parent.children.length;";
 const replacement="    // Solid keyed swaps rely on DOM insertBefore(node, node) being a no-op.\n    if (node === anchor) return;\n"+anchor;
 const keyed=source.includes("if (node === anchor) return;")?source:source.replace(anchor,replacement);
  const focus=keyed.includes('name === "data-preserve-keyboard-focus"')?keyed:keyed.replace("UNIVERSAL_PROPS.has(name)", '(name === "data-preserve-keyboard-focus" || UNIVERSAL_PROPS.has(name))');
  const device=focus.includes('case "device_viewport":')?focus:focus.replace('        case "canvas":','        case "canvas":\n        case "device_viewport":');
  // Animation frames often recreate equal style objects. Do not send these
  // through JSON/NAPI and invalidate the native tree for unchanged geometry.
  const patched=device.replace('unregisterEventHandlers, }','unregisterEventHandlers, sameMutationValue, }').replace('        node.style = isStyle(value) ? value : {};','        const next = isStyle(value) ? value : {};\n        if (sameMutationValue(node.style, next)) return;\n        node.style = next;');
 if(hash(patched)!==pin.patchedSha256)throw new Error("The GPUIX Solid host patch did not match its reviewed checksum");
 writeFileSync(path,patched);return true;
}
module.exports=patchSolidHost;
if(require.main===module)patchSolidHost();
