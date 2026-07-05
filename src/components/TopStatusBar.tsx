export interface TopStatusBarProps {
  thisDevice: string;
  thisDeviceStatus: string;
  peerDiscovery: string;
  peerDiscoveryStatus: string;
  approvalsCount: number;
  queueCount: number;
}

export function TopStatusBar({
  thisDevice,
  thisDeviceStatus,
  peerDiscovery,
  peerDiscoveryStatus,
  approvalsCount,
  queueCount,
}: TopStatusBarProps) {
  return (
    <header className="top-status-bar" data-testid="top-status-bar">
      <StatusMetric
        icon="device"
        label="This device"
        value={thisDevice}
        detail={thisDeviceStatus}
      />
      <StatusMetric
        icon="peers"
        label="Peer discovery"
        value={peerDiscovery}
        detail={peerDiscoveryStatus}
      />
      <StatusMetric
        icon="approvals"
        label="Inbox"
        value={String(approvalsCount)}
        detail={approvalsCount === 1 ? "Pending" : "Pending"}
      />
      <StatusMetric
        icon="queue"
        label="Queue"
        value={String(queueCount)}
        detail={queueCount === 1 ? "Active" : "Active"}
      />
    </header>
  );
}

function StatusMetric({
  icon,
  label,
  value,
  detail,
}: {
  icon: "device" | "peers" | "approvals" | "queue";
  label: string;
  value: string;
  detail: string;
}) {
  return (
    <div className="top-status-metric">
      <span className={`top-status-icon ${icon}`} aria-hidden="true" />
      <div>
        <span>{label}</span>
        <strong>{value}</strong>
        <small>{detail}</small>
      </div>
    </div>
  );
}
