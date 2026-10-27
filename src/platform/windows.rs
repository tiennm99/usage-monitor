//! Windows-only helpers. This is the only module that talks to Win32 APIs.

use std::io;
use std::path::Path;
use std::time::Duration;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    GetAvailableCoreWebView2BrowserVersionString, ICoreWebView2Settings3,
};
use windows::core::{Interface, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, SetLastError, HWND, LPARAM, LRESULT, RECT, WAIT_OBJECT_0,
    WIN32_ERROR, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWM_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
use windows::Win32::UI::Shell::{
    DefSubclassProc, RemoveWindowSubclass, SHAppBarMessage,
    SetCurrentProcessExplicitAppUserModelID, SetWindowSubclass, ABE_BOTTOM, ABE_LEFT, ABE_RIGHT,
    ABE_TOP, ABM_GETTASKBARPOS, APPBARDATA,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, MessageBoxW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE, GW_HWNDPREV, HWND_TOPMOST,
    MA_NOACTIVATE, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOWNOACTIVATE, WM_DISPLAYCHANGE,
    WM_EXITSIZEMOVE, WM_MOUSEACTIVATE, WM_NCDESTROY, WM_SETTINGCHANGE, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

use crate::core::geom::{self, Edge, Rect};
use crate::core::proxy::{self, SystemProxy};

const INTERNET_SETTINGS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
const PERSONALIZE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const AUMID_KEY_PREFIX: &str = r"Software\Classes\AppUserModelId\";

/// The static proxy from `HKCU\…\Internet Settings`, or `None` when
/// `ProxyEnable` is not 1 or no usable `ProxyServer` entry exists.
/// PAC/WPAD (`AutoConfigURL`, auto-detect) is deliberately not evaluated.
pub fn system_proxy() -> Option<SystemProxy> {
    let key = windows_registry::CURRENT_USER
        .open(INTERNET_SETTINGS_KEY)
        .ok()?;
    let enable = key.get_u32("ProxyEnable").ok();
    let server = key.get_string("ProxyServer").ok();
    let proxy_override = key.get_string("ProxyOverride").ok();
    proxy::from_registry_values(enable, server.as_deref(), proxy_override.as_deref())
}

/// Whether the apps theme is dark. `SystemUsesLightTheme` is 1 for light;
/// a missing value means dark, as on a fresh Windows 11 install.
pub fn is_dark() -> bool {
    let light = windows_registry::CURRENT_USER
        .open(PERSONALIZE_KEY)
        .and_then(|key| key.get_u32("SystemUsesLightTheme"));
    !matches!(light, Ok(1))
}

/// Wait until process `pid` has exited, at most `timeout`. Returns true when
/// it is gone (or never existed), false on timeout or a failed wait.
pub fn wait_pid(pid: u32, timeout: Duration) -> bool {
    // SAFETY: plain FFI call; the returned handle is closed below.
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }) else {
        // No such process: it already exited.
        return true;
    };
    let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: `handle` is a valid process handle opened with SYNCHRONIZE.
    let result = unsafe { WaitForSingleObject(handle, ms) };
    // SAFETY: `handle` came from OpenProcess and is closed exactly once.
    if let Err(err) = unsafe { CloseHandle(handle) } {
        log::debug!("CloseHandle for pid {pid} failed: {err}");
    }
    result == WAIT_OBJECT_0
}

/// Whether the Microsoft Edge WebView2 runtime is installed.
pub fn webview2_available() -> bool {
    let mut version = PWSTR::null();
    // SAFETY: a null folder asks for the installed Evergreen runtime; the
    // out pointer is valid for the call.
    let result =
        unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version) };
    // Frees the CoTaskMem string; a null pointer yields "".
    let version = webview2_com::take_pwstr(version);
    match result {
        Ok(()) if !version.is_empty() => {
            log::info!("WebView2 runtime {version}");
            true
        }
        Ok(()) => false,
        Err(err) => {
            log::warn!("WebView2 runtime lookup failed: {err}");
            false
        }
    }
}

/// Show a blocking error message box with no owner window.
pub fn message_box_error(title: &str, text: &str) {
    // SAFETY: both strings outlive the call; no owner window is passed.
    let _ = unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from(title),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        )
    };
}

