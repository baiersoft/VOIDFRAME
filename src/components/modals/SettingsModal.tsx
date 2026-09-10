import React, { useState, useEffect } from "react";
import { X, Settings, Save, Check, AlertTriangle, Cpu } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getConfig, saveConfig, getCpuModel, getGpuModel } from "../../lib/api";
import type { Config } from "../../lib/bindings";

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({ isOpen, onClose }) => {
  const [config, setConfig] = useState<Config | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  // Read once per modal open, not re-fetched while the modal stays open --
  // these are static facts about the machine, not something that changes
  // mid-session.
  const [cpuModel, setCpuModel] = useState<string | null>(null);
  const [gpuModel, setGpuModel] = useState<string | null>(null);
  const [cpuError, setCpuError] = useState<string | null>(null);
  const [gpuError, setGpuError] = useState<string | null>(null);

  useEffect(() => {
    if (!isOpen) return;
    setIsLoading(true);
    setError(null);
    getConfig()
      .then(setConfig)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setIsLoading(false));
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen) return;
    setCpuError(null);
    getCpuModel()
      .then(setCpuModel)
      .catch((e) => setCpuError(e instanceof Error ? e.message : String(e)));
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen) return;
    setGpuError(null);
    getGpuModel()
      .then(setGpuModel)
      .catch((e) => setGpuError(e instanceof Error ? e.message : String(e)));
  }, [isOpen]);

  if (!isOpen) return null;

  const handleSave = async () => {
    if (!config) return;
    setError(null);
    try {
      await saveConfig(config);
      setSaved(true);
      setTimeout(() => {
        setSaved(false);
        onClose();
      }, 800);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/80 backdrop-blur-xl">
      <div className="glass-panel w-full max-w-2xl rounded-2xl border border-white/10 flex flex-col shadow-[0_0_50px_rgba(0,0,0,0.8)] overflow-hidden">
        <div className="p-6 border-b border-white/10 flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-xl bg-[#06b6d4]/10 border border-[#06b6d4]/30 text-[#06b6d4]">
              <Settings className="w-5 h-5" />
            </div>
            <div>
              <h2 className="font-sans font-bold text-lg text-white">Engine Preferences</h2>
              <p className="text-xs text-white/50 font-mono">System Info & Default Run Parameters</p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="p-2 rounded-lg text-white/50 hover:text-white hover:bg-white/5 transition-colors cursor-pointer"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        <div className="p-6 space-y-4 overflow-y-auto max-h-[60vh]">
          <div className="space-y-1.5">
            <h3 className="text-xs font-mono text-white/70 flex items-center gap-1.5">
              <Cpu className="w-3.5 h-3.5 text-[#06b6d4]" /> System Information
            </h3>
            <div className="p-3.5 rounded-xl bg-black/40 border border-white/5 space-y-1.5">
              {cpuError ? (
                <div className="text-xs font-mono text-red-300">{cpuError}</div>
              ) : (
                <div className="text-xs font-mono text-white/80">
                  CPU: <span className="text-white">{cpuModel ?? "Detecting…"}</span>
                </div>
              )}
              {gpuError ? (
                <div className="text-xs font-mono text-red-300">{gpuError}</div>
              ) : (
                <div className="text-xs font-mono text-white/80">
                  GPU: <span className="text-white">{gpuModel ?? "Detecting…"}</span>
                </div>
              )}
            </div>
          </div>

          {isLoading && <div className="text-xs font-mono text-white/50">Loading configuration…</div>}
          {error && (
            <div className="p-3 rounded-xl bg-red-500/10 border border-red-500/30 text-red-300 text-xs font-mono flex items-center gap-2">
              <AlertTriangle className="w-4 h-4 shrink-0" />
              <span>{error}</span>
            </div>
          )}
          {config && (
            <>
              <label className="flex items-center justify-between p-3.5 rounded-xl bg-black/40 border border-white/5 cursor-pointer">
                <div>
                  <div className="text-xs font-mono text-white/80">Thermal Cooldown Between Iterations</div>
                  <div className="text-[10px] font-mono text-white/40 mt-0.5">
                    Uses HWiNFO to wait for temperatures to settle before each iteration.
                  </div>
                </div>
                <input
                  type="checkbox"
                  checked={config.thermal_cooldown_enabled ?? false}
                  onChange={(e) => setConfig({ ...config, thermal_cooldown_enabled: e.target.checked })}
                  className="rounded bg-black/50 border-white/20 text-[#06b6d4] focus:ring-0 w-4 h-4 cursor-pointer"
                />
              </label>

              <label className="flex items-center justify-between p-3.5 rounded-xl bg-black/40 border border-white/5 cursor-pointer">
                <div>
                  <div className="text-xs font-mono text-white/80">Dry Run by Default</div>
                  <div className="text-[10px] font-mono text-white/40 mt-0.5">
                    Applies every tweak in dry-run mode -- registry, power plan, and affinity
                    writes are logged but never actually applied. Still launches CS2 and runs a
                    real, full-length capture.
                  </div>
                </div>
                <input
                  type="checkbox"
                  checked={config.dry_run_default ?? false}
                  onChange={(e) => setConfig({ ...config, dry_run_default: e.target.checked })}
                  className="rounded bg-black/50 border-white/20 text-[#06b6d4] focus:ring-0 w-4 h-4 cursor-pointer"
                />
              </label>

              <label className="flex items-center justify-between p-3.5 rounded-xl bg-black/40 border border-white/5 cursor-pointer">
                <div>
                  <div className="text-xs font-mono text-white/80">Shut down when a run completes (default)</div>
                  <div className="text-[10px] font-mono text-white/40 mt-0.5">
                    Default state of the countdown modal's shutdown toggle -- off by default, an
                    unattended shutdown is opt-in.
                  </div>
                </div>
                <input
                  type="checkbox"
                  checked={config.shutdown_when_complete_default ?? false}
                  onChange={(e) =>
                    setConfig({ ...config, shutdown_when_complete_default: e.target.checked })
                  }
                  className="rounded bg-black/50 border-white/20 text-[#06b6d4] focus:ring-0 w-4 h-4 cursor-pointer"
                />
              </label>

              <div className="space-y-1.5">
                <label htmlFor="post-boot-settle-seconds" className="text-xs font-mono text-white/70">
                  Post-boot Settle (seconds)
                </label>
                <input
                  id="post-boot-settle-seconds"
                  type="number"
                  value={config.post_boot_settle_seconds ?? 180}
                  onChange={(e) =>
                    setConfig({ ...config, post_boot_settle_seconds: Number(e.target.value) })
                  }
                  className="w-full px-3.5 py-2 rounded-xl glass-input text-xs font-mono"
                />
              </div>

              <div className="pt-2 border-t border-white/10 space-y-2">
                <p className="text-[10px] font-mono text-white/40">
                  VOIDFRAME bundles the following third-party tools for hardware telemetry and frame-time capture:
                </p>
                <div className="flex flex-col gap-1">
                  <button
                    type="button"
                    onClick={() => openUrl("https://www.hwinfo.com/")}
                    className="text-left text-xs font-mono text-white/60 hover:text-[#06b6d4] transition-colors cursor-pointer w-fit"
                  >
                    HWiNFO — hardware monitoring &amp; sensors, by REALiX
                  </button>
                  <button
                    type="button"
                    onClick={() => openUrl("https://www.presentmon.com/")}
                    className="text-left text-xs font-mono text-white/60 hover:text-[#06b6d4] transition-colors cursor-pointer w-fit"
                  >
                    PresentMon — frame-time capture, by Intel (GameTechDev)
                  </button>
                </div>
              </div>
            </>
          )}
        </div>

        <div className="p-6 border-t border-white/10 bg-black/30 flex items-center justify-between">
          <button
            onClick={onClose}
            className="glass-pill px-4 py-2 rounded-xl text-xs font-mono text-white/60 hover:text-white cursor-pointer"
          >
            Cancel
          </button>
          <button
            onClick={handleSave}
            disabled={!config}
            className="px-5 py-2 rounded-xl bg-gradient-to-r from-[#06b6d4] to-[#8b5cf6] text-white font-mono text-xs font-bold flex items-center gap-1.5 cursor-pointer shadow-[0_0_20px_rgba(6,182,212,0.3)] disabled:opacity-40"
          >
            {saved ? <Check className="w-4 h-4" /> : <Save className="w-4 h-4" />}
            <span>{saved ? "Saved" : "Save Preferences"}</span>
          </button>
        </div>
      </div>
    </div>
  );
};
