import React, { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen, Save } from "lucide-react";
import type { CustomScriptPayload } from "../../lib/bindings";
import { importCustomScript } from "../../lib/api";

interface CustomScriptEditorProps {
  projectId: string;
  /** `null` for a brand-new module. */
  initialValue: CustomScriptPayload | null;
  onSave: (payload: CustomScriptPayload) => void;
}

const DIALOG_FILTERS = [{ name: "Scripts", extensions: ["bat", "cmd", "ps1"] }];

export const CustomScriptEditor: React.FC<CustomScriptEditorProps> = ({
  projectId,
  initialValue,
  onSave,
}) => {
  const [applyScript, setApplyScript] = useState(initialValue?.apply_script ?? "");
  const [revertScript, setRevertScript] = useState(initialValue?.revert_script ?? "");
  const [description, setDescription] = useState(initialValue?.description ?? "");
  const [requiresReboot, setRequiresReboot] = useState(initialValue?.requires_reboot ?? false);
  const [error, setError] = useState<string | null>(null);

  const browseFor = async (which: "apply" | "revert") => {
    setError(null);
    try {
      const path = await open({ multiple: false, filters: DIALOG_FILTERS });
      if (!path || Array.isArray(path)) return;
      const filename = await importCustomScript(projectId, path);
      if (which === "apply") setApplyScript(filename);
      else setRevertScript(filename);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const canSave = applyScript !== "" && revertScript !== "" && description.trim() !== "";

  const handleSave = () => {
    onSave({
      apply_script: applyScript,
      revert_script: revertScript,
      requires_reboot: requiresReboot,
      description: description.trim(),
    });
  };

  return (
    <div className="space-y-2.5">
      {error && <div className="text-[11px] font-mono text-red-400">{error}</div>}

      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={() => browseFor("apply")}
          aria-label="Browse for apply script"
          className="px-2.5 py-1.5 rounded-lg glass-pill text-[11px] font-mono text-[#22d3ee] flex items-center gap-1.5 cursor-pointer"
        >
          <FolderOpen className="w-3.5 h-3.5" /> Browse (Apply)
        </button>
        <span className="text-xs font-mono text-white/70">
          {applyScript || <span className="text-white/30">(none selected)</span>}
        </span>
      </div>

      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={() => browseFor("revert")}
          aria-label="Browse for revert script"
          className="px-2.5 py-1.5 rounded-lg glass-pill text-[11px] font-mono text-[#22d3ee] flex items-center gap-1.5 cursor-pointer"
        >
          <FolderOpen className="w-3.5 h-3.5" /> Browse (Revert)
        </button>
        <span className="text-xs font-mono text-white/70">
          {revertScript || <span className="text-white/30">(none selected -- required)</span>}
        </span>
      </div>

      <label className="block space-y-1 text-[10px] uppercase text-white/40">
        Description
        <textarea
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          rows={2}
          className="w-full px-3.5 py-2.5 rounded-lg glass-input text-xs font-mono text-white/90 resize-none normal-case"
        />
      </label>

      <label className="flex items-center gap-2 text-[11px] font-mono text-white/60">
        <input
          type="checkbox"
          checked={requiresReboot}
          onChange={(e) => setRequiresReboot(e.target.checked)}
        />
        Requires Reboot
      </label>

      <div className="flex justify-end">
        <button
          type="button"
          disabled={!canSave}
          onClick={handleSave}
          className="px-3.5 py-1.5 rounded-lg bg-[#06b6d4]/20 hover:bg-[#06b6d4]/30 text-[#22d3ee] hover:text-white border border-[#06b6d4]/40 font-mono text-xs font-semibold flex items-center gap-1.5 transition-all cursor-pointer disabled:cursor-not-allowed disabled:opacity-40"
        >
          <Save className="w-3.5 h-3.5" />
          <span>Save</span>
        </button>
      </div>
    </div>
  );
};
