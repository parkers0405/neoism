import type { KeyEvent } from "@gpuix/solid";

type WindowKeyHandler = (event: KeyEvent) => void;

/** The persistent GPUIX Root owns window events; its current App generation
 * owns shortcut policy. Keep the normalized event (and its shared stop flags)
 * intact when forwarding between them. */
export function createWindowKeyRouter() {
  let current: { handler: WindowKeyHandler } | undefined;
  return {
    register(handler: WindowKeyHandler) {
      const registration = { handler }; current = registration;
      return () => { if (current === registration) current = undefined; };
    },
    dispatch(event: KeyEvent) {
      if (!event.defaultPrevented) current?.handler(event);
    },
  };
}