/// Register `aumid` so toasts from this portable exe are shown although no
/// Start-menu shortcut carries the id: write
/// `HKCU\Software\Classes\AppUserModelId\<aumid>` with `DisplayName` and
/// `IconUri` (`icon_png` written to `icon_path`), then make it the process
/// id. Each step is attempted; the first error is returned after all ran.
pub fn register_aumid(
    aumid: &str,
    display_name: &str,
    icon_png: &[u8],
    icon_path: &Path,
) -> io::Result<()> {
    let icon = write_if_changed(icon_path, icon_png);
    if let Err(err) = &icon {
        log::warn!("AUMID icon {} not written: {err}", icon_path.display());
    }

    let key = windows_registry::CURRENT_USER
        .create(format!("{AUMID_KEY_PREFIX}{aumid}"))
        .and_then(|key| {
            key.set_string("DisplayName", display_name)?;
            if icon.is_ok() {
                key.set_string("IconUri", icon_path.to_string_lossy().as_ref())?;
            }
            Ok(())
        })
        .map_err(io::Error::from);

    // SAFETY: the HSTRING outlives the call.
    let process = unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(aumid)) }
        .map_err(io::Error::from);

    key.and(process).and(icon)
}

/// Write `bytes` to `path` unless it already holds exactly them.
fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if std::fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, bytes)
}

/// The current user's start-at-login entry, `HKCU\…\Run\<name>`. Its data
/// is the quoted exe path, as every earlier release wrote it, so a path with
/// spaces cannot resolve to a planted prefix. HKLM is never written, so an
/// elevated run cannot leave an entry that a normal run cannot remove.
/// `is_enabled` honours a Task Manager disable in `StartupApproved\Run`.
pub fn autostart_entry(name: &str) -> io::Result<auto_launch::AutoLaunch> {
    let exe = std::env::current_exe()?;
    Ok(auto_launch::AutoLaunch::new(
        name,
        &quoted_path(&exe),
        auto_launch::WindowsEnableMode::CurrentUser,
        &[] as &[&str],
    ))
}

/// `path` wrapped in double quotes, the form a `Run` value needs.
fn quoted_path(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

/// The window handle of `window`.
fn hwnd_of(window: &tauri::WebviewWindow) -> io::Result<HWND> {
    window
        .hwnd()
        .map_err(|err| io::Error::other(format!("no HWND for {}: {err}", window.label())))
}

/// Make a bubble a tool window that never activates: no Alt-Tab entry and
/// no focus taken on a click or a show. Run it before the first show.
pub fn apply_no_activate(window: &tauri::WebviewWindow) -> io::Result<()> {
    add_ex_style(window, (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW).0)
}

/// Make the details panel a tool window, so it has no Alt-Tab entry
/// (`skip_taskbar` only removes the taskbar button). It still activates:
/// the panel takes focus so that losing it can dismiss the panel. Run it
/// before the first show.
pub fn apply_toolwindow(window: &tauri::WebviewWindow) -> io::Result<()> {
    add_ex_style(window, WS_EX_TOOLWINDOW.0)
}

/// Let mouse input pass through a bubble to the windows below, or take it
/// again. These are the bits tao's `set_ignore_cursor_events` sets, without
/// its re-applying every window flag, which hides a bubble shown through
/// [`show_no_activate`]. Run it on the window's own (main) thread.
pub fn set_click_through(window: &tauri::WebviewWindow, ignore: bool) -> io::Result<()> {
    set_ex_style(window, (WS_EX_TRANSPARENT | WS_EX_LAYERED).0, ignore)
}

/// OR `bits` into a window's extended style and apply the frame change.
fn add_ex_style(window: &tauri::WebviewWindow, bits: u32) -> io::Result<()> {
    set_ex_style(window, bits, true)
}

/// Set or clear `bits` in a window's extended style and apply the frame
/// change.
fn set_ex_style(window: &tauri::WebviewWindow, bits: u32, on: bool) -> io::Result<()> {
    let hwnd = hwnd_of(window)?;
    let bits = bits as isize;
    // SAFETY: `hwnd` is a live top-level window of this process.
    let current = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let wanted = if on { current | bits } else { current & !bits };
    if wanted == current {
        return Ok(());
    }
    // SAFETY: as above. A zero return is only an error when the last error
    // is set, so it is cleared first.
    unsafe { SetLastError(WIN32_ERROR(0)) };
    let previous = unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted) };
    if previous == 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error().unwrap_or(0) != 0 {
            return Err(err);
        }
    }
    // Extended style changes take effect after a frame change.
    // SAFETY: as above; nothing moves, resizes or activates.
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )
    }
    .map_err(io::Error::from)
}

