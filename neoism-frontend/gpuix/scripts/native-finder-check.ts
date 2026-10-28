import { launchNative } from "../tests/support/native-launch";
const app = await launchNative({ command: process.execPath, args: ["tests/support/finder-window.tsx"], cwd: process.cwd() });
let stage="initial";
try {
  const input = app.getByTestId("command-palette");
  await input.waitFor({ timeoutMs: 15000 });
  await app.getByText("file-0.rs").waitFor();
  const width = (await app.getByTestId("finder").bounds()).width;
  if (width > 500 || width < 440) throw new Error(`Compact Finder geometry drifted: ${width}`);
  stage="filter"; await input.fill("main"); await app.getByText("main-0.rs").waitFor();
  stage="navigation"; await input.press("down"); await input.press("down");
  stage="cycle"; await app.getByTestId("finder-mode").click();
  await app.getByText("[exact]").waitFor();
  stage="accept"; await app.getByTestId("finder-result:1").waitFor(); await app.getByTestId("finder-result:1").click();
  await app.getByText("src/main-1.rs").waitFor();
  if (await app.getByTestId("finder-selection").textContent() !== "src/main-1.rs") throw new Error("Finder accepted the wrong file");
  console.log("Native Finder geometry, filtering, keyboard navigation, mode cycling, and mouse acceptance passed.");
} catch(error) { console.error("Finder stage", stage); console.error("selected",await app.getByTestId("finder-selection").textContent()); throw error; } finally { await app.close(); }
