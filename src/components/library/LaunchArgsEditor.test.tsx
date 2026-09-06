import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { LaunchArgsEditor } from "./LaunchArgsEditor";

describe("LaunchArgsEditor", () => {
  it("pre-fills the textarea with initialValue", () => {
    render(<LaunchArgsEditor initialValue="-high -threads 8" onSave={vi.fn()} />);
    expect(screen.getByRole("textbox")).toHaveValue("-high -threads 8");
  });

  it("lets the user type arbitrary text, unvalidated", () => {
    render(<LaunchArgsEditor initialValue="" onSave={vi.fn()} />);
    const textarea = screen.getByRole("textbox");
    fireEvent.change(textarea, { target: { value: "-insecure -whatever" } });
    expect(textarea).toHaveValue("-insecure -whatever");
  });

  it("calls onSave with the current textarea value when Save is clicked", () => {
    const onSave = vi.fn();
    render(<LaunchArgsEditor initialValue="-novid" onSave={onSave} />);
    const textarea = screen.getByRole("textbox");
    fireEvent.change(textarea, { target: { value: "-novid -fullscreen" } });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(onSave).toHaveBeenCalledWith("-novid -fullscreen");
  });
});
