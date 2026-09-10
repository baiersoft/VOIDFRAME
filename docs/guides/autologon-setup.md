# 🔐 Windows AutoLogon Setup Guide

| | |
| :--- | :--- |
| **Status** | Draft |
| **Last updated** | 2026-08-31 |
| **Audience** | VOIDFRAME operators running reboot-required matrices |

For `baiersoft // VOIDFRAME` to reboot Windows during scenario testing (HAGS, GPU
interrupt affinity, kernel timer tweaks, driver installs) and resume the pipeline
**without a password prompt at the lock screen**, Windows must log in
automatically.

> **Pre-flight note.** VOIDFRAME inspects your logon configuration before
> launching any matrix that contains reboots. If it finds a Microsoft account or
> Windows Hello without AutoLogon, it warns you so an overnight run does not stall
> at the lock screen. A local account with no password (Method 1) proceeds
> without a warning.

> [!IMPORTANT]
> VOIDFRAME **never writes your password** to the registry. It only reads your
> logon configuration to produce the pre-flight warning. Configuring AutoLogon is
> a manual step you perform with the methods below.

---

## Method 1 — Local account without a password *(recommended for dedicated benchmark rigs)*

The cleanest zero-friction option for a machine used only for testing / gaming:

1. `Windows + I` → **Accounts** → **Your info**.
2. If linked to a Microsoft account: **"Sign in with a local account instead"**.
3. When prompted for a password, leave every field **blank**.
4. On reboot, Windows boots straight to the desktop and the VOIDFRAME resume task
   starts with elevated privileges.

No secret is stored anywhere because there is no secret.

---

## Method 2 — Sysinternals AutoLogon *(recommended when you must keep a password)*

Microsoft's official **AutoLogon** utility stores the credential encrypted in the
Windows **LSA secrets** store — not in plain text.

1. Download from Microsoft:
   👉 [Microsoft Learn: Sysinternals AutoLogon](https://learn.microsoft.com/en-us/sysinternals/downloads/autologon)
2. Run `Autologon.exe` as Administrator.
3. Enter username, domain / PC name, and password.
4. Click **Enable**.
5. When your benchmarking is done, run `Autologon.exe` again and click **Disable**
   to restore normal login security.

---

## Method 3 — `netplwiz` *(acceptable)*

1. `Windows + R` → `netplwiz` → Enter.
2. Select your account.
3. Uncheck **"Users must enter a user name and password to use this computer"**.
4. **Apply**, enter the password twice, **OK**.

Note: on current Windows builds this option is hidden if Windows Hello sign-in is
enforced — turn off *"require Windows Hello sign-in for Microsoft accounts"* in
**Settings → Accounts → Sign-in options** first.

---

## Not recommended — manual `AutoAdminLogon` / `DefaultPassword`

Setting `AutoAdminLogon = 1` with a `DefaultPassword` value under
`HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon` stores your password
**in clear text** in the registry, readable by any administrator or malware. Use
Method 1 or 2 instead. VOIDFRAME's pre-flight flags a present `DefaultPassword`
as a warning.

---

## After your benchmarking session

- **Method 1:** optionally set a password again (**Accounts → Sign-in options**).
- **Method 2:** run `Autologon.exe` → **Disable**.
- **Method 3:** re-check the `netplwiz` box.

Leaving AutoLogon enabled means anyone with physical access boots straight into
your session.

---

## Changelog

- **2026-08-31** — Moved from `docs/AUTO_LOGON_GUIDE.md`. Reordered so the two
  secret-free / encrypted methods lead; added an explicit "not recommended"
  section for clear-text `DefaultPassword` and the note that VOIDFRAME never
  writes it (finding D2); added the current-Windows `netplwiz` / Windows Hello
  caveat.
