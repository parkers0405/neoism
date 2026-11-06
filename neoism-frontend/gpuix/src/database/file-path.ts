/** Database routing and file-tree decoration must agree. Sidecars are not databases. */
export function isDatabasePath(path: string): boolean {
  return /\.(db|sqlite|sqlite3|db3|turso|tursodb)$/i.test(path);
}

export function isDatabaseSidecarPath(path: string): boolean {
  return /-(wal|shm)$/i.test(path) && isDatabasePath(path.replace(/-(wal|shm)$/i, ""));
}