// Curated dropdown -> cs2_video.txt key/value mapping for the cs2_config
// builder form. Labels and dropdown ordering come from 16 screenshots of
// the live CS2 Video/Advanced Video settings screens (2026-09-08); every
// value except the four marked "inferred" below was directly observed
// changing in a real cs2_video.txt across 6 rounds of one-setting-at-a-time
// changes on the dev rig -- see
// docs/superpowers/specs/2026-09-08-custom-script-cs2-config-ui-design.md
// section 2 for the full verification record. Keys carry the full
// "setting." prefix, matching Cs2ConfigPayload.settings' own established
// convention (model/module.rs's own test uses "setting.fullscreen").

export interface Cs2VideoOption {
  label: string;
  /** One value per `Cs2VideoSetting.keys` entry, same index order. */
  values: string[];
}

export interface Cs2VideoSetting {
  label: string;
  keys: string[];
  options: Cs2VideoOption[];
}

export const CS2_VIDEO_SETTINGS: Cs2VideoSetting[] = [
  {
    label: "Display Mode",
    keys: ["setting.fullscreen", "setting.nowindowborder"],
    options: [
      { label: "Windowed", values: ["0", "0"] },
      { label: "Fullscreen", values: ["1", "0"] },
      { label: "Fullscreen Windowed", values: ["0", "1"] },
    ],
  },
  {
    label: "Aspect Ratio",
    keys: ["setting.aspectratiomode"],
    options: [
      { label: "Normal 4:3", values: ["0"] },
      { label: "Widescreen 16:9", values: ["1"] },
      { label: "Widescreen 16:10", values: ["2"] },
    ],
  },
  {
    label: "V-Sync",
    keys: ["setting.mat_vsync"],
    options: [
      { label: "Disabled", values: ["0"] },
      { label: "Enabled", values: ["1"] },
    ],
  },
  {
    label: "NVIDIA Reflex Low Latency",
    keys: ["setting.r_low_latency"],
    options: [
      { label: "Disabled", values: ["0"] },
      { label: "Enabled", values: ["1"] },
      { label: "Enabled + Boost", values: ["2"] },
    ],
  },
  {
    label: "Multisampling Anti-Aliasing Mode",
    keys: ["setting.msaa_samples", "setting.r_csgo_cmaa_enable"],
    options: [
      { label: "None", values: ["0", "0"] },
      { label: "CMAA2", values: ["0", "1"] },
      { label: "2x MSAA", values: ["2", "0"] },
      // Inferred: literal MSAA sample count, confirmed pattern from the
      // None (0) and 2x (2) points -- never itself toggled.
      { label: "4x MSAA", values: ["4", "0"] },
      { label: "8x MSAA", values: ["8", "0"] },
    ],
  },
  {
    label: "Global Shadow Quality",
    keys: ["setting.videocfg_shadow_quality"],
    options: [
      { label: "Low", values: ["0"] },
      { label: "Medium", values: ["1"] },
      { label: "High", values: ["2"] },
      { label: "Very High", values: ["3"] },
    ],
  },
  {
    label: "Dynamic Shadows",
    keys: ["setting.videocfg_dynamic_shadows"],
    options: [
      { label: "Sun Only", values: ["0"] },
      { label: "All", values: ["1"] },
    ],
  },
  {
    label: "Model / Texture Detail",
    keys: ["setting.videocfg_texture_detail"],
    options: [
      // Inferred: community-source-confirmed 0/1/2 sequence; only "1"
      // (Medium) was itself empirically toggled on the dev rig.
      { label: "Low", values: ["0"] },
      { label: "Medium", values: ["1"] },
      { label: "High", values: ["2"] },
    ],
  },
  {
    label: "Texture Filtering Mode",
    keys: ["setting.r_texturefilteringquality"],
    options: [
      // Inferred: sequential 0-5, confirmed at the "2" (Anisotropic 2X)
      // point empirically; 0/1/3/4/5 match a community script's own
      // independently-observed 1=Trilinear/3=Aniso4X/5=Aniso16X points.
      { label: "Bilinear", values: ["0"] },
      { label: "Trilinear", values: ["1"] },
      { label: "Anisotropic 2X", values: ["2"] },
      { label: "Anisotropic 4X", values: ["3"] },
      { label: "Anisotropic 8X", values: ["4"] },
      { label: "Anisotropic 16X", values: ["5"] },
    ],
  },
  {
    label: "Shader Detail",
    keys: ["setting.shaderquality"],
    options: [
      { label: "Low", values: ["0"] },
      { label: "High", values: ["1"] },
    ],
  },
  {
    label: "Particle Detail",
    keys: ["setting.videocfg_particle_detail"],
    options: [
      { label: "Low", values: ["0"] },
      { label: "Medium", values: ["1"] },
      { label: "High", values: ["2"] },
      { label: "Very High", values: ["3"] },
    ],
  },
  {
    label: "Ambient Occlusion",
    keys: ["setting.videocfg_ao_detail"],
    options: [
      // Value 1 is unused by the in-game UI -- not a typo, empirically confirmed.
      { label: "Disabled", values: ["0"] },
      { label: "Medium", values: ["2"] },
      { label: "High", values: ["3"] },
    ],
  },
  {
    label: "High Dynamic Range",
    keys: ["setting.videocfg_hdr_detail"],
    options: [
      { label: "Quality", values: ["-1"] },
      { label: "Performance", values: ["3"] },
    ],
  },
  {
    label: "FidelityFX Super Resolution",
    keys: ["setting.videocfg_fsr_detail"],
    options: [
      { label: "Disabled (Highest Quality)", values: ["0"] },
      { label: "Ultra Quality", values: ["1"] },
      { label: "Quality", values: ["2"] },
      { label: "Balanced", values: ["3"] },
      { label: "Performance", values: ["4"] },
    ],
  },
];

/** Every key any curated setting owns -- used to keep the raw key/value
 * fallback list from double-editing a key a curated dropdown already
 * covers. */
export const CURATED_KEYS = new Set(CS2_VIDEO_SETTINGS.flatMap((s) => s.keys));

/** Builds the `{key: value}` pairs one option selection writes. */
export function settingsForOption(
  setting: Cs2VideoSetting,
  option: Cs2VideoOption
): Record<string, string> {
  const out: Record<string, string> = {};
  setting.keys.forEach((key, i) => {
    out[key] = option.values[i];
  });
  return out;
}

/** The option (if any) whose full value set matches `settings` exactly --
 * used to pre-select a dropdown when editing an existing module. */
export function optionForSettings(
  setting: Cs2VideoSetting,
  settings: Record<string, string>
): Cs2VideoOption | undefined {
  return setting.options.find((option) =>
    setting.keys.every((key, i) => settings[key] === option.values[i])
  );
}
