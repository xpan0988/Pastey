export type PrimaryView =
  | "bridge"
  | "devices"
  | "transfers"
  | "inbox"
  | "settings";

interface PrimarySidebarProps {
  activeView: PrimaryView;
  onSelectView: (view: PrimaryView) => void;
}

const NAV_ITEMS: Array<{ view: PrimaryView; label: string; icon: string }> = [
  { view: "bridge", label: "Bridge", icon: "home" },
  { view: "devices", label: "Devices", icon: "devices" },
  { view: "transfers", label: "Transfers", icon: "activity" },
  { view: "inbox", label: "Inbox", icon: "approvals" },
  { view: "settings", label: "Settings", icon: "settings" },
];

export function PrimarySidebar({ activeView, onSelectView }: PrimarySidebarProps) {
  return (
    <aside className="primary-sidebar">
      <div className="brand-lockup" aria-label="Pastey">
        <span className="brand-mark" aria-hidden="true">P</span>
        <strong>Pastey</strong>
      </div>
      <nav className="primary-nav" aria-label="Primary navigation">
        {NAV_ITEMS.map((item) => (
          <button
            key={item.view}
            type="button"
            className={`primary-nav-item ${activeView === item.view ? "active" : ""}`}
            aria-current={activeView === item.view ? "page" : undefined}
            onClick={() => onSelectView(item.view)}
          >
            <span className={`primary-nav-icon ${item.icon}`} aria-hidden="true" />
            <span>{item.label}</span>
          </button>
        ))}
      </nav>
      <div className="sidebar-security-note">
        <span className="sidebar-shield" aria-hidden="true" />
        <strong>Secure by design</strong>
      </div>
    </aside>
  );
}
