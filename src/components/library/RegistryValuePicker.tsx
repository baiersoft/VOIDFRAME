import React from "react";
import { Zap } from "lucide-react";
import type { CatalogValueChoice } from "../../lib/bindings";

interface RegistryValuePickerProps {
  choices: CatalogValueChoice[];
  /** The machine's live registry value for this entry, when known -- the
   * choice matching it is disabled, same as PowerPlanPicker disables the
   * live-active plan. `undefined` (read failed, or hasn't loaded yet)
   * disables nothing. */
  currentValue: unknown;
  onSelect: (choice: CatalogValueChoice) => void;
}

export const RegistryValuePicker: React.FC<RegistryValuePickerProps> = ({
  choices,
  currentValue,
  onSelect,
}) => {
  return (
    <div className="space-y-2">
      {choices.map((choice) => {
        const isActive = currentValue !== undefined && choice.value === currentValue;
        return (
          <button
            key={choice.label}
            type="button"
            disabled={isActive}
            onClick={() => onSelect(choice)}
            className={`w-full flex items-center justify-between gap-3 px-3.5 py-2.5 rounded-lg border text-left font-mono text-xs transition-colors cursor-pointer disabled:cursor-not-allowed ${
              isActive
                ? "bg-white/5 border-white/10 text-white/30"
                : "bg-black/40 border-white/10 text-white/80 hover:border-[#06b6d4]/40 hover:text-white"
            }`}
          >
            <span>{choice.label}</span>
            {isActive && (
              <span className="flex items-center gap-1 text-[10px] uppercase text-white/40">
                <Zap className="w-3 h-3" /> Currently active
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
};
