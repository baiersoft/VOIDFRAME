import React, { useState } from "react";
import { Save } from "lucide-react";

interface LaunchArgsEditorProps {
  /** Pre-fills the textarea -- the live current launch options (add flow)
   * or the module's already-stored value (edit flow), decided by the
   * caller. Never includes VOIDFRAME's own reserved tokens. */
  initialValue: string;
  onSave: (args: string) => void;
}

export const LaunchArgsEditor: React.FC<LaunchArgsEditorProps> = ({ initialValue, onSave }) => {
  const [value, setValue] = useState(initialValue);

  return (
    <div className="space-y-2">
      <textarea
        value={value}
        onChange={(e) => setValue(e.target.value)}
        rows={2}
        placeholder="e.g. -high -threads 8"
        aria-label="Launch options"
        className="w-full px-3.5 py-2.5 rounded-lg glass-input text-xs font-mono text-white/90 resize-none"
      />
      <div className="flex justify-end">
        <button
          type="button"
          onClick={() => onSave(value)}
          className="px-3.5 py-1.5 rounded-lg bg-[#06b6d4]/20 hover:bg-[#06b6d4]/30 text-[#22d3ee] hover:text-white border border-[#06b6d4]/40 font-mono text-xs font-semibold flex items-center gap-1.5 transition-all cursor-pointer"
        >
          <Save className="w-3.5 h-3.5" />
          <span>Save</span>
        </button>
      </div>
    </div>
  );
};
