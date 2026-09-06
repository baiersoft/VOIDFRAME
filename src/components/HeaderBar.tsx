import React from "react";
import { ShieldCheck, Settings, RotateCcw, Activity } from "lucide-react";

interface HeaderBarProps {
  onOpenSettings: () => void;
  onOpenPreflight: () => void;
  onEmergencyRestore: () => void;
}

export const HeaderBar: React.FC<HeaderBarProps> = ({
  onOpenSettings,
  onOpenPreflight,
  onEmergencyRestore,
}) => {
  return (
    <header className="relative z-20 border-b border-white/[0.07] bg-[#010103]/80 backdrop-blur-xl px-6 py-2.5 flex items-center justify-between">
      {/* Left: Active Workspace / System State */}
      <div className="flex items-center gap-3">
        <div className="flex items-center gap-2 text-xs font-mono">
          <span className="w-2 h-2 rounded-full bg-[#06b6d4] animate-pulse" />
          <span className="text-white/40 uppercase tracking-wider">Engine //</span>
          <span className="text-white/60">CS2 Benchmark Matrix Active</span>
        </div>
      </div>

      {/* Right: Primary Action Controls */}
      <div className="flex items-center gap-2">
        <button
          onClick={onEmergencyRestore}
          title="Emergency Restore System to Baseline Snapshot"
          className="glass-pill px-3.5 py-1.5 rounded-lg text-xs font-mono text-amber-300 hover:text-amber-100 border-amber-500/30 hover:border-amber-500/60 bg-amber-500/10 hover:bg-amber-500/20 flex items-center gap-1.5 transition-all cursor-pointer shadow-[0_0_15px_rgba(245,181,68,0.1)]"
        >
          <RotateCcw className="w-3.5 h-3.5 text-amber-400" />
          <span>Emergency Restore</span>
        </button>

        <button
          onClick={onOpenPreflight}
          title="Pre-Flight System & AutoLogon Diagnostics"
          className="glass-pill px-3.5 py-1.5 rounded-lg text-xs font-mono text-[#22d3ee] hover:text-white border-[#06b6d4]/30 hover:border-[#06b6d4]/60 bg-[#06b6d4]/10 hover:bg-[#06b6d4]/20 flex items-center gap-1.5 transition-all cursor-pointer shadow-[0_0_15px_rgba(6,182,212,0.1)]"
        >
          <ShieldCheck className="w-3.5 h-3.5 text-[#06b6d4]" />
          <span>Pre-Flight</span>
        </button>

        <button
          onClick={onOpenSettings}
          title="Settings & Config Paths"
          className="glass-pill p-2 rounded-lg text-white/60 hover:text-white border-white/10 hover:border-white/20 transition-colors cursor-pointer"
        >
          <Settings className="w-4 h-4" />
        </button>
      </div>
    </header>
  );
};
