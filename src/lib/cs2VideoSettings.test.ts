import { describe, expect, it } from "vitest";
import { CS2_VIDEO_SETTINGS, settingsForOption, optionForSettings } from "./cs2VideoSettings";

describe("CS2_VIDEO_SETTINGS", () => {
  it("has exactly 14 curated settings", () => {
    expect(CS2_VIDEO_SETTINGS).toHaveLength(14);
  });

  it("every option's values array has the same length as its setting's keys array", () => {
    for (const setting of CS2_VIDEO_SETTINGS) {
      for (const option of setting.options) {
        expect(option.values).toHaveLength(setting.keys.length);
      }
    }
  });

  it("every key is the full setting.* form, not a bare suffix", () => {
    for (const setting of CS2_VIDEO_SETTINGS) {
      for (const key of setting.keys) {
        expect(key.startsWith("setting.")).toBe(true);
      }
    }
  });

  it("Display Mode: Fullscreen Windowed writes fullscreen=0, nowindowborder=1", () => {
    const displayMode = CS2_VIDEO_SETTINGS.find((s) => s.label === "Display Mode")!;
    const option = displayMode.options.find((o) => o.label === "Fullscreen Windowed")!;
    expect(settingsForOption(displayMode, option)).toEqual({
      "setting.fullscreen": "0",
      "setting.nowindowborder": "1",
    });
  });

  it("Multisampling: CMAA2 writes msaa_samples=0, r_csgo_cmaa_enable=1", () => {
    const msaa = CS2_VIDEO_SETTINGS.find((s) => s.label === "Multisampling Anti-Aliasing Mode")!;
    const option = msaa.options.find((o) => o.label === "CMAA2")!;
    expect(settingsForOption(msaa, option)).toEqual({
      "setting.msaa_samples": "0",
      "setting.r_csgo_cmaa_enable": "1",
    });
  });

  it("optionForSettings finds the matching option from a settings map", () => {
    const shadow = CS2_VIDEO_SETTINGS.find((s) => s.label === "Global Shadow Quality")!;
    const found = optionForSettings(shadow, { "setting.videocfg_shadow_quality": "2" });
    expect(found?.label).toBe("High");
  });

  it("optionForSettings returns undefined when the settings map has no match", () => {
    const shadow = CS2_VIDEO_SETTINGS.find((s) => s.label === "Global Shadow Quality")!;
    expect(optionForSettings(shadow, {})).toBeUndefined();
    expect(optionForSettings(shadow, { "setting.videocfg_shadow_quality": "99" })).toBeUndefined();
  });

  it("Ambient Occlusion skips value 1 (0=Disabled, 2=Medium, 3=High)", () => {
    const ao = CS2_VIDEO_SETTINGS.find((s) => s.label === "Ambient Occlusion")!;
    expect(ao.options.map((o) => o.values[0])).toEqual(["0", "2", "3"]);
  });

  it("High Dynamic Range: Quality is -1, Performance is 3", () => {
    const hdr = CS2_VIDEO_SETTINGS.find((s) => s.label === "High Dynamic Range")!;
    expect(hdr.options.find((o) => o.label === "Quality")?.values).toEqual(["-1"]);
    expect(hdr.options.find((o) => o.label === "Performance")?.values).toEqual(["3"]);
  });

  it("FidelityFX Super Resolution: Ultra Quality is 1, Performance is 4", () => {
    const fsr = CS2_VIDEO_SETTINGS.find((s) => s.label === "FidelityFX Super Resolution")!;
    expect(fsr.options.find((o) => o.label === "Ultra Quality")?.values).toEqual(["1"]);
    expect(fsr.options.find((o) => o.label === "Performance")?.values).toEqual(["4"]);
  });
});
