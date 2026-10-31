import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { launchNative } from "../tests/support/native-launch";
import { createAgentDaemon } from "../tests/support/agent-daemon";

const directory = mkdtempSync(join(tmpdir(), "neoism-menu-check-"));
const config = join(directory, "config.json");
writeFileSync(config, JSON.stringify({ editor: { "vim-mode": true }, terminal: { cursor: { blinking: false } } }));
const daemon = createAgentDaemon();
daemon.files.set("README.md", "zzzzspelinggg\n\n# Visible document\n\nThe document must stay visible behind the spelling menu.\n\n" + "More visible document text.\n\n".repeat(15));
const json = async (args: string[]) => JSON.parse(await new Response(Bun.spawn(args, { stdout: "pipe" }).stdout).text());
const before = new Set((await json(["hyprctl", "clients", "-j"])).map((c: any) => c.address));
const app = await launchNative({ command: process.execPath, args: ["src/main.tsx", "--daemon", daemon.endpoint, "--workspace", "a", "--config", config], cwd: process.cwd(), env: { ...process.env, NEOISM_GPUIX_STATE_PATH: join(directory, "session.json") } });
try {
  await app.getByTestId("file:README.md").waitFor();
  await app.getByTestId("file:README.md").click();
  const editor = app.getByTestId("rich-markdown-editor");
  await editor.waitFor(); await app.getByTestId("rich-markdown-caret").waitFor(); await Bun.sleep(250);
  const client = (await json(["hyprctl", "clients", "-j"])).find((c: any) => !before.has(c.address) && c.class === "ai.neoism.gpuix");
  if (!client) throw new Error("Controlled Markdown window not found");
  const monitor = (await json(["hyprctl", "monitors", "-j"])).find((m: any) => m.id === client.monitor);
  const capture = async (path: string) => { if (await Bun.spawn(["grim", "-o", monitor.name, path]).exited) throw new Error("Capture failed"); };
  const initial = join(directory, "before.png"), opened = join(directory, "menu.png");
  await capture(initial);
  const caret = await app.getByTestId("rich-markdown-caret").bounds();
  await app.mouse.click({ x: caret.x + 3, y: caret.y + caret.height / 2 }, { button: 2 });
  const menu = app.getByTestId("markdown-context-menu"); await menu.waitFor(); await Bun.sleep(200);
  const popup = await menu.bounds(); await capture(opened);
  const check = Bun.spawn(["/usr/bin/python3", "-c", `
from PIL import Image
import sys,json
c,m,r=map(json.loads,sys.argv[3:]);scale=m['scale']
x=(c['at'][0]-m['x'])*scale;y=(c['at'][1]-m['y'])*scale
box=(round(x),round(y),round(x+c['size'][0]*scale),round(y+c['size'][1]*scale))
a,b=[Image.open(p).crop(box).convert('RGB') for p in sys.argv[1:3]]
def outside(px,py):
 return not ((r['x']-6)*scale<=px<=(r['x']+r['width']+6)*scale and (r['y']-6)*scale<=py<=(r['y']+r['height']+6)*scale)
ink=kept=0
for py in range(a.height):
 for px in range(a.width):
  if outside(px,py) and min(a.getpixel((px,py)))>180:
   ink+=1;kept+=min(b.getpixel((px,py)))>160
assert ink>500,('No document/chrome ink to compare',ink)
assert kept>ink*.85,('Menu obscured the document/chrome',ink,kept)
print('Spelling menu preserves document and chrome:',kept,'of',ink,'bright pixels outside menu')
`, initial, opened, JSON.stringify(client), JSON.stringify(monitor), JSON.stringify(popup)], { stdout: "inherit", stderr: "inherit" });
  if (await check.exited) throw new Error("Spelling menu covered the window");
  await editor.press("escape");
  await app.getByTestId("topbar-menu").click(); await app.getByTestId("original-hamburger-menu").waitFor();
} finally { await app.close(); daemon.stop(); rmSync(directory, { recursive: true, force: true }); }
