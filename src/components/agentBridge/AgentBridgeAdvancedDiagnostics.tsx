import type { ReactNode } from "react";

interface AgentBridgeAdvancedDiagnosticsProps {
  children: ReactNode;
}

export function AgentBridgeAdvancedDiagnostics({
  children,
}: AgentBridgeAdvancedDiagnosticsProps) {
  return (
    <details className="agent-bridge-advanced" data-testid="agent-bridge-advanced-diagnostics">
      <summary>Advanced diagnostics</summary>
      <div className="agent-bridge-advanced-content">
        <section className="ai-slot-advisory-notice">
          <strong>Advanced safety notes</strong>
          <span>Provider output is untrusted and must pass validation and PolicyGate.</span>
          <span>Trusted room membership and preview acknowledgement are not execution authorization.</span>
          <span>Cloud context is redacted and current-session only.</span>
          <span>No raw shell, file access, hidden transfer, or generic peer runtime exists.</span>
          <span>CL-4 scheduler reservation changes only sender-side data-window allocation for outgoing control demand.</span>
        </section>
        {children}
      </div>
    </details>
  );
}
