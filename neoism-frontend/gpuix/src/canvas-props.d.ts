import "@gpuix/native/host";
declare module "@gpuix/native/host" {
  /** Native `<canvas>` display list (JSON CanvasOp[]); see src/ui/canvas.ts. */
  interface HostProps { commands?: string }
}
