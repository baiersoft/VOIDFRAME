import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { Cs2ConfigEditor } from "./Cs2ConfigEditor";

describe("Cs2ConfigEditor", () => {
  it("renders a dropdown per curated setting, defaulting to Not Set", () => {
    render(<Cs2ConfigEditor initialValue={{}} onSave={vi.fn()} />);
    expect(screen.getByLabelText("Display Mode")).toHaveValue("");
    expect(screen.getByLabelText("Global Shadow Quality")).toHaveValue("");
  });

  it("pre-selects the option matching initialValue's settings", () => {
    render(
      <Cs2ConfigEditor
        initialValue={{ "setting.videocfg_shadow_quality": "2" }}
        onSave={vi.fn()}
      />
    );
    expect(screen.getByLabelText("Global Shadow Quality")).toHaveValue("High");
  });

  it("changing a two-key dropdown writes both keys on save", () => {
    const onSave = vi.fn();
    render(<Cs2ConfigEditor initialValue={{}} onSave={onSave} />);
    fireEvent.change(screen.getByLabelText("Display Mode"), {
      target: { value: "Fullscreen Windowed" },
    });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(onSave).toHaveBeenCalledWith(
      expect.objectContaining({
        "setting.fullscreen": "0",
        "setting.nowindowborder": "1",
      })
    );
  });

  it("lets the user add a raw key/value row for an uncurated setting", () => {
    const onSave = vi.fn();
    render(<Cs2ConfigEditor initialValue={{}} onSave={onSave} />);
    fireEvent.click(screen.getByRole("button", { name: /raw settings/i }));
    fireEvent.click(screen.getByRole("button", { name: /add raw setting/i }));
    fireEvent.change(screen.getByPlaceholderText("key"), {
      target: { value: "setting.monitor_index" },
    });
    fireEvent.change(screen.getByPlaceholderText("value"), { target: { value: "1" } });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(onSave).toHaveBeenCalledWith(
      expect.objectContaining({ "setting.monitor_index": "1" })
    );
  });

  it("raw settings are collapsed by default, even when initialValue has uncurated keys", () => {
    render(
      <Cs2ConfigEditor initialValue={{ "setting.monitor_index": "1" }} onSave={vi.fn()} />
    );
    expect(screen.queryByDisplayValue("setting.monitor_index")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /raw settings \(1\)/i })).toBeInTheDocument();
  });

  it("deleting an existing raw row drops that key from the saved settings", () => {
    const onSave = vi.fn();
    render(
      <Cs2ConfigEditor
        initialValue={{ "setting.monitor_index": "1", "setting.videocfg_shadow_quality": "2" }}
        onSave={onSave}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: /raw settings/i }));
    fireEvent.click(screen.getByRole("button", { name: "" }));
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(onSave).toHaveBeenCalledWith({ "setting.videocfg_shadow_quality": "2" });
  });

  it("renaming an existing raw row drops the original key from the saved settings", () => {
    const onSave = vi.fn();
    render(<Cs2ConfigEditor initialValue={{ "setting.monitor_index": "1" }} onSave={onSave} />);
    fireEvent.click(screen.getByRole("button", { name: /raw settings/i }));
    fireEvent.change(screen.getByDisplayValue("setting.monitor_index"), {
      target: { value: "setting.monitor_idx" },
    });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(onSave).toHaveBeenCalledWith({ "setting.monitor_idx": "1" });
  });

  it("expanding raw settings reveals an existing uncurated key from initialValue", () => {
    render(
      <Cs2ConfigEditor initialValue={{ "setting.monitor_index": "1" }} onSave={vi.fn()} />
    );
    fireEvent.click(screen.getByRole("button", { name: /raw settings/i }));
    expect(screen.getByDisplayValue("setting.monitor_index")).toBeInTheDocument();
    expect(screen.getByDisplayValue("1")).toBeInTheDocument();
  });
});
