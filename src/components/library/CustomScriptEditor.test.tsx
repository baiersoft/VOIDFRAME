import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { CustomScriptEditor } from "./CustomScriptEditor";
import * as api from "../../lib/api";

vi.mock("../../lib/api");
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

describe("CustomScriptEditor", () => {
  it("requires a description and both scripts before Save is enabled", () => {
    render(<CustomScriptEditor projectId="p1" initialValue={null} onSave={vi.fn()} />);
    expect(screen.getByRole("button", { name: /save/i })).toBeDisabled();
  });

  it("calls onSave with the imported filenames once both scripts and a description are set", async () => {
    vi.mocked(api.importCustomScript)
      .mockResolvedValueOnce("apply.bat")
      .mockResolvedValueOnce("revert.bat");
    const { open } = await import("@tauri-apps/plugin-dialog");
    vi.mocked(open)
      .mockResolvedValueOnce("C:\\scripts\\apply.bat")
      .mockResolvedValueOnce("C:\\scripts\\revert.bat");

    const onSave = vi.fn();
    render(<CustomScriptEditor projectId="p1" initialValue={null} onSave={onSave} />);

    fireEvent.click(screen.getByRole("button", { name: /browse.*apply/i }));
    await waitFor(() => expect(screen.getByText("apply.bat")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: /browse.*revert/i }));
    await waitFor(() => expect(screen.getByText("revert.bat")).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText(/description/i), {
      target: { value: "disables thing" },
    });

    expect(screen.getByRole("button", { name: /save/i })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(onSave).toHaveBeenCalledWith({
      apply_script: "apply.bat",
      revert_script: "revert.bat",
      requires_reboot: false,
      description: "disables thing",
    });
  });

  it("pre-fills from initialValue when editing an existing module", () => {
    render(
      <CustomScriptEditor
        projectId="p1"
        initialValue={{
          apply_script: "a.bat",
          revert_script: "r.bat",
          requires_reboot: true,
          description: "d",
        }}
        onSave={vi.fn()}
      />
    );
    expect(screen.getByText("a.bat")).toBeInTheDocument();
    expect(screen.getByText("r.bat")).toBeInTheDocument();
    expect(screen.getByLabelText(/description/i)).toHaveValue("d");
    expect(screen.getByLabelText(/requires reboot/i)).toBeChecked();
  });
});
