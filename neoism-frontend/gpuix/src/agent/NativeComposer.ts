import type { AppConfig } from "../config";
import { coreCall } from "../native-core";
import type { PaintCommand } from "../ui/PaintLayer";
import { sharePaintCommands } from "../editor/paint-command";
export type ComposerRect = [number, number, number, number];
export interface ComposerChip {rect:ComposerRect;label_rect:ComposerRect;caret_rect:ComposerRect;logo_rect:ComposerRect|null;label:string}
export interface ComposerChips {chips:[ComposerChip,ComposerChip,ComposerChip];attach_rect:ComposerRect;font_size:number;narrow:boolean}
export interface ComposerFrame {
  cardColor: number[]; trayColor: number[]; inputRect: ComposerRect; logoRect: ComposerRect; cardRect: ComposerRect; trayRect: ComposerRect | null;
  joinRect:ComposerRect|null; cardRadius: number; trayRadius: number; footerVisible: boolean; visualRows: number; animating: boolean;
  controls: { visual_rect: ComposerRect; hit_rect: ComposerRect; chips_rect: ComposerRect; text_rect: ComposerRect; narrow: boolean; action: "Disabled" | "Send" | "Interrupt" };
  commands: PaintCommand[]; cwdRects: ComposerRect[]; systemRects: ComposerRect[];
}
export interface ComposerInput {
  width: number; height: number; home: boolean; text: string; help: boolean; images: number;
  cwd?: string; branch?: string; system?: string; noProviders: boolean; busy: boolean; state: string;
}
export class NativeComposer {
  readonly id: number;
  private before: PaintCommand[] = [];
  constructor(readonly config: AppConfig) { this.id = coreCall<{ id: number }>("agentcomposer.new", { fontFamily: config.fontFamily, fontWeight: config.fontWeight }).id; }
  frame(input: ComposerInput) {
    const next = coreCall<ComposerFrame>("agentcomposer.frame", { id: this.id, ...input, scale: this.config.scale, theme: this.config.themeName, materialOpacity: this.config.composerOpacity, seconds: performance.now() / 1000 });
    next.commands = sharePaintCommands(this.before, next.commands); this.before = next.commands; return next;
  }
  chips(input:{width:number;height:number;agent:string;model:string;thinking:string}){return coreCall<ComposerChips>("agentcomposer.chips-layout",{id:this.id,...input,scale:this.config.scale});}
  startEffect(kind: string) { coreCall("agentcomposer.fx.start", { id: this.id, kind }); }
  effect(input: { width: number; height: number; inputY: number }) {
    return coreCall<{ commands: PaintCommand[]; prompt?: string | null; animating: boolean }>("agentcomposer.fx.frame", { id: this.id, ...input, scale: this.config.scale, theme: this.config.themeName });
  }
  dispose() { coreCall("agentcomposer.dispose", { id: this.id }); }
}
