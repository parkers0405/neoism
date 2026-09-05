import { copyFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const canonical = fileURLToPath(
  new URL("../../desktop/assets/icons/neoism.png", import.meta.url),
);
const webIcon = fileURLToPath(new URL("../public/icon-512.png", import.meta.url));

copyFileSync(canonical, webIcon);