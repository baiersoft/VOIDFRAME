import { beforeEach, describe, expect, it, vi } from "vitest";

// `api.ts` calls `bindings.ts`'s own generated `commands.X(...)` functions
// (each already an `invoke()` wrapper resolving to a `{ status: "ok" | "error" }`
// tagged union via `bindings.ts`'s own `typedError` helper) rather than
// calling `invoke()` itself, except for the 4 window-chrome commands, which
// have no generated bindings. So the 17 real commands are exercised by
// mocking `./bindings`'s `commands` export directly; the window-chrome
// functions are exercised by mocking `@tauri-apps/api/core`'s `invoke`.

// vi.mock's factory is hoisted above all imports/top-level statements, so
// anything it references must be created via vi.hoisted (not a plain
// top-level const) or the factory runs before that const is initialized.
const { mockCommands, mockInvoke, mockListen } = vi.hoisted(() => ({
  mockCommands: {
    listProjects: vi.fn(),
    getProject: vi.fn(),
    saveProject: vi.fn(),
    deleteProject: vi.fn(),
    validateScenario: vi.fn(),
    preflight: vi.fn(),
    getConfig: vi.fn(),
    saveConfig: vi.fn(),
    listCatalogTweaks: vi.fn(),
    startRun: vi.fn(),
    sendControl: vi.fn(),
    getResults: vi.fn(),
    getRunSnapshot: vi.fn(),
    rollbackNow: vi.fn(),
    emergencyRollback: vi.fn(),
    openDataDir: vi.fn(),
    revealRestoreBat: vi.fn(),
  },
  mockInvoke: vi.fn(),
  mockListen: vi.fn(),
}));

vi.mock("./bindings", () => ({
  commands: mockCommands,
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => mockListen(...args),
}));

// Import AFTER the mocks are registered (vi.mock is hoisted by Vitest, so
// this import ordering is safe -- the mocks are in place before api.ts's own
// top-level code, if any, runs).
import {
  closeWindow,
  getProject,
  getRunSnapshot,
  isWindowMaximized,
  listProjects,
  minimizeWindow,
  saveProject,
  startRun,
  subscribeToEngineEvents,
  toggleMaximizeWindow,
} from "./api";

describe("api command wrappers (generated bindings)", () => {
  beforeEach(() => {
    Object.values(mockCommands).forEach((fn) => fn.mockReset());
  });

  it("listProjects resolves to the command's Ok data", async () => {
    mockCommands.listProjects.mockResolvedValue({
      status: "ok",
      data: [{ id: "p1" }],
    });
    const result = await listProjects();
    expect(result).toEqual([{ id: "p1" }]);
    expect(mockCommands.listProjects).toHaveBeenCalledWith();
  });

  it("getProject passes its argument through and resolves to Ok data", async () => {
    mockCommands.getProject.mockResolvedValue({
      status: "ok",
      data: { id: "p1", name: "Test" },
    });
    const result = await getProject("p1");
    expect(result).toEqual({ id: "p1", name: "Test" });
    expect(mockCommands.getProject).toHaveBeenCalledWith("p1");
  });

  it("an error-status result throws with the backend's error message", async () => {
    mockCommands.getProject.mockResolvedValue({
      status: "error",
      error: "project not found",
    });
    await expect(getProject("missing")).rejects.toThrow("project not found");
  });

  it("startRun passes both arguments through positionally", async () => {
    mockCommands.startRun.mockResolvedValue({ status: "ok", data: "run-123" });
    const result = await startRun("p1", true);
    expect(result).toBe("run-123");
    expect(mockCommands.startRun).toHaveBeenCalledWith("p1", true);
  });

  it("saveProject resolves to void on Ok", async () => {
    mockCommands.saveProject.mockResolvedValue({ status: "ok", data: null });
    await expect(
      saveProject({
        schema_version: "1",
        id: "p1",
        name: "n",
        description: "",
        created_at: "",
        settings: {},
        baseline: { name: "b", description: "" },
      })
    ).resolves.toBeUndefined();
  });

  it("getRunSnapshot resolves to null when there is no active run", async () => {
    mockCommands.getRunSnapshot.mockResolvedValue({ status: "ok", data: null });
    const result = await getRunSnapshot();
    expect(result).toBeNull();
  });
});

describe("window-chrome command wrappers (no generated bindings)", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it("minimizeWindow calls invoke with the raw command name", async () => {
    mockInvoke.mockResolvedValue(undefined);
    await minimizeWindow();
    expect(mockInvoke).toHaveBeenCalledWith("minimize_window");
  });

  it("toggleMaximizeWindow resolves to invoke's boolean result", async () => {
    mockInvoke.mockResolvedValue(true);
    const result = await toggleMaximizeWindow();
    expect(result).toBe(true);
    expect(mockInvoke).toHaveBeenCalledWith("toggle_maximize_window");
  });

  it("closeWindow calls invoke with the raw command name", async () => {
    mockInvoke.mockResolvedValue(undefined);
    await closeWindow();
    expect(mockInvoke).toHaveBeenCalledWith("close_window");
  });

  it("isWindowMaximized resolves to invoke's boolean result", async () => {
    mockInvoke.mockResolvedValue(false);
    const result = await isWindowMaximized();
    expect(result).toBe(false);
    expect(mockInvoke).toHaveBeenCalledWith("is_window_maximized");
  });
});

describe("subscribeToEngineEvents", () => {
  beforeEach(() => {
    mockListen.mockReset();
  });

  it("listens on the vf:event channel and forwards the payload to the handler", async () => {
    let capturedCallback: ((e: { payload: unknown }) => void) | undefined;
    const unlisten = vi.fn();
    mockListen.mockImplementation((_channel: string, cb: (e: { payload: unknown }) => void) => {
      capturedCallback = cb;
      return Promise.resolve(unlisten);
    });

    const handler = vi.fn();
    subscribeToEngineEvents(handler);

    expect(mockListen).toHaveBeenCalledWith("vf:event", expect.any(Function));

    // Let the listen() promise resolve.
    await Promise.resolve();
    await Promise.resolve();

    capturedCallback?.({ payload: { type: "RunComplete", run_id: "r1" } });
    expect(handler).toHaveBeenCalledWith({ type: "RunComplete", run_id: "r1" });
  });

  it("returns an unsubscribe function that calls the underlying unlisten", async () => {
    const unlisten = vi.fn();
    mockListen.mockResolvedValue(unlisten);

    const unsubscribe = subscribeToEngineEvents(vi.fn());
    await Promise.resolve();
    await Promise.resolve();

    unsubscribe();
    expect(unlisten).toHaveBeenCalled();
  });
});
