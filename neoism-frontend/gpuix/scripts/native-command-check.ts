import { launchNative } from "../tests/support/native-launch";
const app = await launchNative({ command: process.execPath, args: ["tests/support/command-window.tsx"], cwd: process.cwd() });
try {
  const input = app.getByTestId("command-palette"); await input.waitFor({ timeoutMs: 15000 });
  await input.fill("Write File"); await app.getByTestId("command:SaveDocument").waitFor(); await input.press("enter");
  await app.getByText("SaveDocument").waitFor();
  const target = app.getByTestId("key-target"); await target.press("ctrl-shift-p");
  await app.getByTestId("command-dialog").waitFor(); await input.fill("Split Right");
  await app.getByTestId("command:SplitRight").waitFor(); await app.getByTestId("command:SplitRight").click();
  await app.getByText("SplitRight").waitFor();
  console.log("Native command filtering, Enter dispatch, custom modifier binding, and mouse dispatch passed.");
} finally { await app.close(); }
