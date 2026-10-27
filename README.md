![Windows](https://img.shields.io/badge/platform-Windows-blue)
![Linux x86_64](https://img.shields.io/badge/platform-Linux_x86__64-blue)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

# Usage Monitor

A floating, see-through bubble that shows how much of your Claude Code and
Codex limits you have used, on Windows and Linux x86_64. Linux ARM64 and macOS
are not supported yet; see [ROADMAP.md](ROADMAP.md).

## Features

- **The bubble:** a round head with your 5-hour usage and a progress ring,
  and a tail with the 7-day usage and time to reset. Left-click it for a
  panel with both windows and their countdowns.
- **Place it anywhere:** drag it, and it snaps to a screen edge when you let
  go near one. Resize it with `Ctrl` + mouse wheel.
- **Always on top:** it stays above other windows, chat notifications
  included, and hides while a fullscreen app (a game, a video, slides) is
  in front.
- **Click-through:** an option to let clicks pass through the bubble to the
  window below.
- **One bubble per provider:** Claude Code is on by default; Codex is
  optional.
- **Tray and alerts:** a tray icon with a usage badge per provider, and
  desktop notifications when usage crosses a threshold or a sign-in expires.
- **Updates itself:** new releases install automatically.

## Requirements

- Windows 10/11, or Linux x86_64.
- Claude Code (CLI or app), installed and signed in. The `claude` CLI must
  be on `PATH` (on Linux, `~/.local/bin` and `~/.claude/local` work too) so
  the app can refresh an expired sign-in.
- For Codex: the Codex CLI, installed and signed in.
- **Windows:** the
  [WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/).
  Windows 11 and most up-to-date Windows 10 machines have it; if not, the
  app tells you and exits.
- **Linux:**
  - WebKitGTK 4.1 and `libayatana-appindicator3-1` for the tray icons. The
    `.deb` installs both.
  - A compositing window manager, or the bubble's corners are drawn
    opaque.
  - An X11 session. Under Wayland the app runs through Xwayland. Without
    Xwayland the bubbles show, but cannot be positioned, snapped or kept on
    top.

## Install

Download from the
[latest release](https://github.com/tiennm99/usage-monitor/releases/latest).

- **Windows:** `usage-monitor.exe`. Put it in a folder you can write to,
  such as `%LOCALAPPDATA%\UsageMonitor\`, and run it. The exe is unsigned:
  when SmartScreen warns, click **More info** → **Run anyway**.
- **Linux AppImage:** `usage-monitor_X.Y.Z_amd64.AppImage`. Put it in a
  folder you can write to, run `chmod +x` on it, and run it.
- **Linux .deb:** `sudo apt install ./usage-monitor_X.Y.Z_amd64.deb`.

The exe and the AppImage are portable: they keep `settings.json` next to
themselves.

## Use

- **Left-click** the bubble for the details panel.
- **Right-click** the bubble or the tray icon for the menu:
  - **Refresh**
  - **Update frequency:** how often usage is fetched.
  - **Providers:** which bubbles to show.
  - **Settings:** start at login, **Reset position**, **Check for updates**
    and the **Auto-update check** interval.
  - **Controls:** resize.
  - **Show widget**
  - **Click-through**
  - **Restart** and **Exit**
- **Tray icon:** a left click shows or hides the bubbles. On Linux any click
  opens the menu, and there is no tooltip.
- **With click-through on**, the bubbles ignore the mouse. Turn it off from
  the tray menu.

Timers follow your local clock. Every 5 minutes means :00, :05, :10 and so
on; hourly means the top of the hour, daily means midnight, and weekly means
Monday at midnight.

### Updates

The app checks GitHub Releases on the **Auto-update check** schedule
(hourly by default). At launch it checks only if a scheduled time has passed
since the last check. A failed check is retried after 1, 2, 4… minutes, but
never later than the next scheduled time.

When a check finds a new release, the exe and the AppImage install it and
restart on their own. The `.deb` only tells you; install the new `.deb` the
same way you installed the first.

## Files and diagnostics

Run with `--diagnose` to write a log, which is cleared on each start.

| What | Windows | Linux |
|------|---------|-------|
| Log (`--diagnose`) | `%TEMP%\usage-monitor.log` | `~/.local/state/usage-monitor/usage-monitor.log` |
| Settings | `settings.json` next to the exe | next to the AppImage; for the `.deb`, `~/.config/UsageMonitor/settings.json` |
| Update downloads, `update-error.log` | `%LOCALAPPDATA%\UsageMonitor\updates\` | `~/.local/share/UsageMonitor/updates/` |
| Web view cache | `%LOCALAPPDATA%\io.github.tiennm99.usagemonitor\` | `~/.local/share/io.github.tiennm99.usagemonitor/` |

On Linux the `XDG_*` variables move these folders when they are set. On
Windows the app also writes `%LOCALAPPDATA%\UsageMonitor\icon.png` and an
`HKCU\Software\Classes\AppUserModelId` entry, which notifications from a
portable exe need.

## Privacy

- **Reads:** your Claude Code credentials (`~/.claude/.credentials.json`)
  and, with Codex on, your Codex credentials (`$CODEX_HOME/auth.json` or
  `~/.codex/auth.json`).
- **Sends:** usage requests to Anthropic and, with Codex on, to ChatGPT,
  plus update checks to GitHub Releases. Nothing else: no analytics, no
  backend, and no credentials sent to any third party.
- **Runs:** when a sign-in has expired, `claude -p .` or `codex exec .`
  runs hidden in the temp folder, so the CLI refreshes its token. Codex runs
  with `--skip-git-repo-check` and a read-only sandbox. Each run sends one
  tiny prompt, which counts toward your usage.
- **Stores:** bubble positions and size, the menu choices, and the time of
  the last update check.

## Build from source

You need [Rust](https://www.rust-lang.org/tools/install) and
[Node.js](https://nodejs.org/). On Debian/Ubuntu, install the build
dependencies first:

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

Then:

```bash
git clone https://github.com/tiennm99/usage-monitor
cd usage-monitor
npm ci
npm run tauri build -- --no-bundle
```

The binary lands in `target/release/`. For the `.deb` and AppImage, install
`patchelf` and run `npm run tauri build -- --bundles deb,appimage` instead.
Local builds are unsigned, so they can't be published as updates; see
[docs/release-process.md](docs/release-process.md).

## Acknowledgements

Inspired by
[CodeZeno/Claude-Code-Usage-Monitor](https://github.com/CodeZeno/Claude-Code-Usage-Monitor),
a taskbar widget for the same problem, and by the floating "boost ball" of
360 Security and IObit Advanced SystemCare. This is a clean-room
implementation against the same public APIs.

## License

Apache License 2.0. See [LICENSE](LICENSE).
