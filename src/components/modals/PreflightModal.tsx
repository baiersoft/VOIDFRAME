import React, { useEffect, useState } from "react";
import { X, CheckCircle2, AlertTriangle, XCircle, ShieldCheck } from "lucide-react";
import { preflight } from "../../lib/api";
import type { Check, CheckStatus } from "../../lib/bindings";

interface PreflightModalProps {
  isOpen: boolean;
  onClose: () => void;
  /** The selected project's id, so the power-plan-noop-vs-active check has
   * scenarios to compare against. `null` when no project is selected. */
  projectId: string | null;
}

const STATUS_STYLES: Record<CheckStatus, { badge: string; icon: React.ReactNode; label: string }> = {
  pass: {
    badge: "bg-emerald-500/20 text-emerald-300 border-emerald-500/40",
    icon: <CheckCircle2 className="w-5 h-5 text-emerald-400 shrink-0 mt-0.5" />,
    label: "PASS",
  },
  warn: {
    badge: "bg-amber-500/20 text-amber-300 border-amber-500/40",
    icon: <AlertTriangle className="w-5 h-5 text-amber-400 shrink-0 mt-0.5" />,
    label: "WARN",
  },
  block: {
    badge: "bg-red-500/20 text-red-300 border-red-500/40",
    icon: <XCircle className="w-5 h-5 text-red-400 shrink-0 mt-0.5" />,
    label: "BLOCKED",
  },
  deferred: {
    badge: "bg-white/10 text-white/40 border-white/20",
    icon: <ShieldCheck className="w-5 h-5 text-white/30 shrink-0 mt-0.5" />,
    label: "M3",
  },
};

export const PreflightModal: React.FC<PreflightModalProps> = ({ isOpen, onClose, projectId }) => {
  const [checks, setChecks] = useState<Check[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isOpen) return;
    setIsLoading(true);
    setError(null);
    setChecks([]);
    preflight(projectId)
      .then((report) => setChecks(report.checks))
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setIsLoading(false));
  }, [isOpen, projectId]);

  if (!isOpen) return null;

  const hasBlock = checks.some((c) => c.status === "block");
  const hasWarn = checks.some((c) => c.status === "warn");

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/85 backdrop-blur-xl">
      <div className="glass-panel w-full max-w-3xl max-h-[85vh] rounded-2xl border border-white/10 flex flex-col shadow-[0_0_70px_rgba(0,0,0,0.4)] overflow-hidden">
        <div className="p-6 border-b border-white/10 flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-xl bg-[#06b6d4]/10 border border-[#06b6d4]/30 text-[#06b6d4]">
              <ShieldCheck className="w-5 h-5" />
            </div>
            <div>
              <h2 className="font-sans font-bold text-lg text-white">Pre-Flight Check</h2>
              <p className="text-xs text-white/50 font-mono">Steam, CS2 Install, Workshop Map & Disk Space</p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="p-2 rounded-lg text-white/50 hover:text-white hover:bg-white/5 transition-colors cursor-pointer"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        <div className="p-6 space-y-3 overflow-y-auto flex-1">
          {isLoading && <div className="text-xs font-mono text-white/50">Running pre-flight checks…</div>}
          {error && (
            <div className="p-3 rounded-xl bg-red-500/10 border border-red-500/30 text-red-300 text-xs font-mono">
              {error}
            </div>
          )}
          {checks.map((check) => {
            const style = STATUS_STYLES[check.status];
            return (
              <div
                key={check.id}
                className="glass-card p-4 rounded-xl border-white/10 space-y-1.5 flex items-start gap-3.5"
              >
                {style.icon}
                <div className="space-y-1 flex-1">
                  <div className="flex items-center justify-between gap-2">
                    <span className="px-1.5 py-0.5 rounded text-[9px] font-mono uppercase bg-white/5 text-white/50 border border-white/10">
                      {check.id}
                    </span>
                    <span className={`px-2 py-0.5 rounded text-[9px] font-mono font-bold border uppercase ${style.badge}`}>
                      {style.label}
                    </span>
                  </div>
                  <p className="text-xs text-white/60 font-body leading-relaxed">{check.detail}</p>
                </div>
              </div>
            );
          })}
        </div>

        <div className="p-6 border-t border-white/10 bg-black/40 flex items-center justify-between">
          <div
            className={`text-xs font-mono flex items-center gap-2 ${
              error ? "text-red-400" : hasBlock ? "text-red-400" : hasWarn ? "text-amber-400" : "text-emerald-400"
            }`}
          >
            {error ? (
              <>
                <XCircle className="w-4 h-4" /> Pre-flight checks could not run
              </>
            ) : hasBlock ? (
              <>
                <XCircle className="w-4 h-4" /> Run is blocked — resolve the issue(s) above
              </>
            ) : hasWarn ? (
              <>
                <AlertTriangle className="w-4 h-4" /> Warnings present — review before running
              </>
            ) : (
              <>
                <CheckCircle2 className="w-4 h-4" /> All checks passed
              </>
            )}
          </div>
          <button
            onClick={onClose}
            className="px-5 py-2 rounded-xl bg-white/10 hover:bg-white/15 text-white font-mono text-xs font-semibold cursor-pointer"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
};
