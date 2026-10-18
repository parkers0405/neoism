/** Originating browser OS, never the WASM target or daemon host OS. */
export interface BrowserPlatform {
  userAgentData?: { platform?: string };
  platform?: string;
  userAgent?: string;
}

export function detectClientOs(
  browser: BrowserPlatform | undefined = typeof navigator === "undefined" ? undefined : navigator,
): string | null {
  // Prefer client hints; fall through unrecognized/empty hints for older browsers.
  for (const value of [browser?.userAgentData?.platform, browser?.platform, browser?.userAgent]) {
    if (!value) continue;
    if (/windows|win32|win64|wince/i.test(value)) return "windows";
    if (/android/i.test(value)) return "android";
    if (/iphone|ipad|ipod|\bios\b/i.test(value)) return "ios";
    if (/mac/i.test(value)) return "macos";
    if (/cros/i.test(value)) return "chromeos";
    if (/linux/i.test(value)) return "linux";
  }
  return null;
}
