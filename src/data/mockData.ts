import { TweakItem, TweakPack, BenchmarkProject, ProjectResults } from "../types";

export const VERIFIED_TWEAKS: TweakItem[] = [
  {
    id: "hags_enabled",
    name: "Hardware-Accelerated GPU Scheduling (Enabled)",
    category: "gpu",
    type: "registry",
    requiresReboot: true,
    description: "Offloads high-frequency video memory and command scheduling from CPU to GPU scheduler processor.",
    msdnUrl: "https://learn.microsoft.com/en-us/windows-hardware/drivers/display/hardware-accelerated-gpu-scheduling",
    keyPath: "HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers",
    valueName: "HwSchMode",
    valueType: "REG_DWORD",
    valueData: 2,
    details: "Reduces CPU draw-call bottlenecks. Direct impact on 1% low frame pacing in dense smoke/utility rounds."
  },
  {
    id: "hags_disabled",
    name: "Hardware-Accelerated GPU Scheduling (Disabled)",
    category: "gpu",
    type: "registry",
    requiresReboot: true,
    description: "Forces legacy WDDM CPU-driven command scheduling.",
    msdnUrl: "https://learn.microsoft.com/en-us/windows-hardware/drivers/display/hardware-accelerated-gpu-scheduling",
    keyPath: "HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers",
    valueName: "HwSchMode",
    valueType: "REG_DWORD",
    valueData: 1,
    details: "Useful comparison baseline on select older GPU drivers where HAGS introduces micro-stutters."
  },
  {
    id: "nvidia_driver_537_58",
    name: "NVIDIA 537.58 NVCleanstall Unattended Package",
    category: "driver_gpu",
    type: "driver_install",
    requiresReboot: true,
    description: "Installs the legendary 537.58 'Golden Branch' driver stripped of telemetry, GeForce Experience, and bloat.",
    driverPackagePath: "data/drivers/nv_537.58_unattended.exe",
    cleanInstall: true,
    dduDeepClean: false,
    details: "Executes unattended express installation with -clean flag. Triggers 3x shader compilation warmup runs post-boot."
  },
  {
    id: "nvidia_driver_560_94_ddu",
    name: "NVIDIA 560.94 Driver + DDU Safe-Mode Deep Clean",
    category: "driver_gpu",
    type: "driver_install",
    requiresReboot: true,
    description: "Executes automated Safe-Mode DDU uninstallation, reboots, and installs clean 560.94 package.",
    driverPackagePath: "data/drivers/nv_560.94_unattended.exe",
    cleanInstall: true,
    dduDeepClean: true,
    details: "Automated bcdedit safeboot loop. Strips all legacy registry artifacts and shader caches before installing."
  },
  {
    id: "npi_latency_extreme",
    name: "NPI Profile: CS2 Latency Extreme (.nip)",
    category: "npi_profile",
    type: "npi_profile",
    requiresReboot: false,
    description: "Injects ultra-low-latency nvidiaProfileInspector profile: Ultra Low Latency Mode, Max Pre-rendered Frames 1.",
    nipProfilePath: "data/profiles/cs2_latency_extreme.nip",
    details: "Silently imported via nvidiaProfileInspector.exe -silent. Original driver profile snapshot created for rollback."
  },
  {
    id: "npi_gsync_cap",
    name: "NPI Profile: G-Sync Reflex Framerate Limiter (.nip)",
    category: "npi_profile",
    type: "npi_profile",
    requiresReboot: false,
    description: "Caps framerate 3 FPS below display refresh (e.g. 237 FPS @ 240Hz / 357 FPS @ 360Hz) with Reflex enabled.",
    nipProfilePath: "data/profiles/cs2_gsync_reflex_cap.nip",
    details: "Guarantees zero G-Sync ceiling tearing and eliminates V-Sync frame queue latency buffer."
  },
  {
    id: "autogpuaffinity_core2",
    name: "AutoGpuAffinity — GPU Interrupt Pinning (Core 2)",
    category: "gpu",
    type: "affinity_gpu",
    requiresReboot: true,
    description: "Pins GPU DPC/ISR driver interrupts to Core 2, keeping Core 0 free for OS and CS2 render threads.",
    msdnUrl: "https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/interrupt-affinity-and-priority",
    keyPath: "HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Enum\\PCI\\<GPU_ID>\\Device Parameters\\Interrupt Management\\Affinity Policy",
    valueName: "DevicePolicy",
    valueType: "REG_DWORD",
    valueData: 4,
    affinityMask: "0x00000004",
    details: "Based on valleyofdoom/AutoGpuAffinity. Eliminates IRQ contention between Windows services and CS2."
  },
  {
    id: "gpu_msi_mode",
    name: "GPU Message Signaled-Based Interrupts (MSI Mode)",
    category: "gpu",
    type: "registry",
    requiresReboot: true,
    description: "Switches GPU from legacy line-based IRQ sharing to packet-based MSI-X memory writes.",
    msdnUrl: "https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/enabling-message-signaled-interrupts-in-the-registry",
    keyPath: "HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Enum\\PCI\\<GPU_ID>\\Device Parameters\\Interrupt Management\\MessageSignaledInterruptProperties",
    valueName: "MSISupported",
    valueType: "REG_DWORD",
    valueData: 1,
    details: "Significantly lowers DPC latency spikes during heavy GPU resource allocation in CS2."
  },
  {
    id: "core_parking_disabled",
    name: "Disable CPU Core Parking (CPMinCores 100%)",
    category: "cpu",
    type: "powercfg",
    requiresReboot: false,
    description: "Forces 100% of logical CPU cores to stay awake, preventing core wake-up latency stutters.",
    msdnUrl: "https://learn.microsoft.com/en-us/windows-server/administration/performance-tuning/hardware/power/processor-power-management-tuning",
    powerGuid: "powercfg /setacvalueindex scheme_current sub_processor CPMINCORES 100",
    details: "Eliminates latency penalty when sleeping cores are rapidly assigned particle compute tasks."
  },
  {
    id: "cpu_idle_disable",
    name: "CPU Idle State Disable (C-States Off)",
    category: "cpu",
    type: "powercfg",
    requiresReboot: false,
    description: "Keeps CPU cores locked in C0 maximum execution state, bypassing power transition delays.",
    msdnUrl: "https://learn.microsoft.com/en-us/windows-server/administration/performance-tuning/hardware/power/processor-power-management-tuning#processor-idle-disable",
    powerGuid: "powercfg /setacvalueindex scheme_current sub_processor IDLEDISABLE 1",
    details: "Provides ultra-stable frame pacing at the cost of higher idle power draw and temperatures."
  },
  {
    id: "intel_pcore_affinity",
    name: "Intel Hybrid P-Core Pinning (Cores 0-15)",
    category: "cpu",
    type: "affinity_cpu",
    requiresReboot: false,
    description: "Pins cs2.exe strictly to Performance Cores (P-Cores), preventing thread migration to E-Cores.",
    msdnUrl: "https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setprocessaffinitymask",
    affinityMask: "0x0000FFFF",
    details: "Critical for Intel 12th-15th Gen CPUs to avoid CS2 worker threads landing on slower E-Cores."
  },
  {
    id: "amd_ccd0_vcache_affinity",
    name: "AMD Ryzen 3D V-Cache CCD0 Pinning",
    category: "cpu",
    type: "affinity_cpu",
    requiresReboot: false,
    description: "Pins cs2.exe strictly to CCD0 (3D V-Cache dies on 7900X3D/7950X3D/9950X3D).",
    msdnUrl: "https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setprocessaffinitymask",
    affinityMask: "0x000000FF",
    details: "Prevents high-latency cross-die Infinity Fabric traversals to the non-V-Cache frequency CCD."
  },
  {
    id: "mmcss_system_responsiveness",
    name: "MMCSS System Responsiveness = 0",
    category: "kernel",
    type: "registry",
    requiresReboot: true,
    description: "Allocates 100% of CPU cycles to foreground multimedia without reserving 20% for background OS tasks.",
    msdnUrl: "https://learn.microsoft.com/en-us/windows/win32/procthread/multimedia-class-scheduler-service",
    keyPath: "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Multimedia\\SystemProfile",
    valueName: "SystemResponsiveness",
    valueType: "REG_DWORD",
    valueData: 0,
    details: "Ensures CS2 game engine thread priority is never throttled by Windows background maintenance."
  },
  {
    id: "global_timer_resolution",
    name: "Global High-Resolution Timer Requests",
    category: "kernel",
    type: "registry",
    requiresReboot: true,
    description: "Enables system-wide 0.5ms timer period requests on Windows 11 2004+.",
    msdnUrl: "https://learn.microsoft.com/en-us/windows/win32/api/timeapi/nf-timeapi-timebeginperiod",
    keyPath: "HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\Session Manager",
    valueName: "GlobalTimerResolutionRequests",
    valueType: "REG_DWORD",
    valueData: 1,
    details: "Enables microsecond timer accuracy for input polling, audio buffers, and frame synchronization."
  },
  {
    id: "fso_disabled",
    name: "Disable Fullscreen Optimizations (FSO)",
    category: "kernel",
    type: "registry",
    requiresReboot: false,
    description: "Forces classic Exclusive Fullscreen behavior, disabling D3D11/D3D12 Independent Flip.",
    msdnUrl: "https://devblogs.microsoft.com/directx/demystifying-full-screen-optimizations/",
    keyPath: "HKEY_CURRENT_USER\\System\\GameConfigStore",
    valueName: "GameDVR_FSEBehavior",
    valueType: "REG_DWORD",
    valueData: 2,
    details: "Can resolve input lag inconsistencies and micro-stutters on specific multi-monitor setups."
  },
  {
    id: "cs2_video_esports_low",
    name: "CS2 Esports Competitive Video Preset",
    category: "cs2_video",
    type: "cs2_config",
    requiresReboot: false,
    description: "Applies high-visibility esports config: Shadows Low, MSAA 2x, FSR Disabled, Reflex Enabled+Boost.",
    videoSettings: {
      "setting.csm_quality": "0",
      "setting.msaa_samples": "2",
      "setting.r_low_latency": "2",
      "setting.mat_vsync": "0"
    },
    details: "Optimized for maximum 1% lows and minimum render queue latency in competitive play."
  },
  {
    id: "cs2_launch_high_threads",
    name: "CS2 Launch Options: -threads 9 -high -nojoy",
    category: "cs2_launch",
    type: "launch_args",
    requiresReboot: false,
    description: "Passes optimal CPU worker thread count, process priority, and disables joystick polling.",
    launchFlags: "-threads 9 -high -nojoy -softparticlesdefaultoff",
    details: "Prevents CS2 engine from spawning excess idle threads while disabling controller overhead."
  }
];

