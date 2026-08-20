export const DEFAULT_DAEMON_URL = "ws://127.0.0.1:7878/session";

export interface DaemonLocation {
  protocol: string;
  host: string;
  port: string;
  search: string;
}

export interface DaemonTarget {
  url: string;
  token?: string;
  /** True when the page was given an explicit daemon (`?daemon=` / `?url=`)
   *  or is being served from the daemon itself (same-origin `/session`). */
  autoConnect: boolean;
  fromQuery: boolean;
}

/**
 * Pick the daemon WebSocket the page should talk to.
 *
 * Debug desktops bind an ephemeral TCP port (`NEOISM_DAEMON_TCP_PORT`),
 * not the head daemon on :7878. The browser cannot read process env, so
 * the launcher passes `?daemon=ws://127.0.0.1:<port>/session` the same
 * way the agent pane inherits `NEOISM_SERVER`. A page served from the
 * daemon uses same-origin `/session`.
 */
export function resolveDaemonTarget(
  loc: DaemonLocation,
  injectedUrl?: string,
): DaemonTarget {
  const params = new URLSearchParams(loc.search);
  const queryUrl = firstNonEmpty(params.get("daemon"), params.get("url"));
  const token = firstNonEmpty(params.get("token"));
  if (queryUrl) {
    return {
      url: queryUrl,
      token,
      autoConnect: true,
      fromQuery: true,
    };
  }

  const viteDev = loc.port === "5173";
  if (!viteDev && (loc.protocol === "http:" || loc.protocol === "https:")) {
    const ws = loc.protocol === "https:" ? "wss:" : "ws:";
    return {
      url: `${ws}//${loc.host}/session`,
      token,
      autoConnect: true,
      fromQuery: false,
    };
  }

  const injected = firstNonEmpty(injectedUrl);
  if (injected) {
    return {
      url: injected,
      token,
      autoConnect: false,
      fromQuery: false,
    };
  }

  return {
    url: DEFAULT_DAEMON_URL,
    token,
    autoConnect: false,
    fromQuery: false,
  };
}

export function viteInjectedDaemonUrl(): string | undefined {
  try {
    const env = (import.meta as { env?: Record<string, string | undefined> })
      .env;
    return firstNonEmpty(env?.VITE_NEOISM_DAEMON_URL);
  } catch {
    return undefined;
  }
}

function firstNonEmpty(
  ...values: Array<string | null | undefined>
): string | undefined {
  for (const value of values) {
    const trimmed = value?.trim();
    if (trimmed) return trimmed;
  }
  return undefined;
}
