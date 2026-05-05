import { formatCode } from "../lib/format";

interface CodeDisplayProps {
  code?: string | null;
}

export function CodeDisplay({ code }: CodeDisplayProps) {
  return <div className="code-display">{formatCode(code)}</div>;
}
