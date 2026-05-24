export type TabKey = "devices" | "rooms" | "settings";

interface BottomTabBarProps {
  activeTab: TabKey;
  onSelectTab: (tab: TabKey) => void;
}

const TABS: Array<{ key: TabKey; label: string; iconClass: string }> = [
  { key: "devices", label: "Devices", iconClass: "tab-icon-devices" },
  { key: "rooms", label: "Rooms", iconClass: "tab-icon-rooms" },
  { key: "settings", label: "Settings", iconClass: "tab-icon-settings" }
];

export function BottomTabBar({ activeTab, onSelectTab }: BottomTabBarProps) {
  return (
    <nav className="bottom-tab-shell" aria-label="Primary navigation">
      <div className="bottom-tab-bar">
        {TABS.map((tab) => (
          <button
            key={tab.key}
            className={`bottom-tab ${activeTab === tab.key ? "active" : ""}`}
            onClick={() => onSelectTab(tab.key)}
            aria-current={activeTab === tab.key ? "page" : undefined}
          >
            <span className={`tab-icon ${tab.iconClass}`} aria-hidden="true" />
            <span>{tab.label}</span>
          </button>
        ))}
      </div>
    </nav>
  );
}
