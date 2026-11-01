import type {AppConfig} from "../config";
import type {HostElement, KeyEvent} from "@gpuix/solid";

export interface DevicePanelProps {
  config: AppConfig;
  visible: boolean;
  workspaceId?: string;
  width: number;
  active?: boolean;
  focusToken?: number;
  onHost?: (host: HostElement | undefined) => void;
  onFocus?: () => void;
  onClose: () => void;
  onCustomKey?: (event: KeyEvent) => boolean;
}