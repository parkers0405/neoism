import { expect, test } from "bun:test";
import { providerLogoIndex, providerLogoColor, providerLogoSvg } from "../src/ui/ProviderLogo";
import { palettes, providerLogoAtlasBase64 } from "../src/design/generated";
test("model logos preserve original provider identity and omit unknown models",()=>{
  expect(providerLogoIndex(" openrouter/openai/gpt-5 ")).toBe(0);
  expect(providerLogoIndex("ANTHROPIC/claude")).toBe(1);
  expect(providerLogoIndex("codex/gpt-5")).toBe(2);
  expect(providerLogoIndex("vertex-ai/gemini")).toBe(3);
  expect(providerLogoIndex("copilot/model")).toBe(4);
  expect(providerLogoIndex("vercel-ai-gateway/model")).toBe(5);
  expect(providerLogoIndex("open-code/model")).toBe(6);
  expect(providerLogoIndex("server default")).toBeUndefined();
  expect(providerLogoIndex("unknown/model")).toBeUndefined();
});
test("original atlas embeds packaged pixels and preserves composer brand treatment",()=>{
  const bytes=Buffer.from(providerLogoAtlasBase64,"base64");
  expect(bytes.readUInt32BE(16)).toBe(448);expect(bytes.readUInt32BE(20)).toBe(64);
  expect(providerLogoSvg(1)).toContain('viewBox="64 0 64 64"');
  expect(providerLogoSvg(1)).toContain(`data:image/png;base64,${providerLogoAtlasBase64}`);
  expect(providerLogoColor(1,palettes.pastel_dark)).toBe("#d97757");
  expect(providerLogoColor(2,palettes.pastel_dark)).toBe("#ffffff");
  expect(providerLogoColor(0,{...palettes.pastel_dark,bg:"#ffffff"})).toBe("#000000");
  expect(providerLogoColor(3,palettes.pastel_dark)).toBe(palettes.pastel_dark.blue);
});
