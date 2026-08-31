type PatchState = "added" | "changed" | "removed";

interface PatchRow {
  readonly id: number;
  state: PatchState;
  label: string;
}

const states: PatchState[] = ["added", "changed", "removed"];

export const rows: PatchRow[] = states.map((state, index) => ({
  id: index + 1,
  state,
  label: `${index + 1}: ${state.toUpperCase()}`,
}));

export function findRow(state: PatchState): PatchRow | undefined {
  return rows.find((row) => row.state === state);
}