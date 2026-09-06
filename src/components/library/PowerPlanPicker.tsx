import React, { useEffect, useState } from "react";
import { Check as CheckIcon, Zap } from "lucide-react";
import { listPowerPlans } from "../../lib/api";
import type { PowerPlan } from "../../lib/bindings";

interface PowerPlanPickerProps {
  onSelect: (plan: PowerPlan) => void;
  /** The module's currently configured plan_guid, when editing an existing
   * module -- highlighted as the current selection. Only the *live active*
   * plan is disabled; this one stays pickable (re-selecting it is a no-op). */
  currentPlanGuid?: string;
}

export const PowerPlanPicker: React.FC<PowerPlanPickerProps> = ({ onSelect, currentPlanGuid }) => {
  const [plans, setPlans] = useState<PowerPlan[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setIsLoading(true);
    setError(null);
    listPowerPlans()
      .then(setPlans)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setIsLoading(false));
  }, []);

  return (
    <div className="space-y-2">
      {isLoading && <div className="text-xs font-mono text-white/50">Loading power plans…</div>}
      {error && <div className="text-xs font-mono text-red-400">{error}</div>}
      {plans.map((plan) => {
        const isCurrent = plan.guid === currentPlanGuid;
        return (
          <button
            key={plan.guid}
            type="button"
            disabled={plan.active}
            onClick={() => onSelect(plan)}
            className={`w-full flex items-center justify-between gap-3 px-3.5 py-2.5 rounded-lg border text-left font-mono text-xs transition-colors cursor-pointer disabled:cursor-not-allowed ${
              plan.active
                ? "bg-white/5 border-white/10 text-white/30"
                : isCurrent
                  ? "bg-[#06b6d4]/15 border-[#06b6d4]/40 text-white"
                  : "bg-black/40 border-white/10 text-white/80 hover:border-[#06b6d4]/40 hover:text-white"
            }`}
          >
            <span className="flex items-center gap-2">
              {isCurrent && !plan.active && <CheckIcon className="w-3.5 h-3.5 text-[#22d3ee]" />}
              {plan.name}
            </span>
            {plan.active && (
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
