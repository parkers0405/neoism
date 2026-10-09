/** Mirrors service-api validate_worker_vm_path / worker_vm_path_contains.
 * Pure VM namespace validation: never consult the SDK host's filesystem.
 * Canonical keys are only for comparison; original wire roots stay unchanged.
 */
export function normalizedVmNamespace(value: unknown): string | undefined {
  if (typeof value !== "string" || new TextEncoder().encode(value).length > 4096 || /\p{Cc}/u.test(value)) return undefined;
  const raw = value;
  const path = raw.startsWith("\\\\?\\") ? raw.slice(4) : raw;
  const windows = /^[A-Za-z]:[/\\]/.test(path);
  if (!windows && (!raw.startsWith("/") || raw.includes("\\"))) return undefined;
  const parts = (windows ? path.slice(3) : raw.slice(1)).split(windows ? /[/\\]/ : /\//);
  if (parts.some(part => !part || part === "." || part === ".." || windows && (
    /[. ]$/.test(part) || /[<>:"|?*]/.test(part) || /^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(part)
  ))) return undefined;
  // Rust eq_ignore_ascii_case does not fold Unicode characters.
  const asciiFold = (s: string): string => s.replace(/[A-Z]/g, c => c.toLowerCase());
  return windows ? `${asciiFold(path[0]!)}:/${parts.map(asciiFold).join("/")}` : `/${parts.join("/")}`;
}

export function sameVmNamespace(a: unknown, b: unknown): boolean {
  const normalized = normalizedVmNamespace(a);
  return normalized !== undefined && normalized === normalizedVmNamespace(b);
}
