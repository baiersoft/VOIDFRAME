import React from "react";
import { LayoutGrid, Sliders, PlayCircle, BarChart3, BookOpen } from "lucide-react";

export type NavTab = "dashboard" | "builder" | "monitor" | "results" | "library";

export interface TabVisibilityFlags {
  hasProject: boolean;
  hasRun: boolean;
  hasResults: boolean;
}

/** The single source of truth for which tabs are currently meaningful --
 * shared between this component's own tab list and App.tsx's fallback
 * redirect when the active tab stops being valid. */
export function isTabVisible(tab: NavTab, flags: TabVisibilityFlags): boolean {
  switch (tab) {
    case "builder":
      return flags.hasProject;
    case "monitor":
      return flags.hasRun;
    case "results":
      return flags.hasResults;
    case "dashboard":
    case "library":
      return true;
  }
}

interface NavigationTabsProps {
  activeTab: NavTab;
  onSelectTab: (tab: NavTab) => void;
  /** Gates the Matrix Builder tab -- it only makes sense once a project is selected. */
  hasProject?: boolean;
  /**
   * Gates the Live Monitor tab's VISIBILITY -- a run has started and the
   * user hasn't left it yet (App.tsx: `activeRunId !== null`). Deliberately
   * NOT the same as `isRunning`: that flips false the instant the run
   * finishes, which would yank the tab away right when the user is looking
   * at the completion banner. `hasRun` only clears once they explicitly
   * leave via "Back to Builder".
   */
  hasRun?: boolean;
  /** The "ACTIVE" badge only -- true strictly while a run is in progress. */
  isRunning?: boolean;
  /** Gates the Visualizer & Charts tab -- only shown once there are results to view. */
  hasResults?: boolean;
}

export const NavigationTabs: React.FC<NavigationTabsProps> = ({
  activeTab,
  onSelectTab,
  hasProject = false,
  hasRun = false,
  isRunning = false,
  hasResults = false,
}) => {
  const flags: TabVisibilityFlags = { hasProject, hasRun, hasResults };
  const allTabs = [
    {
      id: "dashboard" as NavTab,
      label: "Project Explorer",
      icon: LayoutGrid,
    },
    {
      id: "builder" as NavTab,
      label: "Matrix Builder",
      icon: Sliders,
    },
    {
      id: "monitor" as NavTab,
      label: "Live Monitor",
      icon: PlayCircle,
      badge: isRunning ? "ACTIVE" : undefined,
      badgeColor: "bg-cyan-500/20 text-cyan-300 border-cyan-500/40 animate-pulse",
    },
    {
      id: "results" as NavTab,
      label: "Visualizer & Charts",
      icon: BarChart3,
      badge: hasResults ? "RESULTS" : undefined,
      badgeColor: "bg-purple-500/20 text-purple-300 border-purple-500/40",
    },
    {
      id: "library" as NavTab,
      label: "Verified Tweaks",
      icon: BookOpen,
    },
  ];
  const tabs = allTabs.filter((tab) => isTabVisible(tab.id, flags));

  return (
    <nav aria-label="Main Navigation" className="relative z-10 border-b border-white/[0.06] bg-[#010103]/60 px-6 py-2 flex items-center justify-between">
      <div className="flex items-center gap-1.5 overflow-x-auto no-scrollbar">
        {tabs.map((tab) => {
          const Icon = tab.icon;
          const isActive = activeTab === tab.id;
          return (
            <button
              key={tab.id}
              onClick={() => onSelectTab(tab.id)}
              className={`px-4 py-2 rounded-xl font-mono text-xs flex items-center gap-2 transition-all cursor-pointer ${
                isActive
                  ? "bg-white/[0.07] text-white border border-[#06b6d4]/40 shadow-[0_0_15px_rgba(6,182,212,0.15)] font-semibold"
                  : "text-white/50 hover:text-white/90 hover:bg-white/[0.02] border border-transparent"
              }`}
            >
              <Icon className={`w-3.5 h-3.5 ${isActive ? "text-[#22d3ee]" : "text-white/40"}`} />
              <span>{tab.label}</span>
              {tab.badge && (
                <span
                  className={`px-1.5 py-0.2 rounded text-[9px] font-bold tracking-wider border ${tab.badgeColor}`}
                >
                  {tab.badge}
                </span>
              )}
            </button>
          );
        })}
      </div>
    </nav>
  );
};
