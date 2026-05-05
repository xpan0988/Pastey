interface TrayStatusProps {
  activeCount: number;
}

export function TrayStatus({ activeCount }: TrayStatusProps) {
  return (
    <div className="status-chip">
      <span className={`status-dot ${activeCount > 0 ? "active" : ""}`} />
      {activeCount > 0 ? `${activeCount} active` : "Idle in tray"}
    </div>
  );
}
