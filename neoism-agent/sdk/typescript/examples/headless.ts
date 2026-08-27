// Headless Neoism Agent driver — the complete embedding loop a product
// backend (SaaS assistant, bot, pipeline) runs per tenant conversation:
//
//   1. Connect with a bearer token (see "Serving beyond loopback" in the
//      Server and API handbook page).
//   2. Create (or reuse) a session rooted in the tenant's directory.
//   3. Subscribe to the typed event stream BEFORE prompting, with
//      `tail: true` so history is not replayed. The subscription
//      auto-reconnects with a sequence cursor and deduplicates replays.
//   4. Send the prompt with a caller-generated messageId — retrying the
//      same prompt after a network error is idempotent.
//   5. Stream tokens from `message.part.delta`, watch tool calls and
//      usage through typed parts, and finish when `session.status`
//      reports idle for this session.
//
// Run inside this workspace:   npx tsx examples/headless.ts
// Outside, once published:     npm i @neoism/sdk-http && adjust imports
//
// Environment:
//   NEOISM_AGENT_URL    e.g. http://127.0.0.1:4096  (default)
//   NEOISM_AGENT_TOKEN  bearer token when the server requires one
//   NEOISM_DIRECTORY    session/workspace directory (default: cwd)
//   PROMPT              the prompt text (default: a smoke-test line)

import { createHttpClient } from "@neoism/sdk-http";
import type { Event, Part, StepFinishPart } from "@neoism/sdk-core";
import { randomUUID } from "node:crypto";

const client = createHttpClient({
  baseUrl: process.env.NEOISM_AGENT_URL ?? "http://127.0.0.1:4096",
  ...(process.env.NEOISM_AGENT_TOKEN
    ? { token: process.env.NEOISM_AGENT_TOKEN }
    : {}),
});

async function main(): Promise<void> {
  const session = await client.sessions.create({
    directory: process.env.NEOISM_DIRECTORY ?? process.cwd(),
    title: "Headless example",
  });
  console.log(`session ${session.id}`);

  // Subscribe first so no event can slip between prompt and stream.
  const abort = new AbortController();
  const events = client.events.subscribe({
    sessionId: session.id,
    tail: true,
    signal: abort.signal,
  });

  await client.sessions.prompt(session.id, {
    messageId: `msg_${randomUUID().replaceAll("-", "")}`,
    parts: [
      {
        type: "text",
        text: process.env.PROMPT ?? "Reply with the single word: ready",
      },
    ],
  });

  let sawBusy = false;
  const usage: StepFinishPart[] = [];
  for await (const event of events) {
    switch (event.type) {
      case "message.part.delta": {
        // Token stream. `partType` distinguishes answer text from
        // reasoning; both arrive through the same channel.
        if (event.data.field === "text" && event.data.partType === "text") {
          process.stdout.write(event.data.delta);
        }
        break;
      }
      case "message.part.updated": {
        const part: Part = event.data.part;
        if (part.type === "tool") {
          // Discriminated tool state: pending → running → completed/error.
          const state = part.state;
          if (state.status === "completed") {
            console.log(`\n[tool ${part.tool}] ${state.title}`);
          } else if (state.status === "error") {
            console.log(`\n[tool ${part.tool}] failed: ${state.error}`);
          }
        } else if (part.type === "step-finish") {
          // Billing's raw material: typed token counts + server cost.
          usage.push(part);
        }
        break;
      }
      case "permission.asked": {
        // Headless runs should preconfigure permission rules (see the
        // Permissions handbook page) so this never fires; replying here
        // is the interactive fallback.
        console.log(`\n[permission] rejecting: ${event.data.permission}`);
        await client.interactions.permissions.reply(event.data.id, "reject");
        break;
      }
      case "session.error": {
        console.error(`\n[error] ${JSON.stringify(event.data.error)}`);
        break;
      }
      case "session.status": {
        if (event.data.sessionID !== session.id) break;
        const status = event.data.status.type;
        if (status === "busy" || status === "retry") sawBusy = true;
        if (status === "idle" && sawBusy) {
          abort.abort();
        }
        break;
      }
      default: {
        // The union is exhaustive — narrow `event` further as needed.
        const _unhandled: Event = event;
        void _unhandled;
      }
    }
    if (abort.signal.aborted) break;
  }

  const tokens = usage.reduce(
    (sum, step) => sum + step.tokens.input + step.tokens.output + step.tokens.reasoning,
    0,
  );
  const cost = usage.reduce((sum, step) => sum + step.cost, 0);
  console.log(`\nturn complete · ${usage.length} step(s) · ${tokens} tokens · $${cost.toFixed(4)}`);

  const transcript = await client.sessions.messages(session.id, { order: "asc" });
  console.log(`transcript holds ${transcript.items.length} message(s)`);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
