import { expect, test } from "bun:test";
import { providerLogoIndex, providerLogoColor, providerLogoSvg } from "../src/ui/ProviderLogo";
import { palettes, providerLogoAtlasBase64, providerLogoAtlasWidth, providerLogos } from "../src/design/generated";
test("model logos preserve original provider identity and omit unknown models",()=>{
  expect(providerLogoIndex(" openrouter/openai/gpt-5 ")).toBe(0);
  expect(providerLogoIndex("ANTHROPIC/claude")).toBe(1);
  expect(providerLogoIndex("codex/gpt-5")).toBe(2);
  expect(providerLogoIndex("vertex-ai/gemini")).toBe(3);
  expect(providerLogoIndex("copilot/model")).toBe(4);
  expect(providerLogoIndex("vercel-ai-gateway/model")).toBe(5);
  expect(providerLogoIndex("open-code/model")).toBe(6);
  for (const alias of ["xai", "x-ai", "grok"]) {
    expect(providerLogos[alias]).toBe(7);
    expect(providerLogoIndex(` ${alias.toUpperCase()}/grok-4 `)).toBe(7);
  }
  expect(providerLogoIndex("openrouter/xai/grok-4")).toBe(0);
  expect(providerLogoIndex("unknown/grok-4")).toBeUndefined();
  expect(providerLogoIndex("server default")).toBeUndefined();
  expect(providerLogoIndex("unknown/model")).toBeUndefined();
});
test("original atlas embeds packaged pixels and preserves composer brand treatment",()=>{
  const bytes=Buffer.from(providerLogoAtlasBase64,"base64");
  expect(bytes.readUInt32BE(16)).toBe(512);expect(bytes.readUInt32BE(20)).toBe(64);
  expect(providerLogoAtlasWidth).toBe(512);
  expect(providerLogoSvg(7)).toContain('viewBox="448 0 64 64"');
  expect(providerLogoSvg(7)).toContain('width="512" height="64"');
  expect(providerLogoSvg(1)).toContain('viewBox="64 0 64 64"');
  expect(providerLogoSvg(1)).toContain(`data:image/png;base64,${providerLogoAtlasBase64}`);
  expect(providerLogoColor(1,palettes.pastel_dark)).toBe("#d97757");
  expect(providerLogoColor(2,palettes.pastel_dark)).toBe("#ffffff");
  expect(providerLogoColor(7,palettes.pastel_dark)).toBe("#ffffff");
  expect(providerLogoColor(7,{...palettes.pastel_dark,bg:"#ffffff"})).toBe("#000000");
  expect(providerLogoColor(0,{...palettes.pastel_dark,bg:"#ffffff"})).toBe("#000000");
  expect(providerLogoColor(3,palettes.pastel_dark)).toBe(palettes.pastel_dark.blue);
});
