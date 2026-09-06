import React, { useEffect, useState } from "react";
import { Search, Plus, X, Check, Layers, Filter } from "lucide-react";
import { listCatalogTweaks, readCs2LaunchOptions } from "../../lib/api";
import type { CatalogEntry, Module, PowerPlan } from "../../lib/bindings";
import { PowerPlanPicker } from "./PowerPlanPicker";
import { LaunchArgsEditor } from "./LaunchArgsEditor";

interface TweakCatalogProps {
  isOpen: boolean;
  onClose: () => void;
  onSelectTweak?: (module: Module) => void;
  onOpenTweakCreator?: () => void;
  targetScenarioName?: string;
  /** The target scenario's current modules -- used only to grey out a
   * singleton catalog entry (`entry.singleton`) whose kind is already
   * present there. */
  currentModules?: Module[];
  isModal?: boolean;
}

export const TweakCatalog: React.FC<TweakCatalogProps> = ({
  isOpen,
  onClose,
  onSelectTweak,
  targetScenarioName,
  currentModules = [],
  isModal = true,
}) => {
  const [searchTerm, setSearchTerm] = useState("");
  const [categoryFilter, setCategoryFilter] = useState<string>("all");
  const [entries, setEntries] = useState<CatalogEntry[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [addedIds, setAddedIds] = useState<Record<string, boolean>>({});
  const [pickingPowerPlanFor, setPickingPowerPlanFor] = useState<string | null>(null);
  const [pickingLaunchArgsFor, setPickingLaunchArgsFor] = useState<string | null>(null);
  const [launchArgsInitialValue, setLaunchArgsInitialValue] = useState<string | null>(null);

  useEffect(() => {
    if (!isOpen) return;
    setIsLoading(true);
    setError(null);
    listCatalogTweaks()
      .then(setEntries)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setIsLoading(false));
  }, [isOpen]);

  if (!isOpen) return null;

  const categories = ["all", ...Array.from(new Set(entries.map((e) => e.category)))];

  const filtered = entries.filter((e) => {
    const matchesSearch =
      e.name.toLowerCase().includes(searchTerm.toLowerCase()) ||
      e.description.toLowerCase().includes(searchTerm.toLowerCase()) ||
      e.kind.toLowerCase().includes(searchTerm.toLowerCase());
    const matchesCategory = categoryFilter === "all" || e.category === categoryFilter;
    return matchesSearch && matchesCategory;
  });

  const flashAdded = (entryId: string) => {
    setAddedIds((prev) => ({ ...prev, [entryId]: true }));
    setTimeout(() => setAddedIds((prev) => ({ ...prev, [entryId]: false })), 1500);
  };

  const handleAdd = (entry: CatalogEntry) => {
    if (!onSelectTweak) return;
    if (entry.kind === "power_plan") {
      setPickingPowerPlanFor(entry.id);
      return;
    }
    if (entry.kind === "launch_args") {
      setPickingLaunchArgsFor(entry.id);
      setLaunchArgsInitialValue(null);
      readCs2LaunchOptions()
        .then(setLaunchArgsInitialValue)
        .catch(() => setLaunchArgsInitialValue(""));
      return;
    }
    onSelectTweak(entry.module_template as Module);
    flashAdded(entry.id);
  };

  const handlePickPowerPlan = (entry: CatalogEntry, plan: PowerPlan) => {
    if (!onSelectTweak) return;
    onSelectTweak({
      type: "power_plan",
      plan_guid: plan.guid,
      friendly_name: plan.name,
      create_if_missing: false,
    });
    setPickingPowerPlanFor(null);
    flashAdded(entry.id);
  };

  const handleSaveLaunchArgs = (entry: CatalogEntry, args: string) => {
    if (!onSelectTweak) return;
    onSelectTweak({ type: "launch_args", args });
    setPickingLaunchArgsFor(null);
    setLaunchArgsInitialValue(null);
    flashAdded(entry.id);
  };

  const content = (
    <div className={`glass-panel w-full ${isModal ? "max-w-4xl max-h-[85vh]" : "max-w-6xl"} rounded-2xl border border-white/10 flex flex-col shadow-[0_0_50px_rgba(0,0,0,0.8)] overflow-hidden`}>
      {/* Header */}
      <div className="p-6 border-b border-white/10 flex items-center justify-between">
        <div className="space-y-1">
          <div className="flex items-center gap-2">
            <Layers className="w-4 h-4 text-[#06b6d4]" />
            <span className="font-mono text-xs text-white/50 uppercase tracking-wider">
              {isModal ? "Tweak Catalog" : "Verified Tweak Library"}
            </span>
          </div>
          <h2 className="font-sans font-bold text-xl text-white">
            {isModal
              ? `Add Module ${targetScenarioName ? `→ ${targetScenarioName}` : ""}`
              : "Explore Validated System Tweaks"}
          </h2>
        </div>

        {isModal && (
          <button
            onClick={onClose}
            className="p-2 rounded-lg text-white/50 hover:text-white hover:bg-white/5 transition-colors cursor-pointer"
          >
            <X className="w-5 h-5" />
          </button>
        )}
      </div>

      {/* Filter & Search Bar */}
      <div className="p-6 pb-4 border-b border-white/5 bg-black/20 space-y-3">
        <div className="flex flex-col sm:flex-row gap-3 items-center justify-between">
          <div className="relative w-full sm:w-80">
            <Search className="w-4 h-4 absolute left-3.5 top-1/2 -translate-y-1/2 text-white/40" />
            <input
              type="text"
              placeholder="Search modules by name, description, kind..."
              value={searchTerm}
              onChange={(e) => setSearchTerm(e.target.value)}
              className="w-full pl-10 pr-4 py-2.5 rounded-xl glass-input text-xs font-mono"
            />
          </div>

          <div className="flex items-center gap-1.5 overflow-x-auto w-full sm:w-auto pb-1 sm:pb-0">
            <Filter className="w-3.5 h-3.5 text-white/40 shrink-0 mr-1 hidden sm:inline" />
            {categories.map((cat) => (
              <button
                key={cat}
                onClick={() => setCategoryFilter(cat)}
                className={`px-3 py-1 rounded-lg font-mono text-[11px] uppercase transition-all cursor-pointer whitespace-nowrap ${
                  categoryFilter === cat
                    ? "bg-[#06b6d4]/20 text-[#22d3ee] border border-[#06b6d4]/40 font-bold"
                    : "text-white/40 hover:text-white hover:bg-white/5 border border-transparent"
                }`}
              >
                {cat}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Module Entries */}
      <div className="p-6 space-y-4 overflow-y-auto flex-1 max-h-[60vh]">
        {isLoading && <div className="text-xs font-mono text-white/50">Loading catalog…</div>}
        {error && <div className="text-xs font-mono text-red-400">{error}</div>}
        {!isLoading && !error && filtered.length === 0 && (
          <div className="text-center py-10 text-xs font-mono text-white/40">
            No modules match your search filter.
          </div>
        )}
        {filtered.map((entry) => {
          const isAdded = addedIds[entry.id];
          const alreadyPresent =
            entry.singleton && currentModules.some((m) => m.type === entry.kind);
          return (
            <div key={entry.id} className="glass-card p-5 rounded-xl border-white/10 space-y-3">
              <div className="flex items-start justify-between gap-4">
                <div className="space-y-1.5">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono uppercase bg-white/5 text-white/60 border border-white/10">
                      {entry.category}
                    </span>
                    <span className="px-2 py-0.5 rounded text-[9px] font-mono uppercase bg-[#06b6d4]/10 text-[#22d3ee] border border-[#06b6d4]/30">
                      {entry.kind}
                    </span>
                    <h4 className="font-sans font-bold text-base text-white">{entry.name}</h4>
                  </div>
                  <p className="text-xs text-white/70 font-body leading-relaxed max-w-2xl">
                    {entry.description}
                  </p>
                </div>
                {onSelectTweak && (
                  <button
                    onClick={() => handleAdd(entry)}
                    disabled={alreadyPresent}
                    title={alreadyPresent ? "Already added to this scenario" : undefined}
                    className={`px-4 py-2 rounded-lg font-mono text-xs font-semibold flex items-center gap-1.5 transition-all cursor-pointer whitespace-nowrap shrink-0 disabled:cursor-not-allowed disabled:opacity-40 ${
                      isAdded
                        ? "bg-emerald-500/20 text-emerald-300 border border-emerald-500/40"
                        : "bg-[#06b6d4]/20 hover:bg-[#06b6d4]/30 text-[#22d3ee] hover:text-white border border-[#06b6d4]/40"
                    }`}
                  >
                    {isAdded ? (
                      <>
                        <Check className="w-3.5 h-3.5" /> Added
                      </>
                    ) : (
                      <>
                        <Plus className="w-3.5 h-3.5" /> Add Module
                      </>
                    )}
                  </button>
                )}
              </div>
              {pickingPowerPlanFor === entry.id && (
                <div className="pt-2 border-t border-white/5">
                  <PowerPlanPicker onSelect={(plan) => handlePickPowerPlan(entry, plan)} />
                </div>
              )}
              {pickingLaunchArgsFor === entry.id && (
                <div className="pt-2 border-t border-white/5">
                  {launchArgsInitialValue === null ? (
                    <div className="text-xs font-mono text-white/50">
                      Loading current launch options…
                    </div>
                  ) : (
                    <LaunchArgsEditor
                      initialValue={launchArgsInitialValue}
                      onSave={(args) => handleSaveLaunchArgs(entry, args)}
                    />
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );

  if (!isModal) {
    return content;
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/80 backdrop-blur-xl">
      {content}
    </div>
  );
};
