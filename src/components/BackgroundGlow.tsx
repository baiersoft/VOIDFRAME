import React, { useEffect, useState } from "react";

export const BackgroundGlow: React.FC = () => {
  const [mousePos, setMousePos] = useState({ x: 50, y: 50 });

  useEffect(() => {
    const handleMouseMove = (e: MouseEvent) => {
      const x = (e.clientX / window.innerWidth) * 100;
      const y = (e.clientY / window.innerHeight) * 100;
      setMousePos({ x, y });
      document.documentElement.style.setProperty("--mouse-x", `${x}%`);
      document.documentElement.style.setProperty("--mouse-y", `${y}%`);
    };

    window.addEventListener("mousemove", handleMouseMove, { passive: true });
    return () => window.removeEventListener("mousemove", handleMouseMove);
  }, []);

  return (
    <div className="fixed inset-0 pointer-events-none z-0 overflow-hidden">
      {/* Dynamic Mouse-Following Radial Glows */}
      <div
        className="absolute w-[650px] h-[650px] rounded-full blur-[140px] opacity-25 transition-transform duration-700 ease-out"
        style={{
          background: "radial-gradient(circle, #06b6d4 0%, rgba(6, 182, 212, 0) 70%)",
          left: `calc(${mousePos.x}% - 325px)`,
          top: `calc(${mousePos.y}% - 325px)`,
        }}
      />
      <div
        className="absolute w-[500px] h-[500px] rounded-full blur-[160px] opacity-20 transition-transform duration-1000 ease-out"
        style={{
          background: "radial-gradient(circle, #8b5cf6 0%, rgba(139, 92, 246, 0) 70%)",
          left: `calc(${100 - mousePos.x}% - 250px)`,
          top: `calc(${100 - mousePos.y}% - 250px)`,
        }}
      />

      {/* Subtle Static Void Vignette */}
      <div className="absolute inset-0 bg-gradient-to-b from-[#010103]/60 via-transparent to-[#010103]/90" />

      {/* Subtle Matrix Grid */}
      <div className="absolute inset-0 bg-grid-pattern opacity-60" />
    </div>
  );
};
