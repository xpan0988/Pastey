export type OperationTimelineStatus = "pending" | "active" | "complete" | "failed" | "denied";

export interface OperationTimelineStep {
  id: string;
  label: string;
  status: OperationTimelineStatus;
  detail?: string;
  timestamp?: string;
  advancedMetadata?: Record<string, string>;
}

export interface OperationTimelineRow {
  id: string;
  label: string;
  status: OperationTimelineStatus;
  detail?: string;
  timestamp?: string;
  advancedMetadata?: Record<string, string>;
}

interface OperationTimelineProps {
  label: string;
  steps: OperationTimelineStep[];
  rows?: OperationTimelineRow[];
}

export function OperationTimeline({ label, steps, rows = [] }: OperationTimelineProps) {
  if (steps.length === 0 && rows.length === 0) return null;
  return (
    <section className="operation-timeline" aria-label={label}>
      <div className="operation-timeline-heading">
        <span className="agent-bridge-status-label">Operation details</span>
        <strong>Steps</strong>
      </div>
      {steps.length > 0 ? (
        <ol className="operation-step-list">
          {steps.map((step) => (
            <li key={step.id} className={`operation-step ${operationStatusClass(step.status)}`}>
              <span>{step.label}</span>
              {step.detail ? <small>{step.detail}</small> : null}
            </li>
          ))}
        </ol>
      ) : null}
      {rows.length > 0 ? (
        <div className="operation-lifecycle-rows">
          {rows.map((row) => (
            <div key={row.id} className={`operation-lifecycle-row ${operationStatusClass(row.status)}`}>
              <div>
                <strong>{row.label}</strong>
                {row.detail ? <span className="muted">{row.detail}</span> : null}
              </div>
              <span className="operation-row-status">{operationStatusLabel(row.status)}</span>
            </div>
          ))}
        </div>
      ) : null}
    </section>
  );
}

function operationStatusClass(status: OperationTimelineStatus): string {
  return `operation-status-${status}`;
}

function operationStatusLabel(status: OperationTimelineStatus): string {
  switch (status) {
    case "active":
      return "Active";
    case "complete":
      return "Complete";
    case "failed":
      return "Failed";
    case "denied":
      return "Denied";
    case "pending":
    default:
      return "Pending";
  }
}