/// Ask DWM for rounded window corners. Only Windows 11 knows the
/// attribute; Windows 10 returns an error and keeps square corners. DWM
/// ignores a repeated identical value, so calling this on every show is
/// cheap.
pub fn round_corners(window: &tauri::WebviewWindow) -> io::Result<()> {
    let hwnd = hwnd_of(window)?;
    let preference = DWMWCP_ROUND;
    // SAFETY: `hwnd` is a live window of this process; the pointer and size
    // describe `preference`, which outlives the call.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&preference).cast(),
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        )
    }
    .map_err(io::Error::from)
}

/// Turn off WebView2's browser accelerator keys (F5, Ctrl+R, Ctrl+P,
/// Ctrl+F, zoom, F12 and the like) for `window`'s webview, then reload it,
/// because the setting applies from the next navigation. Call it only after
/// the first navigation has finished, or the reload can replace that
/// navigation. The work runs on the main thread; a failure inside the
/// webview is logged there.
pub fn disable_browser_accelerator_keys(window: &tauri::WebviewWindow) -> io::Result<()> {
    let label = window.label().to_string();
    window
        .with_webview(move |webview| {
            // SAFETY: plain COM calls on the live controller Tauri handed
            // over, on the main thread that owns it.
            let result = unsafe {
                webview.controller().CoreWebView2().and_then(|core| {
                    let settings: ICoreWebView2Settings3 = core.Settings()?.cast()?;
                    settings.SetAreBrowserAcceleratorKeysEnabled(false)?;
                    core.Reload()
                })
            };
            if let Err(err) = result {
                log::warn!("{label} keeps the browser accelerator keys: {err}");
            }
        })
        .map_err(|err| io::Error::other(format!("no webview for {}: {err}", window.label())))
}

/// Show a bubble without activating it. Bubbles are shown and hidden only
/// through this and [`hide`], never through Tauri's `show`/`hide`.
pub fn show_no_activate(window: &tauri::WebviewWindow) -> io::Result<()> {
    let hwnd = hwnd_of(window)?;
    // SAFETY: `hwnd` is a live window of this process. The return value is
    // the previous visibility, not an error.
    let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    Ok(())
}

/// Put a bubble back at the top of the always-on-top band, above any
/// topmost window that came after it, without activating it.
pub fn raise_topmost(window: &tauri::WebviewWindow) -> io::Result<()> {
    let hwnd = hwnd_of(window)?;
    // SAFETY: `hwnd` is a live window of this process; nothing moves,
    // resizes or activates.
    unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    }
    .map_err(io::Error::from)
}

/// Whether a visible always-on-top window of another process sits above
/// `window` in the z-order and overlaps it, such as a chat app's toast.
/// Our own windows (the panel, a context menu) never count, so raising a
/// bubble over them is never asked for.
pub fn covered_by_other_topmost(window: &tauri::WebviewWindow) -> io::Result<bool> {
    let hwnd = hwnd_of(window)?;
    let Some((ours, _)) = visible_window_bounds(hwnd) else {
        return Ok(false);
    };
    let own_pid = std::process::id();
    // SAFETY: `hwnd` is a live window of this process; each step only reads
    // the z-order, and a window closing mid-walk ends it with an error.
    let mut above = unsafe { GetWindow(hwnd, GW_HWNDPREV) };
    // Topmost windows sit at the head of the z-order, so the walk is short;
    // the cap guards against a z-order that keeps changing under it.
    for _ in 0..MAX_Z_ORDER_WALK {
        let Ok(other) = above else {
            return Ok(false);
        };
        if covers(other, &ours, own_pid) {
            return Ok(true);
        }
        // SAFETY: as above.
        above = unsafe { GetWindow(other, GW_HWNDPREV) };
    }
    Ok(false)
}

const MAX_Z_ORDER_WALK: usize = 1024;

