import React, { useRef, useState } from "react";
import { AlertOctagon, X, FolderOpen, FileCode2 } from "lucide-react";
import { emergencyRollback, openDataDir, revealRestoreBat } from "../../lib/api";
import type { RevertReport } from "../../lib/bindings";

interface EmergencyRestoreModalProps {
  isOpen: boolean;
  onClose: () => void;
}

type Phase = "confirm" | "running" | "done" | "error";

export const EmergencyRestoreModal: React.FC<EmergencyRestoreModalProps> = ({ isOpen, onClose }) => {
  const [phase, setPhase] = useState<Phase>("confirm");
  const [reports, setReports] = useState<RevertReport[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const cancelledRef = useRef(false);

  if (!isOpen) return null;

  const handleClose = () => {
    cancelledRef.current = true;
    setPhase("confirm");
    setReports([]);
    setError(null);
    setActionError(null);
    onClose();
  };

  const handleConfirm = async () => {
    cancelledRef.current = false;
    setPhase("running");
    setError(null);
    try {
      const result = await emergencyRollback();
      if (cancelledRef.current) return;
      setReports(result);
      setPhase("done");
    } catch (e) {
      if (cancelledRef.current) return;
      setError(e instanceof Error ? e.message : String(e));
      setPhase("error");
    }
  };

  const handleOpenDataDir = async () => {
    setActionError(null);
    try {
      await openDataDir();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleRevealRestoreBat = async () => {
    setActionError(null);
    try {
      await revealRestoreBat();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/85 backdrop-blur-xl">
      <div className="glass-panel w-full max-w-lg rounded-2xl border border-red-500/30 flex flex-col shadow-[0_0_70px_rgba(0,0,0,0.4)] overflow-hidden">
        <div className="p-6 border-b border-white/10 flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-xl bg-red-500/10 border border-red-500/30 text-red-400">
              <AlertOctagon className="w-5 h-5" />
            </div>
            <h2 className="font-sans font-bold text-lg text-white">Emergency Restore</h2>
          </div>
          <button onClick={handleClose} className="p-2 rounded-lg text-white/50 hover:text-white hover:bg-white/5 cursor-pointer">
            <X className="w-5 h-5" />
          </button>
        </div>

        <div className="p-6 space-y-4">
          {phase === "confirm" && (
            <>
              <p className="text-xs text-white/70 font-body leading-relaxed">
                Scans for the most recently modified run and reverts every mutation recorded in
                its journals (registry values, power plan, CPU affinity, launch args) back to
                their pre-run values. Use this if a run crashed or the app was closed mid-run and
                the normal end-of-run rollback never happened.
              </p>
              <div className="flex items-center gap-2">
                <button
                  onClick={handleOpenDataDir}
                  className="px-3 py-1.5 rounded-lg glass-pill text-[10px] font-mono text-white/70 flex items-center gap-1.5 cursor-pointer"
                >
                  <FolderOpen className="w-3.5 h-3.5" /> Open Data Folder
                </button>
                <button
                  onClick={handleRevealRestoreBat}
                  className="px-3 py-1.5 rounded-lg glass-pill text-[10px] font-mono text-white/70 flex items-center gap-1.5 cursor-pointer"
                >
                  <FileCode2 className="w-3.5 h-3.5" /> Reveal Restore Script
                </button>
              </div>
              {actionError && (
                <div className="p-3 rounded-xl bg-red-500/10 border border-red-500/30 text-red-300 text-xs font-mono">
                  {actionError}
                </div>
              )}
            </>
          )}
          {phase === "running" && (
            <p className="text-xs text-white/50 font-mono">Reverting every journal in the most recent run…</p>
          )}
          {phase === "done" && (
            <div className="space-y-2">
              {reports.map((r, i) => (
                <div key={i} className="glass-card p-3 rounded-xl border-white/10 text-xs font-mono space-y-1">
                  <div className="text-emerald-400">{r.reverted} mutation(s) reverted</div>
                  {r.verify_failures.length > 0 && (
                    <div className="text-red-400">
                      {r.verify_failures.length} did not verify: {r.verify_failures.join("; ")}
                    </div>
                  )}
                </div>
              ))}
              {reports.length === 0 && (
                <p className="text-xs text-white/50 font-mono">Nothing to revert.</p>
              )}
            </div>
          )}
          {phase === "error" && (
            <div className="p-3 rounded-xl bg-red-500/10 border border-red-500/30 text-red-300 text-xs font-mono">
              {error}
            </div>
          )}
        </div>

        <div className="p-6 border-t border-white/10 bg-black/40 flex items-center justify-end gap-3">
          <button onClick={handleClose} className="px-5 py-2 rounded-xl bg-white/10 hover:bg-white/15 text-white font-mono text-xs font-semibold cursor-pointer">
            Close
          </button>
          {phase === "confirm" && (
            <button
              onClick={handleConfirm}
              className="px-5 py-2 rounded-xl bg-red-500/20 hover:bg-red-500/30 text-red-300 border border-red-500/40 font-mono text-xs font-semibold cursor-pointer"
            >
              Execute Emergency Restore
            </button>
          )}
        </div>
      </div>
    </div>
  );
};