export const VERIFIED_TWEAKPACKS: TweakPack[] = [
  {
    schema_version: "1.0.0",
    id: "pack_esports_extreme",
    name: "Competitive Latency & Driver Extreme Pack",
    author: "cs2kitchen",
    description: "Clean NVIDIA 537.58 Driver, Latency Extreme NPI Profile, Core Parking Off, and 0.5ms Global Timer.",
    prerequisites: {
      os_build_min: 19041,
      gpu_vendor: "NVIDIA"
    },
    modules: [VERIFIED_TWEAKS[2], VERIFIED_TWEAKS[4], VERIFIED_TWEAKS[8], VERIFIED_TWEAKS[13]]
  },
  {
    schema_version: "1.0.0",
    id: "pack_dpc_interrupt_master",
    name: "DPC Latency & AutoGpuAffinity Master Pack",
    author: "valleyofdoom",
    description: "AutoGpuAffinity on Core 2, GPU MSI Mode Enabled, MMCSS System Responsiveness = 0.",
    prerequisites: {
      os_build_min: 19041,
      gpu_vendor: "ANY"
    },
    modules: [VERIFIED_TWEAKS[6], VERIFIED_TWEAKS[7], VERIFIED_TWEAKS[12]]
  }
];

export const MOCK_PROJECTS: BenchmarkProject[] = [
  {
    id: "proj_01",
    name: "Kernel Scheduling & HAGS Telemetry Matrix",
    description: "Investigating the impact of Hardware-Accelerated GPU Scheduling vs. GPU Interrupt Affinity on Dust2 1% lows.",
    createdAt: "2026-08-30T14:20:00Z",
    lastRunAt: "2026-08-31T18:45:00Z",
    status: "completed",
    settings: {
      warmupLoops: 2,
      measureLoops: 3,
      benchmarkType: "workshop_dust2",
      targetMapId: "3240880604",
      captureDurationSeconds: 60,
      watchdogTimeoutSeconds: 90,
      netconPort: 2121,
      autoShutdownOnComplete: false,
      enableThermalGuard: true,
      maxIdleTempCelsius: 52,
      cooldownTimeoutSeconds: 120
    },
    baseline: {
      name: "Clean Windows Stock Baseline",
      description: "Default Windows 11 settings, HAGS Driver Default, Balanced Power Scheme."
    },
    scenarios: [
      {
        id: "scen_01",
        name: "HAGS Enabled + Core Parking 100%",
        description: "Hardware GPU scheduling enabled with all CPU cores unparked.",
        enabled: true,
        rebootRequired: true,
        tweaks: [VERIFIED_TWEAKS[0], VERIFIED_TWEAKS[8]]
      },
      {
        id: "scen_02",
        name: "AutoGpuAffinity (Core 2) + MSI Mode + HAGS",
        description: "Complete GPU DPC offloading combined with hardware scheduling and packet interrupts.",
        enabled: true,
        rebootRequired: true,
        tweaks: [VERIFIED_TWEAKS[0], VERIFIED_TWEAKS[6], VERIFIED_TWEAKS[7]]
      },
      {
        id: "scen_03",
        name: "NVIDIA 537.58 Driver + NPI Latency Extreme",
        description: "Clean BYOB driver installation stripped of bloat with extreme low latency profile.",
        enabled: true,
        rebootRequired: true,
        requiresShaderWarmup: true,
        tweaks: [VERIFIED_TWEAKS[2], VERIFIED_TWEAKS[4]]
      }
    ],
    resultsSummary: {
      totalRuns: 12,
      bestScenarioName: "AutoGpuAffinity (Core 2) + MSI Mode + HAGS",
      maxP1GainPercent: 14.8,
      bestWcpsScore: 325.4
    }
  },
  {
    id: "proj_02",
    name: "BYOB Driver Branch: 537.58 vs. 560.94 DDU Matrix",
    description: "Determining if older 537.58 driver yields better 1% low frame pacing than the latest 560.94 branch.",
    createdAt: "2026-08-31T10:15:00Z",
    status: "idle",
    settings: {
      warmupLoops: 3,
      measureLoops: 5,
      benchmarkType: "workshop_dust2",
      targetMapId: "3240880604",
      captureDurationSeconds: 60,
      watchdogTimeoutSeconds: 90,
      netconPort: 2121,
      autoShutdownOnComplete: false,
      enableThermalGuard: true,
      maxIdleTempCelsius: 50,
      cooldownTimeoutSeconds: 180
    },
    baseline: {
      name: "Current Installed NVIDIA Driver",
      description: "Baseline driver without DDU safe boot wipe."
    },
    scenarios: [
      {
        id: "scen_11",
        name: "NVIDIA 537.58 Clean Install",
        description: "Unattended NVCleanstall package with 3x shader cache warmup.",
        enabled: true,
        rebootRequired: true,
        requiresShaderWarmup: true,
        tweaks: [VERIFIED_TWEAKS[2], VERIFIED_TWEAKS[4]]
      },
      {
        id: "scen_12",
        name: "NVIDIA 560.94 + DDU Safe-Mode Wipe",
        description: "Full DDU safe-mode purge loop before 560.94 express installation.",
        enabled: true,
        rebootRequired: true,
        requiresShaderWarmup: true,
        dduDeepCleanEnabled: true,
        tweaks: [VERIFIED_TWEAKS[3], VERIFIED_TWEAKS[4]]
      }
    ]
  }
];

