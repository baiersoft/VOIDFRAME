import React, { useState, useEffect } from "react";
import { Minus, Square, Copy, X } from "lucide-react";
import {
  isWindowMaximized,
  minimizeWindow,
  toggleMaximizeWindow,
  closeWindow,
} from "../lib/api";

export const TitleBar: React.FC = () => {
  const [isMaximized, setIsMaximized] = useState(false);

  useEffect(() => {
    const checkState = async () => {
      try {
        const max = await isWindowMaximized();
        setIsMaximized(max);
      } catch {
        // Outside Tauri runtime (e.g. browser)
      }
    };

    checkState();

    const handleResize = () => {
      checkState();
    };

    window.addEventListener("resize", handleResize);
    return () => window.removeEventListener("resize", handleResize);
  }, []);

  const handleMinimize = async () => {
    try {
      await minimizeWindow();
    } catch (err) {
      console.warn("Minimize window error:", err);
    }
  };

  const handleToggleMaximize = async () => {
    try {
      const state = await toggleMaximizeWindow();
      setIsMaximized(state);
    } catch (err) {
      console.warn("Maximize window error:", err);
      setIsMaximized(!isMaximized);
    }
  };

  const handleClose = async () => {
    try {
      await closeWindow();
    } catch (err) {
      console.warn("Close window error:", err);
    }
  };

  return (
    <div
      data-tauri-drag-region
      className="relative z-30 h-9 bg-[#010103] border-b border-white/[0.08] px-3 flex items-center justify-between select-none"
    >
      {/* Left: App Icon & Brand Name */}
      <div data-tauri-drag-region className="flex items-center gap-2 pointer-events-none">
        <svg
          xmlns="http://www.w3.org/2000/svg"
          viewBox="0 0 1024 1024"
          className="w-4 h-4 shrink-0 drop-shadow-[0_0_8px_rgba(6,182,212,0.6)]"
        >
          <path
            d="M 230 290 L 512 790 L 794 290 L 670 290 L 512 590 L 354 290 Z"
            fill="#06b6d4"
          />
          <polygon points="512,390 580,480 512,570 444,480" fill="#8b5cf6" />
        </svg>

        <span className="font-mono text-[11px] font-bold tracking-[0.2em] text-white/90">
          baiersoft <span className="text-[#06b6d4]">//</span> VOIDFRAME
        </span>
        <span className="px-1.5 py-0.2 rounded text-[9px] font-mono bg-[#06b6d4]/10 text-[#22d3ee] border border-[#06b6d4]/30 ml-1">
          v0.1.0-RC
        </span>
      </div>

      {/* Center: Draggable Area */}
      <div
        data-tauri-drag-region
        className="flex-1 h-full flex items-center justify-center pointer-events-auto cursor-default"
      />

      {/* Right: Window Controls */}
      <div className="flex items-center gap-1 -mr-1">
        <button
          onClick={handleMinimize}
          title="Minimize"
          className="w-8 h-7 rounded flex items-center justify-center text-white/60 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer"
        >
          <Minus className="w-3.5 h-3.5" />
        </button>

        <button
          onClick={handleToggleMaximize}
          title={isMaximized ? "Restore" : "Maximize"}
          className="w-8 h-7 rounded flex items-center justify-center text-white/60 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer"
        >
          {isMaximized ? <Copy className="w-3 h-3 rotate-180" /> : <Square className="w-3 h-3" />}
        </button>

        <button
          onClick={handleClose}
          title="Close"
          className="w-8 h-7 rounded flex items-center justify-center text-white/60 hover:text-white hover:bg-red-500/80 transition-colors cursor-pointer"
        >
          <X className="w-3.5 h-3.5" />
        </button>
      </div>
    </div>
  );
};
