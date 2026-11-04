import {
  DaemonConnection as SharedDaemonConnection,
  type DaemonConnectionConfig,
  type DaemonConnectionHooks,
  type SocketFactory,
  type ConnectionFailure,
} from "../../../service-client/src/client";
import { profileStart, profileEnd } from "../profiling";
import { batch } from "solid-js";

export * from "../../../service-client/src/client";

/** Keep GUI instrumentation at the host boundary, not in the shared transport. */
export class DaemonConnection extends SharedDaemonConnection {
  constructor(
    config: DaemonConnectionConfig,
    createSocket?: SocketFactory,
    onFailure?: (failure: ConnectionFailure) => void,
    hooks: DaemonConnectionHooks = { profileStart, profileEnd, batch },
  ) {
    super(config, createSocket, onFailure, hooks);
  }
}