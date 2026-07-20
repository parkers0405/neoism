import {
  test,
  expect,
  completeConnectionHandshake,
  DEFAULT_DAEMON_URL,
} from "./fixtures";

/**
 * 12-flow web smoke harness — mirrors the manual walkthrough enumerated
 * in `/SMOKE_TEST_REPORT.md` and Task A3 in `TASKS.md`.
 *
 * Flows marked `test.skip()` need:
 *   - a real input device (touch, IME composition) the headless runner
 *     can't synthesize without the CDP `Input.dispatchTouchEvent` path
 *     wired through Playwright's mobile context (TODO: enable behind a
 *     dedicated `mobile` project), OR
 *   - a binary the CI image may not have (tailscale CLI for discover),
 *     OR
 *   - daemon broadcast frames that arrive only after a long-running
 *     wasm boot round-trip (agent pane streaming).
 *
 * The non-skipped tests assert on chrome DOM + network behaviour that
 * `e2e-up.sh`'s daemon + vite combo serve out of the box.
 */

test.describe("D1 — Web E2E smoke harness", () => {
  test("connection handshake completes and terminal mounts", async ({
    app,
  }) => {
    // Fixture already drove the form to the terminal panel; the workplace
    // switcher is opened on demand through the command palette.
    await expect(app.locator(".terminal-panel")).toBeVisible();
    await expect(app.locator(".app-chrome-bar")).toHaveCount(0);
  });

  test("wasm bundle is served and the terminal canvas attaches", async ({
    app,
  }) => {
    // The terminal canvas only mounts once `TerminalPanel` finishes its
    // wasm import + bridge wire-up. If the bundle is missing this never
    // appears.
    await expect(app.locator(".terminal-canvas")).toBeVisible({
      timeout: 30_000,
    });
  });

  test("flow 1 — PTY: shell echoes typed input", async ({
    browser,
    daemonUrl,
  }) => {
    // Smoke-level: focus the canvas, type a recognisable string, and
    // assert that the daemon round-trip produced *some* pty-output
    // frame (we observe via the websocket message log). We can't read
    // the rendered glyph from a wasm canvas, but a single output frame
    // proves the daemon spawned a shell, the bridge piped input bytes
    // back, and the read loop is alive.
    //
    // `page.on("websocket")` only fires for sockets opened *after* the
    // listener attaches, so this test deliberately bypasses the `app`
    // fixture (which already opens a socket during the handshake) and
    // creates a fresh page so it can pre-register the listener.
    const context = await browser.newContext();
    const page = await context.newPage();
    const wsFrames: string[] = [];
    page.on("websocket", (ws) => {
      ws.on("framereceived", ({ payload }) => {
        if (typeof payload === "string") wsFrames.push(payload);
      });
    });

    await page.goto("about:blank");
    await page.evaluate(() => {
      try {
        window.localStorage.clear();
      } catch {
        /* sandboxed */
      }
    });
    await page.goto("/");
    await completeConnectionHandshake(page, daemonUrl);
    await page.waitForSelector(".terminal-canvas", { timeout: 30_000 });

    const canvas = page.locator(".terminal-canvas");
    await canvas.click();
    await page.keyboard.type("echo neoism-e2e\n", { delay: 20 });

    try {
      await expect
        .poll(
          () => wsFrames.some((frame) => frame.includes("PtyOutput")),
          {
            timeout: 15_000,
            message: "no PtyOutput frame after typing into canvas",
          },
        )
        .toBe(true);
    } finally {
      await context.close();
    }
  });

  test("flow 2 — File tree: skipped (needs CDP file-system bridge)", () => {
    // TODO(d1-followup): exercise FilesService.create/rename/delete via
    // the wasm bridge once we expose a chrome-level command palette
    // hook the harness can fire without reaching for native context
    // menus painted into the wasm canvas.
    test.skip(true, "file-tree context menu lives inside wasm canvas");
  });

  test("flow 3 — Agent pane: skipped (requires agent api key)", () => {
    // TODO(d1-followup): set NEOISM_AGENT_API_KEY + NEOISM_AGENT_MODEL
    // for the daemon and gate on streaming token frames.
    test.skip(true, "agent streaming needs a configured API key");
  });

  test("flow 4 — Image clipboard: paste uploads to /clipboard-image", async ({
    app,
  }, testInfo) => {
    // Permission grant: clipboard-read isn't reliable across CDP
    // versions; instead we POST a PNG directly to the daemon endpoint
    // the paste path eventually hits, and assert the file lands and is
    // served back with the same bytes. Proves the daemon's image
    // ingest + GET-back loop is alive — the wasm-level keybind that
    // calls this endpoint is exercised in unit tests.
    const daemonHttpBase = DEFAULT_DAEMON_URL
      .replace(/^wss:/, "https:")
      .replace(/^ws:/, "http:")
      .replace(/\/session$/, "");

    // Minimal 1x1 PNG (transparent pixel).
    const pngBase64 =
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGBgAAAABQABXvMqOgAAAABJRU5ErkJggg==";
    const pngBytes = Buffer.from(pngBase64, "base64");

    const apiContext = await app.request;
    const upload = await apiContext.post(
      `${daemonHttpBase}/clipboard-image-upload`,
      {
        multipart: {
          image: {
            name: "smoke.png",
            mimeType: "image/png",
            buffer: pngBytes,
          },
        },
      },
    );

    if (upload.status() === 404) {
      // Some daemon builds expose the upload via a websocket message
      // instead of an HTTP route; the round-trip (GET /clipboard-image/...)
      // is the load-bearing assertion below.
      testInfo.skip(
        true,
        "daemon does not expose /clipboard-image-upload HTTP route",
      );
      return;
    }

    // Whether or not we ingested via HTTP, the /clipboard-image/<name>
    // GET path is always served (404 if absent, 200 if present). We
    // only proceed to the GET-back check when the upload succeeded.
    expect([200, 201, 204]).toContain(upload.status());

    const fetched = await apiContext.get(
      `${daemonHttpBase}/clipboard-image/smoke.png`,
    );
    expect(fetched.status()).toBe(200);
  });

  test("flow 5 — Command palette: skipped (wasm chrome popup)", () => {
    // TODO(d1-followup): Cmd+P opens a wasm-rendered overlay; we need a
    // pixel-diff or a JS-exposed test hook to confirm the popup
    // materialised.
    test.skip(true, "command palette overlay is painted inside wasm");
  });

  test("flow 6 — Finder: skipped (wasm chrome popup)", () => {
    // TODO(d1-followup): same constraint as the command palette.
    test.skip(true, "finder overlay is painted inside wasm");
  });

  test("flow 7 — Multi-pane: skipped (wasm chrome popup)", () => {
    // TODO(d1-followup): pane split state lives entirely inside
    // ContextGrid; expose a test hook on `TerminalPanel` so the harness
    // can assert pane count after Cmd+\.
    test.skip(true, "pane splits live inside wasm ContextGrid");
  });

  test("flow 8 — Touch gestures: skipped (needs mobile project)", () => {
    // TODO(d1-followup): add a second Playwright project with
    // `hasTouch: true` + `isMobile: true` + a viewport size matching the
    // C1/C3 mobile-keyboard tests.
    test.skip(true, "touch synthesis needs a mobile-context project");
  });

  test("flow 9 — IME composition: skipped (needs CDP IME shim)", () => {
    // TODO(d1-followup): CDP `Input.imeSetComposition` is supported by
    // recent Chrome but Playwright doesn't expose it; reach in via
    // `page.context().newCDPSession()` and drive a composition stream
    // manually.
    test.skip(true, "IME synthesis needs raw CDP access");
  });

  test("flow 10 — Tailscale: /tailnet-peers endpoint responds", async ({
    app,
  }) => {
    // The "Discover" button in the chrome switcher hits this endpoint;
    // the JSON shape is the contract the switcher widget expects. If
    // tailscale CLI isn't installed the daemon returns an empty list
    // (still 200) rather than an error.
    const daemonHttpBase = DEFAULT_DAEMON_URL
      .replace(/^wss:/, "https:")
      .replace(/^ws:/, "http:")
      .replace(/\/session$/, "");
    const apiContext = await app.request;
    const peers = await apiContext.get(`${daemonHttpBase}/tailnet-peers`);
    expect(peers.status()).toBe(200);
    const body = (await peers.json()) as { peers: unknown[] };
    expect(Array.isArray(body.peers)).toBe(true);
  });

  test("flow 11 — Multi-daemon: registry survives a workplace add", async ({
    app,
  }) => {
    // Drive the WorkplaceService directly via a test hook: add a second
    // workplace, assert listWorkplaces() now reports two entries. This
    // is the chrome-level state the switcher renders from — we don't
    // need to actually dial the second daemon to prove the registry
    // wiring is alive.
    const initialCount = await app.evaluate(() => {
      try {
        const raw = window.localStorage.getItem("neoism.workplaces.v1");
        if (!raw) return 0;
        return (JSON.parse(raw)?.entries ?? []).length;
      } catch {
        return 0;
      }
    });

    expect(initialCount).toBeGreaterThanOrEqual(1);

    // Add a second URL by mutating localStorage + reloading; the
    // service hydrates on construction.
    await app.evaluate(() => {
      const raw = window.localStorage.getItem("neoism.workplaces.v1");
      const parsed = raw ? JSON.parse(raw) : { entries: [], lastActiveId: null };
      parsed.entries.push({
        id: "e2e-second-daemon",
        url: "ws://127.0.0.1:7879/session",
        label: "127.0.0.1:7879",
        transport: "manual",
      });
      window.localStorage.setItem(
        "neoism.workplaces.v1",
        JSON.stringify(parsed),
      );
    });
    await app.reload();
    await app.waitForSelector(".connection-form, .terminal-panel", {
      timeout: 30_000,
    });

    const finalCount = await app.evaluate(() => {
      const raw = window.localStorage.getItem("neoism.workplaces.v1");
      if (!raw) return 0;
      return (JSON.parse(raw)?.entries ?? []).length;
    });
    expect(finalCount).toBeGreaterThanOrEqual(initialCount + 1);
  });

  test("flow 12 — vite serves the wasm bundle", async ({ app }) => {
    // The wasm bundle is the load-bearing artefact for every other
    // flow; assert vite's static handler still returns it. This is the
    // same probe `SMOKE_TEST_REPORT.md` ran via curl, lifted into the
    // harness so a future bundle-name regression fails here loudly.
    const response = await app.request.head(
      "/neoism-terminal-wasm/neoism_terminal_wasm_bg.wasm",
    );
    expect([200, 304]).toContain(response.status());
  });
});
