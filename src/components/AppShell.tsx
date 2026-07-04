import type { ReactNode } from "react";
import { PrimarySidebar, type PrimaryView } from "./PrimarySidebar";
import { TopStatusBar, type TopStatusBarProps } from "./TopStatusBar";

interface AppShellProps {
  activeView: PrimaryView;
  topStatus: TopStatusBarProps;
  children: ReactNode;
  onSelectView: (view: PrimaryView) => void;
}

export function AppShell({
  activeView,
  topStatus,
  children,
  onSelectView,
}: AppShellProps) {
  return (
    <div className="workstation-shell" data-testid="app-shell">
      <PrimarySidebar activeView={activeView} onSelectView={onSelectView} />
      <div className="workstation-main">
        <TopStatusBar {...topStatus} />
        <div className="workstation-body">
          <section className="workstation-content" aria-live="polite">
            {children}
          </section>
        </div>
      </div>
    </div>
  );
}
