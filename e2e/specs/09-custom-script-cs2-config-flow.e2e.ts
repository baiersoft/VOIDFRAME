import path from "node:path";
import os from "node:os";
import fs from "node:fs";
import { spawn } from "node:child_process";
import { $, browser, expect } from "@wdio/globals";
import {
  createProjectViaUI,
  deleteProjectsByNamePrefix,
  expectBodyContains,
  projectCard,
  typeIntoSearchBox,
} from "../helpers";

const PREFIX = "E2E Script Config";

// Two real scratch scripts this spec owns end to end -- imported through the
// real `import_custom_script` command (which copies them into the project's
// own `scripts/` dir), hashed and shown verbatim by the real
// `unconfirmed_custom_scripts` in the confirmation modal, and "run" by the
// mock backend's `run_script` (MockController records the call and reports
// exit 0 -- nothing on this machine ever executes them).
const APPLY_SCRIPT = "vf-e2e-apply.bat";
const REVERT_SCRIPT = "vf-e2e-revert.bat";
const APPLY_MARKER = "VOIDFRAME e2e apply marker";
const REVERT_MARKER = "VOIDFRAME e2e revert marker";

// `CustomScriptEditor.tsx`'s Browse buttons open the real NATIVE Windows file
// picker (`@tauri-apps/plugin-dialog`'s `open()`). It renders correctly, but
// it is a Win32 modal that lives entirely outside the webview: WebDriver
// cannot see it, select in it, or type into it -- no selector, no
// `browser.keys`, nothing. Confirmed live while writing this spec.
//
// So the only way to drive this form the way a user actually does is from
// OUTSIDE the webview. This helper script finds the app process's own modal
// dialog window (window class `#32770`, same process id as voidframe.exe),
// brings it to the foreground, and types the path into its already-focused
// "File name" box. It polls for the dialog rather than assuming it is already
// up, so the spec can start it BEFORE clicking Browse -- the click's own
// WebDriver command may not return until the modal is gone.
//
// Requires an interactive, elevated session -- which the suite already needs
// anyway, since the app carries a `requireAdministrator` manifest (UIPI would
// otherwise block synthetic input to its windows) and since the app has to
// have a real desktop to put a window on at all.
const PICK_FILE_PS1 = String.raw`
param([Parameter(Mandatory = $true)][string]$FilePath)

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class VfDialog {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  public static IntPtr FindDialog(uint pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint p; GetWindowThreadProcessId(h, out p);
      if (p != pid || !IsWindowVisible(h)) { return true; }
      StringBuilder sb = new StringBuilder(64);
      GetClassName(h, sb, sb.Capacity);
      if (sb.ToString() == "#32770") { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
"@

$handle = [IntPtr]::Zero
$deadline = (Get-Date).AddSeconds(60)
while ((Get-Date) -lt $deadline) {
  $proc = Get-Process -Name voidframe -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($proc) {
    $handle = [VfDialog]::FindDialog([uint32]$proc.Id)
    if ($handle -ne [IntPtr]::Zero) { break }
  }
  Start-Sleep -Milliseconds 200
}
if ($handle -eq [IntPtr]::Zero) { Write-Error "no file dialog appeared"; exit 1 }

[void][VfDialog]::SetForegroundWindow($handle)
Start-Sleep -Milliseconds 400
if ([VfDialog]::GetForegroundWindow() -ne $handle) { Write-Error "could not focus the file dialog"; exit 2 }

# SendKeys treats + ^ % ~ ( ) { } [ ] as control characters -- brace-escape
# every one of them so the path is typed literally.
$escaped = [regex]::Replace($FilePath, '[+^%~(){}\[\]]', '{$0}')
$shell = New-Object -ComObject WScript.Shell
$shell.SendKeys($escaped)
Start-Sleep -Milliseconds 300
$shell.SendKeys('{ENTER}')
exit 0
`;

// A scratch dir this spec owns (NOT the app's data root -- these are the
// "user's own scripts on disk" the picker browses to; the app copies them
// into its own project dir on import).
let scratchDir = "";

function scratch(name: string): string {
  return path.join(scratchDir, name);
}

/** Picks one option in a `Cs2ConfigEditor` curated dropdown by its visible
 * label. WebdriverIO's own `selectByVisibleText`/`selectByAttribute`/
 * `selectByIndex` are all silent no-ops against this embedded WebDriver
 * provider -- confirmed by direct diagnostic (reading `getValue()` straight
 * back after the call: the `<select>`'s own DOM value is still ""), because
 * they all work by clicking the `<option>` element, which has no clickable
 * box while the dropdown is closed. Same class of provider limitation
 * `helpers.ts`'s `typeIntoSearchBox` documents for `setValue`. Setting the
 * value and dispatching a real bubbling `change` is exactly what a native
 * selection does as far as the app is concerned: React routes a `<select>`'s
 * `onChange` straight off the native `change` event (no value tracker
 * involved, unlike a text input). */
