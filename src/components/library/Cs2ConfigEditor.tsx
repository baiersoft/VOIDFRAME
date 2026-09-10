import React, { useState } from "react";
import { ChevronDown, ChevronRight, Plus, Save, Trash2 } from "lucide-react";
import {
  CS2_VIDEO_SETTINGS,
  CURATED_KEYS,
  optionForSettings,
  settingsForOption,
} from "../../lib/cs2VideoSettings";

interface Cs2ConfigEditorProps {
  /** The module's current `settings` map -- empty `{}` for a brand-new module. */
  initialValue: Record<string, string>;
  onSave: (settings: Record<string, string>) => void;
}

interface RawRow {
  key: string;
  value: string;
}

function rawRowsFrom(settings: Record<string, string>): RawRow[] {
  return Object.entries(settings)
    .filter(([key]) => !CURATED_KEYS.has(key))
    .map(([key, value]) => ({ key, value }));
}

export const Cs2ConfigEditor: React.FC<Cs2ConfigEditorProps> = ({ initialValue, onSave }) => {
  const [settings, setSettings] = useState<Record<string, string>>(initialValue);
  const [rawRows, setRawRows] = useState<RawRow[]>(rawRowsFrom(initialValue));
  // Collapsed by default -- most users never touch this, and pre-filling
  // from a real cs2_video.txt can surface a dozen+ uncurated keys at once,
  // which reads as noise/clutter ahead of the 14 curated settings above it.
  const [showRaw, setShowRaw] = useState(false);

  const handleCuratedChange = (settingLabel: string, optionLabel: string) => {
    const setting = CS2_VIDEO_SETTINGS.find((s) => s.label === settingLabel);
    if (!setting) return;
    if (optionLabel === "") {
      // "Not Set": drop this setting's keys entirely.
      setSettings((prev) => {
        const next = { ...prev };
        setting.keys.forEach((k) => delete next[k]);
        return next;
      });
      return;
    }
    const option = setting.options.find((o) => o.label === optionLabel);
    if (!option) return;
    setSettings((prev) => ({ ...prev, ...settingsForOption(setting, option) }));
  };

  const handleSave = () => {
    // Curated keys come from `settings`; every uncurated key comes from the
    // raw rows alone. `settings` still carries the uncurated keys it was
    // seeded with from `initialValue`, so spreading it whole would resurrect
    // a raw row the user deleted or renamed in this session.
    const merged = Object.fromEntries(
      Object.entries(settings).filter(([key]) => CURATED_KEYS.has(key))
    );
    for (const row of rawRows) {
      if (row.key.trim() !== "") merged[row.key.trim()] = row.value;
    }
    onSave(merged);
  };

  return (
    <div className="space-y-3">
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-2.5">
        {CS2_VIDEO_SETTINGS.map((setting) => {
          const selected = optionForSettings(setting, settings);
          return (
            <label key={setting.label} className="space-y-1 text-[10px] uppercase text-white/40">
              {setting.label}
              <select
                aria-label={setting.label}
                value={selected?.label ?? ""}
                onChange={(e) => handleCuratedChange(setting.label, e.target.value)}
                className="w-full px-2.5 py-1.5 rounded-lg glass-input text-xs font-mono text-white/90 normal-case"
              >
                <option value="">Not Set</option>
                {setting.options.map((o) => (
                  <option key={o.label} value={o.label}>
                    {o.label}
                  </option>
                ))}
              </select>
            </label>
          );
        })}
      </div>

      <div className="space-y-1.5 pt-2 border-t border-white/5">
        <button
          type="button"
          onClick={() => setShowRaw((prev) => !prev)}
          aria-expanded={showRaw}
          className="w-full flex items-center justify-between text-[10px] uppercase text-white/40 hover:text-white/70 cursor-pointer"
        >
          <span className="flex items-center gap-1">
            {showRaw ? <ChevronDown className="w-3 h-3" /> : <ChevronRight className="w-3 h-3" />}
            Raw settings{rawRows.length > 0 && ` (${rawRows.length})`}
          </span>
        </button>
        {showRaw && (
          <>
            <div className="flex justify-end">
              <button
                type="button"
                onClick={() => setRawRows((prev) => [...prev, { key: "", value: "" }])}
                className="text-[11px] font-mono text-[#22d3ee] flex items-center gap-1 cursor-pointer"
              >
                <Plus className="w-3 h-3" /> Add Raw Setting
              </button>
            </div>
            {rawRows.map((row, idx) => (
              <div key={idx} className="flex items-center gap-1.5">
                <input
                  placeholder="key"
                  value={row.key}
                  onChange={(e) =>
                    setRawRows((prev) =>
                      prev.map((r, i) => (i === idx ? { ...r, key: e.target.value } : r))
                    )
                  }
                  className="flex-1 px-2 py-1 rounded glass-input text-[11px] font-mono"
                />
                <input
                  placeholder="value"
                  value={row.value}
                  onChange={(e) =>
                    setRawRows((prev) =>
                      prev.map((r, i) => (i === idx ? { ...r, value: e.target.value } : r))
                    )
                  }
                  className="flex-1 px-2 py-1 rounded glass-input text-[11px] font-mono"
                />
                <button
                  type="button"
                  onClick={() => setRawRows((prev) => prev.filter((_, i) => i !== idx))}
                  className="text-white/30 hover:text-red-400 p-1 cursor-pointer"
                >
                  <Trash2 className="w-3 h-3" />
                </button>
              </div>
            ))}
          </>
        )}
      </div>

      <div className="flex justify-end">
        <button
          type="button"
          onClick={handleSave}
          className="px-3.5 py-1.5 rounded-lg bg-[#06b6d4]/20 hover:bg-[#06b6d4]/30 text-[#22d3ee] hover:text-white border border-[#06b6d4]/40 font-mono text-xs font-semibold flex items-center gap-1.5 transition-all cursor-pointer"
        >
          <Save className="w-3.5 h-3.5" />
          <span>Save</span>
        </button>
      </div>
    </div>
  );
};
