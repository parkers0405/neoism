/** Inspect the emitted file, not just Bun's requested compile options. */
export function assertWindowsGuiExecutable(bytes: Uint8Array): void {
  const pe = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const invalid = () => { throw new Error("Packaged Neoism must be a Windows GUI PE executable (subsystem 2), not a console launcher"); };
  if (bytes.length < 64 || pe.getUint16(0, true) !== 0x5a4d) return invalid();
  const offset = pe.getUint32(0x3c, true);
  // Subsystem has the same offset in PE32 and PE32+ optional headers.
  if (offset > bytes.length - 94 || pe.getUint32(offset, true) !== 0x4550) return invalid();
  const magic = pe.getUint16(offset + 24, true);
  if (![0x10b, 0x20b].includes(magic) || pe.getUint16(offset + 92, true) !== 2) invalid();
}