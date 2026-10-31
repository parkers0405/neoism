/** AppKit traffic lights occupy logical pixels, independent of Neoism's UI scale. */
export function titlebarLeftInset(platform: string, fullscreen: boolean) {
  return platform === "darwin" && !fullscreen ? 80 : 0;
}

export function observeTitlebarInset(renderer: {isFullscreen: () => boolean}, platform: string, changed: (inset: number) => void) {
  let previous = -1;
  const sample = () => {
    const inset = titlebarLeftInset(platform, platform === "darwin" && renderer.isFullscreen());
    if (inset !== previous) { previous = inset; changed(inset); }
  };
  sample();
  // Includes the green button and macOS's own fullscreen shortcut, not just app actions.
  const timer = platform === "darwin" ? setInterval(sample, 100) : undefined;
  return () => { if (timer !== undefined) clearInterval(timer); };
}
