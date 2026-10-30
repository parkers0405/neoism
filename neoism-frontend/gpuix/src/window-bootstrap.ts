import type { GpuixRenderer } from "@gpuix/native";
import type { AppConfig } from "./config";

export interface WindowBootstrap { config: AppConfig; renderer: GpuixRenderer; ready: Promise<void> }
