export interface TerminalOutputUpdate {
  kind: "none" | "append" | "reset";
  data: string;
}

export function terminalInputBytes(data: string): number[] {
  return [...new TextEncoder().encode(data)];
}

export function terminalOutputUpdate(
  previousSnapshot: string,
  currentSnapshot: string,
): TerminalOutputUpdate {
  if (currentSnapshot === previousSnapshot) {
    return { kind: "none", data: "" };
  }
  if (currentSnapshot.startsWith(previousSnapshot)) {
    return {
      kind: "append",
      data: currentSnapshot.slice(previousSnapshot.length),
    };
  }
  return { kind: "reset", data: currentSnapshot };
}
