interface AgentBridgeOverviewProps {
  workflowStatus: string;
  summary: string;
  error?: string;
  actionLabel: string;
  actionDisabled?: boolean;
  onAction: () => void;
  secondaryActionLabel?: string;
  onSecondaryAction?: () => void;
}

export function AgentBridgeOverview({
  workflowStatus,
  summary,
  error,
  actionLabel,
  actionDisabled = false,
  onAction,
  secondaryActionLabel,
  onSecondaryAction,
}: AgentBridgeOverviewProps) {
  return (
    <section className="agent-bridge-section" aria-labelledby="agent-bridge-overview-title">
      <div className="agent-bridge-section-header">
        <div>
          <strong id="agent-bridge-overview-title">Overview</strong>
          <p className="muted">Advisory planning and the next explicit preview action.</p>
        </div>
        <span className="ai-slot-pending-status">{workflowStatus}</span>
      </div>
      <div className="agent-bridge-status-row">
        <span className="agent-bridge-status-label">Workflow</span>
        <strong>{workflowStatus}</strong>
        <span className="muted">{error ?? summary}</span>
      </div>
      <div className="benchmark-controls">
        <button
          className="secondary-button"
          data-testid="agent-bridge-next-action"
          disabled={actionDisabled}
          onClick={onAction}
        >
          {actionLabel}
        </button>
        {secondaryActionLabel && onSecondaryAction ? (
          <button className="secondary-button" onClick={onSecondaryAction}>
            {secondaryActionLabel}
          </button>
        ) : null}
      </div>
    </section>
  );
}
