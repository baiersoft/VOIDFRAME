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

  it("defaults to the Dust2 workshop benchmark with map_id set", () => {
    const onCreateProject = vi.fn().mockResolvedValue(undefined);
    render(
      <NewProjectModal isOpen={true} onClose={vi.fn()} onCreateProject={onCreateProject} />
    );
    fireEvent.change(screen.getByPlaceholderText(/BYOB Driver Branch/), {
      target: { value: "Test Project" },
    });
    fireEvent.click(screen.getByText("Create Matrix"));

    const created = onCreateProject.mock.calls[0][0];
    expect(created.settings.benchmark_kind).toBe("workshop_dust2");
    expect(created.settings.map_id).toBe("3240880604");
    expect(created.settings.capture_seconds).toBe(105);
  });

  it("switching to AveYo keeps the (inert) Dust2 map_id and sets the AveYo capture default", () => {
    // `map_id` is inert for AveYo (spec D4) but must stay a valid id: an
    // empty sentinel would make the project fail `Settings::validate` -- and
    // silently vanish from the dashboard -- if its kind were ever switched
    // back to Dust2.
    const onCreateProject = vi.fn().mockResolvedValue(undefined);
    render(
      <NewProjectModal isOpen={true} onClose={vi.fn()} onCreateProject={onCreateProject} />
    );
    fireEvent.change(screen.getByPlaceholderText(/BYOB Driver Branch/), {
      target: { value: "Test Project" },
    });
    fireEvent.click(screen.getByLabelText(/AveYo benchmark\.cfg v2/));
    fireEvent.click(screen.getByText("Create Matrix"));

    const created = onCreateProject.mock.calls[0][0];
    expect(created.settings.benchmark_kind).toBe("aveyo_cfg_v2");
    expect(created.settings.map_id).toBe("3240880604");
    expect(created.settings.capture_seconds).toBe(57);
  });

  it("switching to AveYo defaults Measurement Loops to 5", () => {
    const onCreateProject = vi.fn().mockResolvedValue(undefined);
    render(
      <NewProjectModal isOpen={true} onClose={vi.fn()} onCreateProject={onCreateProject} />
    );
    fireEvent.change(screen.getByPlaceholderText(/BYOB Driver Branch/), {
      target: { value: "Test Project" },
    });
    fireEvent.click(screen.getByLabelText(/AveYo benchmark\.cfg v2/));
    fireEvent.click(screen.getByText("Create Matrix"));

    const created = onCreateProject.mock.calls[0][0];
    expect(created.settings.measure_loops).toBe(5);
  });

  it("defaults to 3 Measurement Loops for the untouched Dust2 default", () => {
    const onCreateProject = vi.fn().mockResolvedValue(undefined);
    render(
      <NewProjectModal isOpen={true} onClose={vi.fn()} onCreateProject={onCreateProject} />
    );
    fireEvent.change(screen.getByPlaceholderText(/BYOB Driver Branch/), {
      target: { value: "Test Project" },
    });
    fireEvent.click(screen.getByText("Create Matrix"));

    const created = onCreateProject.mock.calls[0][0];
    expect(created.settings.measure_loops).toBe(3);
  });

  it("manually selecting 8 Measurement Loops overrides the kind default", () => {
    const onCreateProject = vi.fn().mockResolvedValue(undefined);
    render(
      <NewProjectModal isOpen={true} onClose={vi.fn()} onCreateProject={onCreateProject} />
    );
    fireEvent.change(screen.getByPlaceholderText(/BYOB Driver Branch/), {
      target: { value: "Test Project" },
    });
    // Warmup Loops and Measurement Loops are the two <select>s, in DOM order.
    fireEvent.change(screen.getAllByRole("combobox")[1], {
      target: { value: "8" },
    });
    fireEvent.click(screen.getByText("Create Matrix"));

    const created = onCreateProject.mock.calls[0][0];
    expect(created.settings.measure_loops).toBe(8);
  });

  it("switching benchmark kind resets a manually-selected loop count to the new kind's default", () => {
    const onCreateProject = vi.fn().mockResolvedValue(undefined);
    render(
      <NewProjectModal isOpen={true} onClose={vi.fn()} onCreateProject={onCreateProject} />
    );
    fireEvent.change(screen.getByPlaceholderText(/BYOB Driver Branch/), {
      target: { value: "Test Project" },
    });
    fireEvent.click(screen.getByLabelText(/AveYo benchmark\.cfg v2/));
    // Warmup Loops and Measurement Loops are the two <select>s, in DOM order.
    fireEvent.change(screen.getAllByRole("combobox")[1], {
      target: { value: "8" },
    });
    fireEvent.click(screen.getByText(/Dust2 Workshop/));
    fireEvent.click(screen.getByText("Create Matrix"));

    const created = onCreateProject.mock.calls[0][0];
    expect(created.settings.benchmark_kind).toBe("workshop_dust2");
    expect(created.settings.measure_loops).toBe(3);
  });
});