fn covers(other: HWND, ours: &Rect, own_pid: u32) -> bool {
    // SAFETY: read-only queries; a stale handle yields 0 / false.
    let ex_style = unsafe { GetWindowLongPtrW(other, GWL_EXSTYLE) } as u32;
    if ex_style & WS_EX_TOPMOST.0 == 0
        || unsafe { !IsWindowVisible(other).as_bool() }
        || is_dwm_cloaked(other)
        || window_process_id(other).is_none_or(|pid| pid == own_pid)
    {
        return false;
    }
    visible_window_bounds(other).is_some_and(|(theirs, _)| theirs.intersects(ours))
}

/// Hide a bubble (see [`show_no_activate`]).
pub fn hide(window: &tauri::WebviewWindow) -> io::Result<()> {
    let hwnd = hwnd_of(window)?;
    // SAFETY: as in `show_no_activate`.
    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
    Ok(())
}

/// A bubble window message the app reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BubbleHookEvent {
    /// An OS move loop ended (`WM_EXITSIZEMOVE`): the user dropped the bubble.
    MoveEnd,
    /// A system setting changed (`WM_SETTINGCHANGE`): theme, taskbar, work area.
    SettingChange,
    /// The display layout or resolution changed (`WM_DISPLAYCHANGE`).
    DisplayChange,
}

type BubbleHookCallback = Box<dyn Fn(BubbleHookEvent) + Send + 'static>;

/// Subclass id of the bubble hook ("BUBB").
const BUBBLE_SUBCLASS_ID: usize = 0x4255_4242;

/// Subclass a bubble window so `callback` hears about move ends, setting
/// changes and display changes. The callback runs inside the window
/// procedure on the main thread, after the default handling; it must not
/// block, and should hand any Tauri work to another thread. The hook also
/// answers `WM_MOUSEACTIVATE` with `MA_NOACTIVATE`, so a click never
/// activates the bubble. Must be called on the main thread (the window's
/// thread); the hook removes itself when the window is destroyed.
pub fn install_bubble_hook(
    window: &tauri::WebviewWindow,
    callback: impl Fn(BubbleHookEvent) + Send + 'static,
) -> io::Result<()> {
    let hwnd = hwnd_of(window)?;
    let boxed: Box<BubbleHookCallback> = Box::new(Box::new(callback));
    let data = Box::into_raw(boxed);
    // SAFETY: `hwnd` belongs to this thread; `data` stays valid until the
    // hook frees it on WM_NCDESTROY.
    let installed = unsafe {
        SetWindowSubclass(
            hwnd,
            Some(bubble_subclass_proc),
            BUBBLE_SUBCLASS_ID,
            data as usize,
        )
    };
    if installed.as_bool() {
        Ok(())
    } else {
        // SAFETY: the hook was not installed, so nothing else owns `data`.
        drop(unsafe { Box::from_raw(data) });
        Err(io::Error::other(format!(
            "SetWindowSubclass failed for {}",
            window.label()
        )))
    }
}

unsafe extern "system" fn bubble_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_NCDESTROY => {
            // SAFETY: the hook was installed on this window with this proc
            // and id; `data` is the box `install_bubble_hook` leaked, freed
            // exactly once here after the hook is gone.
            unsafe {
                let _ = RemoveWindowSubclass(hwnd, Some(bubble_subclass_proc), BUBBLE_SUBCLASS_ID);
                drop(Box::from_raw(data as *mut BubbleHookCallback));
                return DefSubclassProc(hwnd, msg, wparam, lparam);
            }
        }
        _ => {}
    }
    // SAFETY: forwarding the message down the subclass chain.
    let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    let event = match msg {
        WM_EXITSIZEMOVE => Some(BubbleHookEvent::MoveEnd),
        WM_SETTINGCHANGE => Some(BubbleHookEvent::SettingChange),
        WM_DISPLAYCHANGE => Some(BubbleHookEvent::DisplayChange),
        _ => None,
    };
    if let Some(event) = event {
        // SAFETY: `data` stays valid until WM_NCDESTROY, handled above.
        let callback = unsafe { &*(data as *const BubbleHookCallback) };
        callback(event);
    }
    result
}