// Generate realistic 60-second frame-time curve data for visualization
function generateFrameTimeline(baseMs: number, jitter: number, spikeCount: number) {
  const points = [];
  const totalFrames = 300;
  for (let i = 0; i < totalFrames; i++) {
    const timeSec = (i / totalFrames) * 60;
    let frameTime = baseMs + (Math.sin(i * 0.15) * (jitter * 0.5)) + ((Math.random() - 0.5) * jitter);
    if (Math.abs(timeSec - 15) < 0.8 || Math.abs(timeSec - 32) < 0.8 || Math.abs(timeSec - 48) < 0.8) {
      frameTime += Math.random() * (spikeCount * 4.5);
    }
    const gpuBusy = frameTime * (0.86 + (Math.random() * 0.08));
    points.push({
      timeMs: Math.round(timeSec * 10) / 10,
      frameTimeMs: Math.round(frameTime * 100) / 100,
      gpuBusyMs: Math.round(gpuBusy * 100) / 100
    });
  }
  return points;
}

export const MOCK_PROJECT_RESULTS: ProjectResults = {
  projectId: "proj_01",
  projectName: "Kernel Scheduling & HAGS Telemetry Matrix",
  completedAt: "2026-08-31T18:45:00Z",
  baselineResult: {
    scenarioId: "baseline",
    scenarioName: "Clean Windows Stock Baseline",
    isBaseline: true,
    aggregated: {
      avgFps: 412.4,
      medianFps: 418.1,
      p1Fps: 264.2,
      p01Fps: 182.5,
      gpuBusyMs: 2.12,
      frameTimeMs: 2.42,
      frameJitterMs: 0.48,
      bottleneckRatio: 0.87,
      wcpsScore: 266.3, // (412.4*0.2) + (264.2*0.4) + (182.5*0.2) + ((100/0.48)*0.2)
      avgCpuTemp: 58.4,
      avgGpuTemp: 51.2
    },
    runs: [
      {
        runIndex: 1,
        isWarmup: false,
        avgFps: 410.8,
        medianFps: 416.5,
        p1Fps: 261.4,
        p01Fps: 179.8,
        gpuBusyMs: 2.14,
        frameTimeMs: 2.43,
        frameJitterMs: 0.51,
        bottleneckRatio: 0.88,
        wcpsScore: 263.2,
        cpuTempCelsius: 58.6,
        gpuTempCelsius: 51.4,
        dataPoints: generateFrameTimeline(2.43, 0.51, 3)
      },
      {
        runIndex: 2,
        isWarmup: false,
        avgFps: 413.2,
        medianFps: 419.0,
        p1Fps: 265.8,
        p01Fps: 184.2,
        gpuBusyMs: 2.11,
        frameTimeMs: 2.42,
        frameJitterMs: 0.46,
        bottleneckRatio: 0.87,
        wcpsScore: 268.4,
        cpuTempCelsius: 58.2,
        gpuTempCelsius: 51.0,
        dataPoints: generateFrameTimeline(2.42, 0.46, 2)
      }
    ]
  },
  scenarioResults: [
    {
      scenarioId: "scen_01",
      scenarioName: "HAGS Enabled + Core Parking 100%",
      isBaseline: false,
      aggregated: {
        avgFps: 428.6,
        medianFps: 432.4,
        p1Fps: 288.4,
        p01Fps: 204.1,
        gpuBusyMs: 2.18,
        frameTimeMs: 2.33,
        frameJitterMs: 0.38,
        bottleneckRatio: 0.93,
        wcpsScore: 294.5,
        avgCpuTemp: 59.1,
        avgGpuTemp: 52.3
      },
      deltaVsBaseline: {
        avgFpsPercent: 3.9,
        p1FpsPercent: 9.2,
        p01FpsPercent: 11.8,
        jitterPercent: -20.8,
        gpuBusyDeltaMs: 0.06,
        wcpsDeltaPercent: 10.6
      },
      runs: []
    },
    {
      scenarioId: "scen_02",
      scenarioName: "AutoGpuAffinity (Core 2) + MSI Mode + HAGS",
      isBaseline: false,
      aggregated: {
        avgFps: 436.2,
        medianFps: 441.0,
        p1Fps: 303.4,
        p01Fps: 226.8,
        gpuBusyMs: 2.21,
        frameTimeMs: 2.29,
        frameJitterMs: 0.28,
        bottleneckRatio: 0.96,
        wcpsScore: 325.4,
        avgCpuTemp: 57.8,
        avgGpuTemp: 50.8
      },
      deltaVsBaseline: {
        avgFpsPercent: 5.8,
        p1FpsPercent: 14.8,
        p01FpsPercent: 24.3,
        jitterPercent: -41.6,
        gpuBusyDeltaMs: 0.09,
        wcpsDeltaPercent: 22.2
      },
      runs: [
        {
          runIndex: 1,
          isWarmup: false,
          avgFps: 436.2,
          medianFps: 441.0,
          p1Fps: 303.4,
          p01Fps: 226.8,
          gpuBusyMs: 2.21,
          frameTimeMs: 2.29,
          frameJitterMs: 0.28,
          bottleneckRatio: 0.96,
          wcpsScore: 325.4,
          cpuTempCelsius: 57.8,
          gpuTempCelsius: 50.8,
          dataPoints: generateFrameTimeline(2.29, 0.28, 0)
        }
      ]
    },
    {
      scenarioId: "scen_03",
      scenarioName: "NVIDIA 537.58 Driver + NPI Latency Extreme",
      isBaseline: false,
      aggregated: {
        avgFps: 432.8,
        medianFps: 437.5,
        p1Fps: 298.2,
        p01Fps: 221.0,
        gpuBusyMs: 2.20,
        frameTimeMs: 2.31,
        frameJitterMs: 0.31,
        bottleneckRatio: 0.95,
        wcpsScore: 314.6,
        avgCpuTemp: 58.0,
        avgGpuTemp: 51.5
      },
      deltaVsBaseline: {
        avgFpsPercent: 4.9,
        p1FpsPercent: 12.9,
        p01FpsPercent: 21.1,
        jitterPercent: -35.4,
        gpuBusyDeltaMs: 0.08,
        wcpsDeltaPercent: 18.1
      },
      runs: []
    }
  ]
};
