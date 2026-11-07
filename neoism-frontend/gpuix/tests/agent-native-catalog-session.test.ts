import { expect, test } from "bun:test";
import { catalogThread } from "../src/agent/catalog-session";

test("native root branding requires provider-matched persisted ownership", () => {
  for (const [provider, runtime] of Object.entries({ codex: "codex-app-server", claude: "claude-agent-sdk", opencode: "opencode-server" })) {
    const session = { id: "root", title: "Native", externalAgent: { provider, runtime } };
    expect(catalogThread(session as never).external_provider).toBe(provider);
    expect(catalogThread({ ...session, parentId: "parent" } as never).external_provider).toBeNull();
    expect(catalogThread({ ...session, externalAgent: { provider, runtime: "acp" } } as never).external_provider).toBe(provider);
    for (const invalid of [{ runtime: "unknown" }, { runtime: "native" }, { nested: true }, { nativeProjection: true }]) {
      expect(catalogThread({ ...session, externalAgent: { ...session.externalAgent, ...invalid } } as never).external_provider).toBeNull();
    }
  }
  expect(catalogThread({ id: "root", externalAgent: { provider: "codex", runtime: "claude-agent-sdk" } } as never).external_provider).toBeNull();
});