/// The taskbar's docked edge and rect, when the shell reports one.
pub fn taskbar() -> Option<(Edge, Rect)> {
    let mut data = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        ..Default::default()
    };
    // SAFETY: `data` is a correctly sized APPBARDATA.
    if unsafe { SHAppBarMessage(ABM_GETTASKBARPOS, &mut data) } == 0 {
        return None;
    }
    let edge = match data.uEdge {
        ABE_LEFT => Edge::Left,
        ABE_TOP => Edge::Top,
        ABE_RIGHT => Edge::Right,
        ABE_BOTTOM => Edge::Bottom,
        _ => return None,
    };
    let rc = data.rc;
    Some((edge, Rect::new(rc.left, rc.top, rc.right, rc.bottom)))
}

/// Bring `window` to the foreground. A popup menu needs its owner in the
/// foreground, or it does not dismiss on a click elsewhere.
pub fn set_foreground(window: &tauri::WebviewWindow) {
    match window.hwnd() {
        // SAFETY: the handle belongs to a live window of this process.
        Ok(hwnd) => {
            if !unsafe { SetForegroundWindow(hwnd) }.as_bool() {
                log::debug!("SetForegroundWindow({}) refused", window.label());
            }
        }
        Err(err) => log::warn!("no HWND for {}: {err}", window.label()),
    }
}

// ---------- Fullscreen detection ----------

/// Shell windows that cover a monitor without being a fullscreen app.
const IGNORED_FULLSCREEN_CLASSES: [&str; 3] = ["Progman", "WorkerW", "Shell_TrayWnd"];

struct FullscreenEvaluation {
    is_fullscreen: bool,
    hwnd: HWND,
    class_name: String,
    bounds: Option<Rect>,
    bounds_source: &'static str,
    monitor: Option<Rect>,
    reason: &'static str,
}

/// Whether the foreground window covers its whole monitor as a fullscreen
/// app: not a window of this process (bubbles, panel, popup menu, WebView
/// children), not a shell window, visible, not minimized or cloaked, its
/// visible bounds within 2 px of the monitor rect and its style borderless
/// or popup.
pub fn foreground_is_fullscreen() -> super::FullscreenCheck {
    // SAFETY: plain FFI call with no arguments.
    let fg = unsafe { GetForegroundWindow() };
    let evaluation = evaluate_foreground_fullscreen(fg);
    super::FullscreenCheck {
        fullscreen: evaluation.is_fullscreen,
        reason: evaluation.reason,
        detail: format!(
            "fg=0x{:X} class={} bounds_source={} bounds={} monitor={}",
            evaluation.hwnd.0 as usize,
            if evaluation.class_name.is_empty() {
                "<unknown>"
            } else {
                evaluation.class_name.as_str()
            },
            evaluation.bounds_source,
            format_rect(evaluation.bounds.as_ref()),
            format_rect(evaluation.monitor.as_ref()),
        ),
    }
}

fn evaluate_foreground_fullscreen(fg: HWND) -> FullscreenEvaluation {
    let mut evaluation = FullscreenEvaluation {
        is_fullscreen: false,
        hwnd: fg,
        class_name: String::new(),
        bounds: None,
        bounds_source: "none",
        monitor: None,
        reason: "not evaluated",
    };

    if fg.is_invalid() {
        evaluation.reason = "no foreground window";
        return evaluation;
    }

    evaluation.class_name = window_class_name(fg);
    if window_process_id(fg) == Some(std::process::id()) {
        evaluation.reason = "foreground window is ours";
        return evaluation;
    }
    if is_ignored_fullscreen_class(&evaluation.class_name) {
        evaluation.reason = "ignored foreground class";
        return evaluation;
    }
    // SAFETY: plain queries on a window handle; a stale one returns false.
    if unsafe { !IsWindowVisible(fg).as_bool() || IsIconic(fg).as_bool() } {
        evaluation.reason = "foreground invisible or minimized";
        return evaluation;
    }
    if is_dwm_cloaked(fg) {
        evaluation.reason = "foreground cloaked";
        return evaluation;
    }

    let Some((bounds, source)) = visible_window_bounds(fg) else {
        evaluation.reason = "foreground bounds unavailable";
        return evaluation;
    };
    evaluation.bounds = Some(bounds);
    evaluation.bounds_source = source;

    // SAFETY: as above; NEAREST always yields a monitor for a live window.
    let monitor = unsafe { MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        evaluation.reason = "foreground monitor unavailable";
        return evaluation;
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a correctly sized MONITORINFO.
    if unsafe { !GetMonitorInfoW(monitor, &mut info).as_bool() } {
        evaluation.reason = "foreground monitor info unavailable";
        return evaluation;
    }
    let monitor = rect_from(&info.rcMonitor);
    evaluation.monitor = Some(monitor);

    if !geom::rect_covers_monitor(&bounds, &monitor) {
        evaluation.reason = "visible bounds do not cover monitor";
        return evaluation;
    }

    if !window_style_allows_fullscreen(fg) {
        evaluation.reason = "standard framed window";
        return evaluation;
    }

    evaluation.is_fullscreen = true;
    evaluation.reason = "visible bounds cover monitor";
    evaluation
}

/// The id of the process that created `hwnd`; `None` for a stale handle.
fn window_process_id(hwnd: HWND) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: `pid` outlives the call; a stale handle returns thread id 0.
    let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(std::ptr::from_mut(&mut pid))) };
    (thread != 0).then_some(pid)
}

