/** Validate the emitted image before reading or changing any PE header field. */
function windowsSubsystemOffset(bytes: Uint8Array): number {
  const pe = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const invalid = () => { throw new Error("Packaged Neoism must be a Windows GUI PE executable (subsystem 2), not a console launcher"); };
  if (bytes.length < 64 || pe.getUint16(0, true) !== 0x5a4d) return invalid();
  const offset = pe.getUint32(0x3c, true);
  // Subsystem has the same offset in PE32 and PE32+ optional headers.
  if (offset > bytes.length - 94 || pe.getUint32(offset, true) !== 0x4550) return invalid();
  const magic = pe.getUint16(offset + 24, true);
  if (![0x10b, 0x20b].includes(magic)) return invalid();
  return offset + 92;
}

/** Inspect the emitted file, not just Bun's requested compile options. */
export function assertWindowsGuiExecutable(bytes: Uint8Array): void {
  const offset = windowsSubsystemOffset(bytes);
  if (new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint16(offset, true) !== 2)
    throw new Error("Packaged Neoism must be a Windows GUI PE executable (subsystem 2), not a console launcher");
}

/** Bun 1.3.13 can retain CUI despite compile.windows.hideConsole. Set the
 * loader's actual subsystem, as EDITBIN /SUBSYSTEM:WINDOWS does, without
 * rewriting/truncating Bun's appended bytecode or changing any other bytes. */
export function configureWindowsGuiExecutable(bytes: Uint8Array): void {
  const offset = windowsSubsystemOffset(bytes), pe = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const subsystem = pe.getUint16(offset, true);
  if (subsystem !== 2 && subsystem !== 3) throw new Error(`Unexpected Windows executable subsystem ${subsystem}`);
  pe.setUint16(offset, 2, true);
  assertWindowsGuiExecutable(bytes);
}