async function selectCuratedOption(label: string, option: string): Promise<void> {
  await browser.execute(
    (l: string, o: string) => {
      const el = document.querySelector<HTMLSelectElement>(`select[aria-label="${l}"]`);
      if (!el) throw new Error(`no <select> labelled ${l}`);
      el.value = o;
      el.dispatchEvent(new Event("change", { bubbles: true }));
    },
    label,
    option
  );
}

/** Starts the native-dialog driver and returns as soon as it is spawned --
 * deliberately NOT awaited to completion, because it has to already be
 * polling while the (possibly blocking) Browse click is issued.
 *
 * Not `detached` and not `stdio: "ignore"`, both deliberately: a detached
 * child here exits 0 immediately without ever running the script (confirmed
 * by direct testing), and with stderr swallowed that failure looks exactly
 * like "the dialog never opened". Inheriting stderr surfaces the script's own
 * `Write-Error` lines in the test output instead. */
function armNativeFilePicker(filePath: string): void {
  const child = spawn(
    path.join(process.env.SystemRoot ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe"),
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", scratch("pick-file.ps1"), "-FilePath", filePath],
    { stdio: ["ignore", "ignore", "inherit"] }
  );
  child.on("error", (e) => console.error("failed to spawn the file-picker driver:", e));
  child.unref();
}

// Exercises the two module kinds the custom_script/cs2_config UI work added,
// end to end through the REAL UI against the same synthetic backend
// 06-mock-run-flow.e2e.ts uses (VOIDFRAME_SIMULATE_RUN + the `mock-run` Cargo
// feature -- see wdio.conf.ts): the cs2_config curated-dropdown builder form,
// the custom_script file-picker form, the one-time "custom scripts run
// elevated" warning shown before the first custom_script add on this
// machine/profile (not a per-run confirmation gate -- that turned out to be
// unpractical friction for a single-user, locally-run app and was removed),
// and the SCRIPT-REVERTED results badge a scenario carrying a custom_script
// earns once its revert has actually run. Timing matches 06's (baseline + 1
// scenario, ~80s of real settle delay).
describe("custom_script + cs2_config — builder forms, one-time warning, SCRIPT-REVERTED badge", () => {
  before(() => {
    scratchDir = fs.mkdtempSync(path.join(os.tmpdir(), "voidframe-e2e-scripts-"));
    fs.writeFileSync(scratch("pick-file.ps1"), PICK_FILE_PS1, "utf8");
    fs.writeFileSync(scratch(APPLY_SCRIPT), `@echo off\r\nrem ${APPLY_MARKER}\r\n`, "utf8");
    fs.writeFileSync(scratch(REVERT_SCRIPT), `@echo off\r\nrem ${REVERT_MARKER}\r\n`, "utf8");
  });

  after(async () => {
    try {
      await deleteProjectsByNamePrefix(PREFIX);
    } finally {
      fs.rmSync(scratchDir, { recursive: true, force: true });
    }
  });

  it("builds both module kinds, shows the one-time warning at most once, and flags the result", async function () {
    this.timeout(5 * 60 * 1000);

    const name = `${PREFIX} ${Date.now()}`;
    await createProjectViaUI(name);
    await (await $(`h3=${name}`)).waitForDisplayed();
    await (await projectCard(name).$("button=Edit Matrix")).click();
    await (await $(`h2=${name}`)).waitForDisplayed();

    await $("button=Add Scenario").click();
    await (await $("span=Scenario 1")).waitForDisplayed();
    await $("button=Browse Catalog").click();

    // --- cs2_config: the curated-dropdown builder form (Cs2ConfigEditor). ---
    await (await $("h4=CS2 Video Config")).waitForDisplayed();
    const cs2Row = () =>
      $('//h4[text()="CS2 Video Config"]/ancestor::div[contains(@class,"glass-card")][1]');
    await (await cs2Row().$("button=Add Module")).click();

    // Three curated settings covering both shapes the mapping has: one
    // two-key setting (Display Mode -> setting.fullscreen +
    // setting.nowindowborder) and two single-key ones -- 4 keys in total,
    // which is what the module summary below must report.
    await selectCuratedOption("Display Mode", "Fullscreen");
    await selectCuratedOption("V-Sync", "Disabled");
    await selectCuratedOption("Global Shadow Quality", "Low");
    await (await cs2Row().$("button=Save")).click();

    // --- custom_script: the file-picker builder form (CustomScriptEditor).
    // Both Browse buttons go through the real OS file dialog, driven from
    // outside the webview -- see PICK_FILE_PS1's note. ---
    await (await $("h4=Custom Script")).waitForDisplayed();
    const scriptRow = () =>
      $('//h4[text()="Custom Script"]/ancestor::div[contains(@class,"glass-card")][1]');
    await (await scriptRow().$("button=Add Module")).click();

    // The one-time "custom scripts run elevated" warning -- shown before the
    // very first custom_script add on this machine/profile, never again
    // after. Whether it appears here depends on whether some earlier run on
    // this same profile already dismissed it, so this is deliberately
    // tolerant of either state rather than asserting it always appears.
    const warningHeading = await $("h2=Custom Scripts Run Elevated");
    if (await warningHeading.isExisting()) {
      await (await $("button=Got It")).click();
      await warningHeading.waitForDisplayed({ reverse: true, timeout: 10_000 });
    }

    // Generous waits: each one covers the driver's own startup (compiling its
    // P/Invoke shim), its poll for the dialog, and the real `import_custom_script`
    // round trip that follows the pick.
    armNativeFilePicker(scratch(APPLY_SCRIPT));
    await (await scriptRow().$('button[aria-label="Browse for apply script"]')).click();
    await expectBodyContains(APPLY_SCRIPT, 60_000);

    armNativeFilePicker(scratch(REVERT_SCRIPT));
    await (await scriptRow().$('button[aria-label="Browse for revert script"]')).click();
    await expectBodyContains(REVERT_SCRIPT, 60_000);

    // Both scripts are picked, but Save stays disabled until a description is
    // filled in too (the form's own `canSave`).
    await expect(scriptRow().$("button=Save")).toBeDisabled();
    await typeIntoSearchBox(await scriptRow().$("textarea"), "e2e scratch script");
    await expect(scriptRow().$("button=Save")).toBeEnabled();
    await (await scriptRow().$("button=Save")).click();

    // Close the catalog (its header X) and confirm both modules landed.
    await (
      await $('//h2[starts-with(text(),"Add Module")]/ancestor::div[contains(@class,"border-b")][1]//button')
    ).click();
    await expectBodyContains("Configured Modules (2)");
    await expectBodyContains("4 settings");
    await expectBodyContains("e2e scratch script");

    // --- Run start goes straight to the countdown -- no per-run gate.
    // Both scripts are brand new to this project, but that no longer blocks
    // anything (the confirmation gate was removed; only the one-time warning
    // above, already handled, ever interrupts adding a custom_script). ---
    await $("button=Launch Autonomous Pipeline").click();

    // --- Countdown -> Live Monitor -> terminal state, same as 06. ---
    await (await $("h2=Starting Run")).waitForDisplayed();
    await $("h2=Starting Run").waitForDisplayed({ reverse: true, timeout: 15_000 });
    await expectBodyContains("Autonomous Pipeline Active");

    await browser.waitUntil(
      async () => {
        const text = await (await $("body")).getText();
        return text.includes("Run Complete") || text.includes("Run Failed");
      },
      { timeout: 4 * 60 * 1000, interval: 2_000, timeoutMsg: "mock run never reached a terminal state" }
    );

    const failedBanner = await $("text=Run Failed");
    if (await failedBanner.isExisting()) {
      const reason = await (await $(".text-white\\/80")).getText().catch(() => "(no reason text found)");
      throw new Error(`Mock run ended in RunFailed -- reason: ${reason}`);
    }

    // --- Results: the badge belongs to THIS scenario's row (it carried the
    // custom_script whose revert ran), and to no other. ---
    await $("button=View Results").click();
    await (await $("button=Percentiles (FPS)")).waitForDisplayed({ timeout: 10_000 });

    await (
      await $('//span[text()="Scenario 1"]/ancestor::tr[1]//span[text()="SCRIPT-REVERTED"]')
    ).waitForDisplayed({ timeout: 10_000 });
    // NewProjectModal's own baseline name -- the baseline carries no modules
    // at all, so its row must never earn the badge.
    await expect(
      $('//span[text()="Stock System Baseline"]/ancestor::tr[1]//span[text()="SCRIPT-REVERTED"]')
    ).not.toBeExisting();

    await $("button=Project Explorer").click();
  });
});
