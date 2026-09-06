import React, { useState } from "react";
import { AlertTriangle, ShieldAlert, CheckCircle2, X } from "lucide-react";

interface DDUWarningModalProps {
  isOpen: boolean;
  onClose: () => void;
  onConfirm: () => void;
}

export const DDUWarningModal: React.FC<DDUWarningModalProps> = ({
  isOpen,
  onClose,
  onConfirm,
}) => {
  const [acknowledged, setAcknowledged] = useState(false);

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/85 backdrop-blur-xl">
      <div className="glass-panel w-full max-w-xl rounded-2xl border border-amber-500/40 flex flex-col shadow-[0_0_60px_rgba(245,158,11,0.2)] overflow-hidden">
        {/* Header */}
        <div className="p-6 border-b border-white/10 flex items-center justify-between bg-amber-500/10">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-xl bg-amber-500/20 border border-amber-500/40 text-amber-400">
              <ShieldAlert className="w-6 h-6" />
            </div>
            <div>
              <h2 className="font-sans font-bold text-lg text-white">
                DDU Safe-Mode Deep Clean Warning
              </h2>
              <p className="text-xs text-amber-300 font-mono">
                Automated Driver Purge & Safe-Boot Cycle
              </p>
            </div>
          </div>

          <button
            onClick={onClose}
            className="p-2 rounded-lg text-white/50 hover:text-white hover:bg-white/5 transition-colors cursor-pointer"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Content */}
        <div className="p-6 space-y-4 text-xs font-mono text-white/80 leading-relaxed">
          <div className="p-4 rounded-xl bg-black/50 border border-amber-500/20 space-y-2">
            <div className="flex items-center gap-2 text-amber-400 font-bold uppercase tracking-wider text-[11px]">
              <AlertTriangle className="w-4 h-4" />
              <span>Safe-Mode Execution Mechanics</span>
            </div>
            <p className="text-white/70 font-sans">
              VOIDFRAME will configure <code className="text-[#22d3ee]">bcdedit safeboot minimal</code> and reboot into Windows Safe Mode. DDU will execute silently via Task Scheduler, remove all NVIDIA drivers and registry remnants, restore standard boot, and restart Windows before installing your clean driver bundle.
            </p>
          </div>

          <div className="space-y-2 text-white/60 font-sans">
            <h4 className="font-bold text-white font-mono uppercase text-[11px]">Pre-Flight Requirements:</h4>
            <ul className="list-disc pl-5 space-y-1">
              <li>Windows Account must be a passwordless local account or have AutoLogon configured.</li>
              <li>BitLocker recovery key should be backed up if system drive is encrypted.</li>
              <li>CS2 Shader Warmup runs (3-5 loops) will be enforced automatically after driver install.</li>
            </ul>
          </div>

          {/* Acknowledgement Checkbox */}
          <label className="flex items-start gap-3 p-3.5 rounded-xl bg-white/[0.03] border border-white/10 hover:border-white/20 transition-all cursor-pointer">
            <input
              type="checkbox"
              checked={acknowledged}
              onChange={(e) => setAcknowledged(e.target.checked)}
              className="mt-0.5 rounded border-white/20 text-[#06b6d4] focus:ring-[#06b6d4] cursor-pointer"
            />
            <span className="text-xs text-white/90 font-sans">
              I understand that DDU will execute an automated Safe-Mode purge cycle and agree to proceed.
            </span>
          </label>
        </div>

        {/* Footer */}
        <div className="p-6 border-t border-white/10 bg-black/30 flex items-center justify-between">
          <button
            onClick={onClose}
            className="glass-pill px-4 py-2 rounded-xl text-xs font-mono text-white/60 hover:text-white cursor-pointer"
          >
            Cancel
          </button>

          <button
            disabled={!acknowledged}
            onClick={() => {
              onConfirm();
              onClose();
            }}
            className={`px-5 py-2 rounded-xl font-mono text-xs font-bold flex items-center gap-1.5 transition-all ${
              acknowledged
                ? "bg-gradient-to-r from-amber-500 to-amber-600 text-white cursor-pointer shadow-[0_0_20px_rgba(245,158,11,0.4)]"
                : "bg-white/10 text-white/30 cursor-not-allowed"
            }`}
          >
            <CheckCircle2 className="w-4 h-4" />
            <span>Enable DDU Deep Clean</span>
          </button>
        </div>
      </div>
    </div>
  );
};
