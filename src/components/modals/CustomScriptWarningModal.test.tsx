import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CustomScriptWarningModal } from "./CustomScriptWarningModal";

describe("CustomScriptWarningModal", () => {
  it("calls onAcknowledge when Got It is clicked", () => {
    const onAcknowledge = vi.fn();
    render(<CustomScriptWarningModal onAcknowledge={onAcknowledge} onCancel={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /got it/i }));
    expect(onAcknowledge).toHaveBeenCalled();
  });

  it("calls onCancel when Cancel is clicked", () => {
    const onCancel = vi.fn();
    render(<CustomScriptWarningModal onAcknowledge={vi.fn()} onCancel={onCancel} />);
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(onCancel).toHaveBeenCalled();
  });
});
