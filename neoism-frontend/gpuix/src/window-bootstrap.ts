import type { GpuixRenderer } from "@gpuix/native";
import type { AppConfig } from "./config";

export interface WindowBootstrap { config: Promise<AppConfig>; renderer: GpuixRenderer; ready: Promise<void>; onInput?:(event:import("@gpuix/native").EventPayload)=>void }