/// The visible bounds of `hwnd`: the DWM extended frame (without the
/// invisible resize borders), else the window rect.
fn visible_window_bounds(hwnd: HWND) -> Option<(Rect, &'static str)> {
    let mut rect = RECT::default();
    // SAFETY: the pointer and size describe `rect`, which outlives the call.
    let dwm_ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            std::ptr::from_mut(&mut rect).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok()
    };
    if dwm_ok && !rect_is_empty(&rect) {
        return Some((rect_from(&rect), "dwm-extended-frame"));
    }

    // SAFETY: as above.
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return None;
    }
    if rect_is_empty(&rect) {
        None
    } else {
        Some((rect_from(&rect), "window-rect"))
    }
}

/// Whether DWM cloaks `hwnd` (a suspended UWP app or a window on another
/// virtual desktop).
fn is_dwm_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: the pointer and size describe `cloaked`, which outlives the call.
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            std::ptr::from_mut(&mut cloaked).cast(),
            std::mem::size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
    }
}

fn window_class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: the slice bounds the write.
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    match usize::try_from(len) {
        Ok(len) if len > 0 => String::from_utf16_lossy(&buf[..len]),
        _ => String::new(),
    }
}

fn is_ignored_fullscreen_class(class_name: &str) -> bool {
    IGNORED_FULLSCREEN_CLASSES
        .iter()
        .any(|ignored| class_name.eq_ignore_ascii_case(ignored))
}

fn window_style_allows_fullscreen(hwnd: HWND) -> bool {
    // SAFETY: a zero style is only an error when the last error is set,
    // so it is cleared first.
    unsafe { SetLastError(WIN32_ERROR(0)) };
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
    if style == 0 && unsafe { GetLastError() } != WIN32_ERROR(0) {
        return false;
    }
    // The style is a 32-bit value in a pointer-sized return.
    geom::window_style_bits_allow_fullscreen(style as u32)
}

fn rect_from(rect: &RECT) -> Rect {
    Rect::new(rect.left, rect.top, rect.right, rect.bottom)
}

fn rect_is_empty(rect: &RECT) -> bool {
    rect.right <= rect.left || rect.bottom <= rect.top
}

fn format_rect(rect: Option<&Rect>) -> String {
    match rect {
        Some(r) => format!("({},{} {}x{})", r.l, r.t, r.r - r.l, r.b - r.t),
        None => String::from("<none>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_classes_are_ignored_case_insensitively() {
        assert!(is_ignored_fullscreen_class("Progman"));
        assert!(is_ignored_fullscreen_class("workerw"));
        assert!(is_ignored_fullscreen_class("SHELL_TRAYWND"));
        assert!(!is_ignored_fullscreen_class("Chrome_WidgetWin_1"));
        assert!(!is_ignored_fullscreen_class(""));
    }

    #[test]
    fn a_null_window_has_no_process() {
        assert_eq!(window_process_id(HWND::default()), None);
    }

    #[test]
    fn run_value_path_is_quoted() {
        assert_eq!(
            quoted_path(Path::new(r"D:\My Tools\usage-monitor.exe")),
            r#""D:\My Tools\usage-monitor.exe""#
        );
    }
}
