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
 if(digest!==pin.originalSha256)throw new Error("The GPUIX Solid host has unrecognized edits; refusing to overwrite it");
 const anchor="    removeFromParent(node);\n    const index = anchor ? parent.children.indexOf(anchor) : parent.children.length;";
 const replacement="    // Solid keyed swaps rely on DOM insertBefore(node, node) being a no-op.\n    if (node === anchor) return;\n"+anchor;
 const patched=source.replace(anchor,replacement);
 if(hash(patched)!==pin.patchedSha256)throw new Error("The GPUIX Solid host patch did not match its reviewed checksum");
 writeFileSync(path,patched);return true;
}
module.exports=patchSolidHost;
if(require.main===module)patchSolidHost();
