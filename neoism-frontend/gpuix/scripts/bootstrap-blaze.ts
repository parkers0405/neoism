// Shared checksum-pinned release installer; no compiler bootstrap on this host.
import {fileURLToPath} from "node:url";
const child=Bun.spawn(["node",fileURLToPath(new URL("../../../scripts/setup-blaze.mjs",import.meta.url))],{stdout:"inherit",stderr:"inherit"});
process.exit(await child.exited);
export {};
