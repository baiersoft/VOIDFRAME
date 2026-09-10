import React from "react";
import { AlertTriangle } from "lucide-react";

interface CustomScriptWarningModalProps {
  onAcknowledge: () => void;
  onCancel: () => void;
}

/** Shown once, the first time the user ever adds a `custom_script` module --
 * not per-script or per-run. VOIDFRAME runs elevated and cannot verify what
 * a custom script actually does; this is a one-time heads-up, not a
 * per-use confirmation gate (that turned out to be unpractical friction for
 * a single-user, locally-run app where the user is always the one who
 * wrote the script). */
export const CustomScriptWarningModal: React.FC<CustomScriptWarningModalProps> = ({
  onAcknowledge,
  onCancel,
}) => {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/80 backdrop-blur-xl">
      <div className="glass-panel w-full max-w-md rounded-2xl border border-amber-500/30 overflow-hidden">
        <div className="p-6 border-b border-white/10 flex items-center gap-2">
          <AlertTriangle className="w-4 h-4 text-amber-400" />
          <h2 className="font-sans font-bold text-lg text-white">Custom Scripts Run Elevated</h2>
        </div>
        <div className="p-6 space-y-3">
          <p className="text-xs text-white/70 font-body leading-relaxed">
            Custom scripts run with administrator privileges. VOIDFRAME runs your apply/revert
            scripts but cannot verify what they actually do to your system.
          </p>
          <p className="text-xs text-white/70 font-body leading-relaxed">
            Only add scripts you wrote yourself or trust completely. This warning won&apos;t show
            again.
          </p>
        </div>
        <div className="p-6 border-t border-white/10 flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="px-3.5 py-2 rounded-lg text-xs font-mono text-white/60 hover:text-white cursor-pointer"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onAcknowledge}
            className="px-3.5 py-2 rounded-lg bg-amber-500/20 hover:bg-amber-500/30 text-amber-300 hover:text-white border border-amber-500/40 font-mono text-xs font-semibold transition-all cursor-pointer"
          >
            Got It
          </button>
        </div>
      </div>
    </div>
  );
};
