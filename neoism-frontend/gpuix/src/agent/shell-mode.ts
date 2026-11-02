/** Only column-zero ! is shell mode. Merely editing this draft never executes. */
export function shellCommand(text: string): string | undefined {
  return text.startsWith("!") ? text.slice(1).trimStart() : undefined;
}
export function exitShellMode(text: string, key: string): string | undefined {
  const command = shellCommand(text);
  if (command === undefined) return undefined;
  return key === "escape" || key === "composer-escape" || key === "backspace" && !command ? "" : undefined;
}