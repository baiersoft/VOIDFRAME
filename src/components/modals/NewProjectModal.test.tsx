import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { NewProjectModal } from "./NewProjectModal";

describe("NewProjectModal", () => {
  it("creates a project whose schema_version matches the engine's current SCHEMA_VERSION (2.0.0)", () => {
    // Regression: a stale "1.0.0" literal here was silently accepted by
    // `save_project` (which doesn't validate schema_version) but then
    // rejected by `Project::load` on the very next `list_projects` refresh
    // (crates/voidframe-engine/src/model/project.rs, SCHEMA_VERSION bumped
    // to "2.0.0" for WCPS v3) -- so every newly created project vanished
    // from the dashboard immediately while still sitting on disk.
    const onCreateProject = vi.fn().mockResolvedValue(undefined);
    render(
      <NewProjectModal isOpen={true} onClose={vi.fn()} onCreateProject={onCreateProject} />
    );

    fireEvent.change(screen.getByPlaceholderText(/BYOB Driver Branch/), {
      target: { value: "Test Project" },
    });
    fireEvent.click(screen.getByText("Create Matrix"));

    expect(onCreateProject).toHaveBeenCalledTimes(1);
    const created = onCreateProject.mock.calls[0][0];
    expect(created.schema_version).toBe("2.0.0");
    expect(created.settings.schema_version).toBe("2.0.0");
  });
});
