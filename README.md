![Windows](https://img.shields.io/badge/platform-Windows-blue)
![Linux x86_64](https://img.shields.io/badge/platform-Linux_x86__64-blue)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

# Usage Monitor

A floating, draggable circular bubble that shows your Claude Code and/or
Codex usage on Windows and Linux — inspired by the floating "memory boost
ball" UX of 360 Security and IObit Advanced SystemCare.

Drop it anywhere on screen, drag it around, snap it to a monitor edge,
left-click for a panel with both your 5-hour and 7-day windows, right-click
for the menu.

Linux ARM64 and macOS are not supported yet; see [ROADMAP.md](ROADMAP.md).

## Acknowledgements

Inspired by [CodeZeno/Claude-Code-Usage-Monitor](https://github.com/CodeZeno/Claude-Code-Usage-Monitor),
which solves the same "how close am I to the Claude Code limit?" problem
with a horizontal taskbar widget. This project takes the UX in a different
direction — a floating, draggable circular bubble that the user can place
anywhere on screen — and is a clean-room implementation: the HTTP client,
provider polling, credential discovery, tray rendering, and
self-updater are all written from scratch against the same public APIs
(Anthropic, ChatGPT, GitHub Releases).

## What you get

- A see-through floating bubble: a round head showing your current 5-hour
  Claude Code or Codex usage as a percentage and a colored progress ring,
  with a slimmer tail holding the 7-day usage and reset bars
- Drag anywhere — the bubble snaps to monitor work-area edges when
  released
- Resize with `Ctrl + MouseWheel` on the bubble, or use **Controls** →
  **Make smaller / Make larger / Reset size** from the right-click menu
  (140–360 logical pixels)
- Left-click the bubble for an expanded panel with both **5h** and **7d**
  bars plus reset countdowns
- Right-click for refresh, displayed providers, update frequency, startup,
  **Reset position**, updates, **Show widget**, **Click-through**,
  **Restart** and **Exit**
- A system tray icon per enabled provider, with a usage badge
- Desktop notifications when usage crosses a threshold, when a sign-in
  expires and after an update was applied
- Auto-hide when a fullscreen app is in the foreground (games, video,
  presentations) — reappears when you leave fullscreen
- Bubbles stay above other always-on-top windows, such as chat
  notifications: on Windows a covered bubble is raised again within about
  1.5 s, and on every platform shown bubbles are put back on top every
  minute, so nothing can keep them covered or hidden

## Who this is for

Windows 10/11 and Linux x86_64 users who already have **Claude Code (CLI or
App) installed and signed in** on the same machine. Codex support is
optional — install and sign in to the Codex CLI, then enable Codex from the
right-click **Providers** menu.

## Requirements

- Windows 10 or Windows 11, or Linux x86_64 (see below)
- Claude Code (CLI or App) installed and authenticated
- The `claude` CLI, for automatic token refresh. The app looks for it on
  `PATH`, and on Linux also in `~/.local/bin` and `~/.claude/local`. Without
  it an expired sign-in is not refreshed and you have to sign in again.
- Optional: Codex CLI installed and authenticated, if you want Codex usage.
  Its token is refreshed through the `codex` CLI, found the same way.

### Windows

The app draws its windows with the Microsoft Edge **WebView2 runtime**.
Windows 11 ships it, and most up-to-date Windows 10 machines already have it.
If it is missing, the app shows a message with the download link and exits;
install the
[Evergreen WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/)
and start the app again.

### Linux

- **Runtime libraries:** WebKitGTK 4.1 (`libwebkit2gtk-4.1-0` on
  Debian/Ubuntu) and, for the tray icons, `libayatana-appindicator3-1`.
  The `.deb` pulls these in. Without an AppIndicator library the app runs
  without tray icons.
- **A compositing window manager.** The bubble is a transparent window;
  without a compositor its corners are drawn opaque.
- **X11 is the supported session.** Under Wayland the app runs through
  Xwayland, where positioning, snapping and always-on-top work. Without
  Xwayland (or with `GDK_BACKEND=wayland` set) the bubbles still show, but
  they cannot be positioned, snapped, kept on top or saved where you drop
  them.
- **Tray limits:** Linux tray icons (AppIndicator) only open the menu. They
  show no tooltip, and a left click does not toggle the bubble; use the
  menu instead.

## Install

### Windows

Grab `usage-monitor.exe` from the
[Releases page](https://github.com/tiennm99/usage-monitor/releases/latest).
Put it in a folder of its own that you can write to (e.g.
`%LOCALAPPDATA%\UsageMonitor\`) and run it. The app is portable: it keeps
`settings.json` next to the exe, so moving the folder keeps your settings. It
updates itself from the same Releases feed: when a check finds a new
release, it installs it and restarts on its own, with no click needed.

**First-run note:** the binary is unsigned, so SmartScreen will show
"Windows protected your PC". Click **More info** → **Run anyway**. Code
signing is on the roadmap.

### Linux x86_64

From the same
[Releases page](https://github.com/tiennm99/usage-monitor/releases/latest),
pick one:

- `usage-monitor_X.Y.Z_amd64.AppImage`: make it executable
  (`chmod +x`) and run it. Like the Windows exe it is portable and keeps
  `settings.json` next to the AppImage file, so give it a folder you can
  write to. The AppImage updates itself in place, the same way. If you turned on "Start
  at login" in v0.6.0 or earlier and the AppImage path contains a space,
  turn it off and on again once: those releases wrote the path unquoted,
  so the app never started at login.
- `usage-monitor_X.Y.Z_amd64.deb`: install it with
  `sudo apt install ./usage-monitor_X.Y.Z_amd64.deb`. The app
  still checks for updates, but installing one is left to you: download the
  new `.deb` and install it the same way.

### Build from source

Prerequisites: the [Rust toolchain](https://www.rust-lang.org/tools/install)
and [Node.js](https://nodejs.org/) with npm. The Tauri CLI is a local dev
dependency, so `npm ci` installs it.

On Windows:

```powershell
git clone https://github.com/tiennm99/usage-monitor
cd usage-monitor
npm ci
npm run tauri build -- --no-bundle
```

The binary lands at `target\release\usage-monitor.exe`.

On Linux x86_64, install the build dependencies first (Debian/Ubuntu):

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
git clone https://github.com/tiennm99/usage-monitor
cd usage-monitor
npm ci
npm run tauri build -- --no-bundle
```

The binary lands at `target/release/usage-monitor`. To build the
`.deb` and AppImage instead, also install `patchelf` (the AppImage needs it)
and run `npm run tauri build -- --bundles deb,appimage`; they land under
`target/release/bundle/`.

Local builds carry no updater signature (`createUpdaterArtifacts` is off in
`tauri.conf.json`), so they cannot be published as an update. Only the
release workflow signs builds; see
[docs/release-process.md](docs/release-process.md).

## Use

Run the app. The bubble appears near the bottom-right corner of your
primary monitor on first launch. Drag it where you want it, release to snap
to the nearest edge if you let go close to one.

- **Left-click** the bubble to open the expanded panel (5h + 7d + countdowns)
- **Right-click** for **Refresh**, **Update frequency**, **Providers**,
  **Settings**, **Controls**, **Show widget**, **Click-through**,
  **Restart** and **Exit**.
  **Settings** holds "Start with Windows" ("Start at login" on Linux),
  **Reset position**, a manual **Check for updates** and the
  **Auto-update check** interval (Disabled / Hourly / Daily / Weekly)
- **Drag** anywhere — it floats on top of all other windows
- **Ctrl + MouseWheel** on the bubble, or **Controls** in the right-click
  menu, to resize it
- **Tray icon:** left-click toggles the bubble visibility, right-click opens
  the same menu (on Linux, any click opens the menu)
- **Click-through:** when checked, the bubbles ignore the mouse, so clicks
  reach the windows underneath. You can't drag, resize or right-click a
  bubble then; uncheck it from the tray icon's menu

### Providers

Use the right-click **Providers** menu to choose what is shown:

- **Claude Code** is enabled by default
- **Codex** can be enabled alongside Claude Code or shown by itself

When multiple providers are shown, each gets its own bubble that you can position
independently.

### Changes from the v0.5.0 Windows build

The app moved from a hand-written Win32 UI to Tauri. Most behaviour is the
same; these differences are visible:

- The transparent corners around the round bubble no longer pass clicks
  through to the window below; they just ignore clicks.
- Alerts are Windows toast notifications instead of tray balloons, so they
  no longer use a warning or info icon.
- Proxy auto-configuration (PAC scripts and WPAD) is no longer used. A
  static system proxy, its bypass list and the `HTTPS_PROXY`-style
  environment variables still are, including a SOCKS `ALL_PROXY` such as
  `socks5h://127.0.0.1:7890`, for usage requests and updates alike.
- Only one copy runs per signed-in user session, instead of one per machine.

## Diagnostics

```text
usage-monitor.exe --diagnose     # Windows
usage-monitor --diagnose         # Linux .deb (pass the same flag to the AppImage)
```

This writes a log file, truncated on each start, to:

| OS | Log file |
|----|----------|
| Windows | `%TEMP%\usage-monitor.log` |
| Linux | `$XDG_STATE_HOME/usage-monitor/usage-monitor.log`, usually `~/.local/state/usage-monitor/usage-monitor.log` (readable by your user only) |

Settings are saved to:

| OS | Settings file |
|----|---------------|
| Windows | `settings.json` next to `usage-monitor.exe` |
| Linux AppImage | `settings.json` next to the `.AppImage` file |
| Linux `.deb` | `$XDG_CONFIG_HOME/UsageMonitor/settings.json`, usually `~/.config/UsageMonitor/settings.json` |

Other files the app writes:

| What | Windows | Linux |
|------|---------|-------|
| Update downloads and `update-error.log` | `%LOCALAPPDATA%\UsageMonitor\updates\` | `~/.local/share/UsageMonitor/updates/` |
| Web view profile (cache, storage) | `%LOCALAPPDATA%\io.github.tiennm99.usagemonitor\` (WebView2) | `~/.local/share/io.github.tiennm99.usagemonitor/` (WebKitGTK) |

On Windows the app also writes its notification icon to
`%LOCALAPPDATA%\UsageMonitor\icon.png` and registers it under
`HKCU\Software\Classes\AppUserModelId\io.github.tiennm99.usagemonitor`,
which Windows needs to show toasts from a portable exe.

## Privacy and security

What the app reads:

- Your local Claude Code OAuth credentials from `~/.claude/.credentials.json`
- If Codex is enabled, your local Codex credentials from `$CODEX_HOME/auth.json`
  or `~/.codex/auth.json`

What the app sends over the network:

- Requests to Anthropic's Claude endpoints to read your usage
- Requests to ChatGPT's Codex usage endpoint, if Codex is enabled
- Requests for `latest.json` from GitHub Releases, hourly by default and at
  launch when the last check is older than that. A failed check is retried
  sooner, starting after a minute and backing off up to the interval. Set **Settings** → **Auto-update check** to
  **Disabled** in the right-click menu to stop them; **Check for updates**
  still works when you click it.

What the app runs:

- When a usage request is rejected because the sign-in has expired, the app
  runs `claude -p .` for Claude Code and `codex exec .` for Codex, for each
  provider that was rejected. They run hidden in the background, so the CLI
  refreshes its token. Each run sends one tiny prompt, which counts toward
  your usage. The runs use the system temporary folder as their working
  folder, and Codex runs with `--skip-git-repo-check` and a read-only
  sandbox.

What the app stores locally:

- Bubble position(s) per model
- Bubble size
- Whether the bubbles are shown (**Show widget**)
- Whether the bubbles let clicks through (**Click-through**)
- Polling frequency
- Auto-update check interval
- Last update check time
- Displayed provider preferences

What it does **not** do: send credentials to any third-party server, run a
backend service, collect analytics, upload your project files, or write to
your Codex `auth.json` directly.

## License

Apache License 2.0 — see [LICENSE](LICENSE).
