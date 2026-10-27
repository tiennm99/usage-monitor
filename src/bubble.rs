//! Bubble windows: one transparent, undecorated, always-on-top webview per
//! enabled live provider, labelled `bubble-<slug>`.
//!
//! Rust owns the window: its exact physical size, its position, snapping
//! at the end of a drag, clamping into the work area, and showing it
//! without taking focus. The page (`ui/bubble.js`) only draws the payload
//! and reports clicks, drag starts, right-clicks and Ctrl+wheel.
//!
//! Window event handlers and the Windows hook hand their Tauri work to a
//! short-lived thread ([`defer`]), so it runs after the event that caused
//! it (after tao applied its own DPI resize, for one) and never inside a
//! window procedure. No function here holds the state lock across a Tauri
//! call.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
#[cfg(not(windows))]
use std::time::Instant;

use tauri::{
    AppHandle, Manager, Monitor, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, Window, WindowEvent,
};

use crate::core::geom::{self, Point, Rect, SnapInput};
use crate::core::visibility::Action;
use crate::platform;
use crate::runtime::{self, Runtime, BUBBLE_LABEL_PREFIX};
use crate::usage::ProviderId;

/// How long a new bubble waits for `bubble_ready` before it is shown
/// anyway; a hidden webview may throttle the page that would send it.
const READY_FALLBACK: Duration = Duration::from_millis(1500);

/// Quiet time after the last `Moved` event that counts as the end of a
/// drag where the OS reports no move-loop end.
#[cfg(not(windows))]
const MOVE_END_DEBOUNCE: Duration = Duration::from_millis(250);

const TITLE: &str = "Usage Monitor";

/// Work area used when no monitor can be enumerated.
const FALLBACK_SCREEN: Rect = Rect::new(0, 0, 1920, 1080);

/// A monitor in physical px.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Screen {
    bounds: Rect,
    work: Rect,
    dpi: u32,
}

impl Screen {
    fn of(monitor: &Monitor) -> Self {
        let pos = monitor.position();
        let size = monitor.size();
        let work = monitor.work_area();
        Self {
            bounds: Rect::from_xywh(pos.x, pos.y, clamp_u32(size.width), clamp_u32(size.height)),
            work: Rect::from_xywh(
                work.position.x,
                work.position.y,
                clamp_u32(work.size.width),
                clamp_u32(work.size.height),
            ),
            dpi: runtime::dpi_for_scale(monitor.scale_factor()),
        }
    }
}

const FALLBACK: Screen = Screen {
    bounds: FALLBACK_SCREEN,
    work: FALLBACK_SCREEN,
    dpi: geom::BASE_DPI,
};

fn clamp_u32(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// Where a new bubble goes: its top-left, its physical size and the DPI it
/// was sized for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Placement {
    pos: Point,
    size: (i32, i32),
    dpi: u32,
}

/// Index of the screen that holds most of `rect`; when none overlaps it,
/// the screen whose bounds are nearest to its centre.
fn screen_for(rect: Rect, screens: &[Screen]) -> Option<usize> {
    let overlap = |s: &Screen| -> i64 {
        let w = (rect.r.min(s.bounds.r) - rect.l.max(s.bounds.l)).max(0);
        let h = (rect.b.min(s.bounds.b) - rect.t.max(s.bounds.t)).max(0);
        i64::from(w) * i64::from(h)
    };
    let (cx, cy) = (
        i64::from(rect.l) + i64::from(rect.w()) / 2,
        i64::from(rect.t) + i64::from(rect.h()) / 2,
    );
    let distance = |s: &Screen| -> i64 {
        let dx = (i64::from(s.bounds.l) - cx)
            .max(cx - i64::from(s.bounds.r))
            .max(0);
        let dy = (i64::from(s.bounds.t) - cy)
            .max(cy - i64::from(s.bounds.b))
            .max(0);
        dx * dx + dy * dy
    };
    let best_overlap = screens
        .iter()
        .enumerate()
        .map(|(i, s)| (i, overlap(s)))
        .filter(|(_, area)| *area > 0)
        .max_by_key(|(i, area)| (*area, std::cmp::Reverse(*i)))
        .map(|(i, _)| i);
    best_overlap.or_else(|| {
        screens
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| distance(s))
            .map(|(i, _)| i)
    })
}

/// Place a new bubble: at its saved top-left on the screen it overlaps
/// most, otherwise at the default spot on the primary screen; sized for
/// that screen's DPI and clamped into its work area.
fn initial_placement(
    provider: ProviderId,
    size_logical: i32,
    saved: Option<(i32, i32)>,
    screens: &[Screen],
    primary: Screen,
) -> Placement {
    let (screen, pos) = match saved {
        Some((x, y)) => {
            let (w, h) = geom::bubble_size_px(size_logical, primary.dpi);
            let probe = Rect::from_xywh(x, y, w, h);
            let screen = screen_for(probe, screens).map_or(primary, |i| screens[i]);
            (screen, Point::new(x, y))
        }
        None => {
            let (w, h) = geom::bubble_size_px(size_logical, primary.dpi);
            (
                primary,
                geom::default_position(primary.work, w, h, provider),
            )
        }
    };
    let size = geom::bubble_size_px(size_logical, screen.dpi);
    let rect = Rect::from_xywh(pos.x, pos.y, size.0, size.1);
    Placement {
        pos: geom::clamp_into_work_area(rect, screen.work, provider),
        size,
        dpi: screen.dpi,
    }
}

fn runtime_of(app: &AppHandle) -> Option<Arc<Runtime>> {
    app.try_state::<Arc<Runtime>>().map(|r| Arc::clone(&r))
}

/// Run `f` on a short-lived thread (see the module docs).
fn defer(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(err) = std::thread::Builder::new().name(name.into()).spawn(f) {
        log::error!("{name} thread could not start: {err}");
    }
}

fn screens(app: &AppHandle) -> Vec<Screen> {
    match app.available_monitors() {
        Ok(monitors) => monitors.iter().map(Screen::of).collect(),
        Err(err) => {
            log::warn!("monitors could not be enumerated: {err}");
            Vec::new()
        }
    }
}

/// The primary screen, else the first one, else a 1920x1080 stand-in.
fn primary_screen(app: &AppHandle, screens: &[Screen]) -> Screen {
    primary_monitor_screen(app, screens).unwrap_or(FALLBACK)
}

/// The primary screen, else the first one; `None` without any monitor.
fn primary_monitor_screen(app: &AppHandle, screens: &[Screen]) -> Option<Screen> {
    match app.primary_monitor() {
        Ok(Some(monitor)) => Some(Screen::of(&monitor)),
        Ok(None) => screens.first().copied(),
        Err(err) => {
            log::warn!("primary monitor unavailable: {err}");
            screens.first().copied()
        }
    }
}

/// The screen a window is on (Win32 picks the nearest), else the primary,
/// else a 1920x1080 stand-in.
fn screen_of(window: &WebviewWindow) -> Screen {
    monitor_screen_of(window).unwrap_or(FALLBACK)
}

/// The screen a window is on, else the primary; `None` without any monitor.
fn monitor_screen_of(window: &WebviewWindow) -> Option<Screen> {
    match window.current_monitor() {
        Ok(Some(monitor)) => return Some(Screen::of(&monitor)),
        Ok(None) => {}
        Err(err) => log::warn!("monitor of {} unavailable: {err}", window.label()),
    }
    let app = window.app_handle();
    primary_monitor_screen(app, &screens(app))
}

/// Work area of the monitor `window` is on, else of the primary; `None`
/// when no monitor can be found.
pub(crate) fn work_area_of(window: &WebviewWindow) -> Option<Rect> {
    monitor_screen_of(window).map(|s| s.work)
}

fn outer_rect(window: &WebviewWindow) -> Option<Rect> {
    let pos = window.outer_position();
    let size = window.outer_size();
    match (pos, size) {
        (Ok(pos), Ok(size)) => Some(Rect::from_xywh(
            pos.x,
            pos.y,
            clamp_u32(size.width),
            clamp_u32(size.height),
        )),
        (Err(err), _) | (_, Err(err)) => {
            log::warn!("bounds of {} unavailable: {err}", window.label());
            None
        }
    }
}

fn dpi_of(window: &WebviewWindow) -> u32 {
    match window.scale_factor() {
        Ok(scale) => runtime::dpi_for_scale(scale),
        Err(err) => {
            log::warn!("scale factor of {} unavailable: {err}", window.label());
            geom::BASE_DPI
        }
    }
}

fn set_position(window: &WebviewWindow, pos: Point) {
    if let Err(err) = window.set_position(PhysicalPosition::new(pos.x, pos.y)) {
        log::warn!("{} not moved: {err}", window.label());
    }
}

fn set_size(window: &WebviewWindow, (w, h): (i32, i32)) {
    let size = PhysicalSize::new(w.max(1).unsigned_abs(), h.max(1).unsigned_abs());
    if let Err(err) = window.set_size(size) {
        log::warn!("{} not resized: {err}", window.label());
    }
}

/// Bubble windows that exist, with their provider, in display order.
fn windows(app: &AppHandle) -> Vec<(ProviderId, WebviewWindow)> {
    ProviderId::ALL
        .into_iter()
        .filter_map(|p| {
            app.get_webview_window(&runtime::bubble_label(p))
                .map(|w| (p, w))
        })
        .collect()
}

// ---------- Creation, ready and show ----------

/// Creation number of each bubble still waiting for `bubble_ready`. The
/// number keeps a fallback timer of a destroyed window from showing a new
/// window with the same label.
static PENDING_READY: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);
static CREATIONS: AtomicU64 = AtomicU64::new(0);

fn with_pending<T>(f: impl FnOnce(&mut HashMap<String, u64>) -> T) -> T {
    let mut guard = PENDING_READY.lock().unwrap_or_else(PoisonError::into_inner);
    f(guard.get_or_insert_with(HashMap::new))
}

/// Create a bubble from a short-lived thread. Building a webview window
/// inside a menu or window event handler deadlocks on Windows (WebView2),
/// while off the main thread the build is posted to the event loop. The
/// lock makes a toggle and a `Destroyed` recreate for the same provider
/// check and build one at a time, so only one of them creates the window.
/// The main thread never takes it.
pub fn spawn_create(app: &AppHandle, provider: ProviderId) {
    static CREATE: Mutex<()> = Mutex::new(());
    let app = app.clone();
    defer("bubble-create", move || {
        let _serial = CREATE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(err) = create(&app, provider) {
            log::error!("bubble for {} could not be created: {err}", provider.slug());
        }
    });
}

/// Create `provider`'s bubble, hidden, at its exact physical size and
/// position. Does nothing while a window with the label still exists; a
/// destroyed one is recreated from its `Destroyed` event. The page shows it
/// through `bubble_ready`, or [`READY_FALLBACK`] after creation.
pub fn create(app: &AppHandle, provider: ProviderId) -> tauri::Result<()> {
    let label = runtime::bubble_label(provider);
    if app.get_webview_window(&label).is_some() {
        log::debug!("{label} still exists; not creating it again");
        return Ok(());
    }
    let Some(runtime) = runtime_of(app) else {
        log::warn!("{label} not created: the runtime is not ready");
        return Ok(());
    };
    let (size_logical, saved, click_through) = runtime.with_state(|s| {
        (
            s.settings.bubble_size_logical,
            s.settings.bubble_positions.get(provider),
            s.settings.click_through,
        )
    });
    let all = screens(app);
    let primary = primary_screen(app, &all);
    let placement = initial_placement(provider, size_logical, saved, &all, primary);
    let scale = f64::from(placement.dpi) / f64::from(geom::BASE_DPI);

    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("bubble.html".into()))
        .title(TITLE)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .focused(false)
        .focusable(false)
        .visible(false)
        // Close to the final size; the exact physical size is set below.
        .inner_size(
            f64::from(placement.size.0) / scale,
            f64::from(placement.size.1) / scale,
        )
        .build()?;
    if platform::positioning_supported() {
        set_position(&window, placement.pos);
    }
    set_size(&window, placement.size);
    if click_through {
        set_ignore_cursor(&window, true);
    }
    log::info!(
        "{label} created at ({}, {}) size {}x{} dpi {}",
        placement.pos.x,
        placement.pos.y,
        placement.size.0,
        placement.size.1,
        placement.dpi
    );

    prepare_native(app, &window, provider);
    #[cfg(target_os = "linux")]
    if let Some(is_dark) = platform::linux::portal_prefers_dark() {
        runtime.set_dark(is_dark);
    }
    #[cfg(target_os = "macos")]
    match window.theme() {
        Ok(theme) => runtime.set_dark(theme != tauri::Theme::Light),
        Err(err) => log::debug!("theme of {label} unavailable: {err}"),
    }

    let creation = CREATIONS.fetch_add(1, Ordering::Relaxed);
    with_pending(|p| p.insert(label.clone(), creation));
    let app = app.clone();
    defer("bubble-ready-fallback", move || {
        std::thread::sleep(READY_FALLBACK);
        let still_pending = with_pending(|p| {
            if p.get(&label) == Some(&creation) {
                p.remove(&label);
                true
            } else {
                false
            }
        });
        if still_pending {
            log::warn!("{label} sent no bubble_ready; showing it anyway");
            show_if_visible(&app, &label);
        }
    });
    Ok(())
}

/// Windows: make the bubble a non-activating tool window and hook its
/// move-end, setting-change and display-change messages. Both must run on
/// the window's own (main) thread before the first show.
#[cfg(windows)]
fn prepare_native(app: &AppHandle, window: &WebviewWindow, provider: ProviderId) {
    let window = window.clone();
    let hook_app = app.clone();
    let queued = app.run_on_main_thread(move || {
        if let Err(err) = platform::windows::apply_no_activate(&window) {
            log::warn!("{} may take focus: {err}", window.label());
        }
        let hooked = platform::windows::install_bubble_hook(&window, move |event| {
            on_hook_event(&hook_app, provider, event);
        });
        if let Err(err) = hooked {
            log::warn!("{} has no move-end hook: {err}", window.label());
        }
    });
    if let Err(err) = queued {
        log::warn!("bubble set-up could not reach the main thread: {err}");
    }
}

#[cfg(not(windows))]
fn prepare_native(_app: &AppHandle, _window: &WebviewWindow, _provider: ProviderId) {}

#[cfg(windows)]
fn on_hook_event(app: &AppHandle, provider: ProviderId, event: platform::windows::BubbleHookEvent) {
    use platform::windows::BubbleHookEvent;
    let app = app.clone();
    defer("bubble-hook", move || match event {
        BubbleHookEvent::MoveEnd => move_end(&app, provider),
        BubbleHookEvent::SettingChange => {
            if let Some(runtime) = runtime_of(&app) {
                runtime.set_dark(platform::windows::is_dark());
            }
            reclamp(&app, provider);
        }
        BubbleHookEvent::DisplayChange => reclamp(&app, provider),
    });
}

/// The page applied its first state: show the bubble unless it is hidden.
pub fn mark_ready(app: &AppHandle, label: &str) -> Result<(), String> {
    let provider = label
        .strip_prefix(BUBBLE_LABEL_PREFIX)
        .and_then(ProviderId::from_slug)
        .ok_or_else(|| "not a bubble window".to_string())?;
    if app.get_webview_window(label).is_none() {
        return Err("no such bubble window".into());
    }
    with_pending(|p| p.remove(label));
    log::debug!("{} ready", provider.slug());
    show_if_visible(app, label);
    Ok(())
}

/// Show one bubble when the visibility state allows it.
fn show_if_visible(app: &AppHandle, label: &str) {
    let Some(runtime) = runtime_of(app) else {
        return;
    };
    if !runtime.with_state(|s| s.visibility.shows()) {
        return;
    }
    let app = app.clone();
    let label = label.to_string();
    let queued = app.clone().run_on_main_thread(move || {
        if let Some(window) = app.get_webview_window(&label) {
            show(&window);
        }
    });
    if let Err(err) = queued {
        log::warn!("bubble show could not reach the main thread: {err}");
    }
}

/// Show or hide every bubble. On Windows this uses the raw no-activate
/// show and plain hide, never Tauri's, for both directions.
pub fn apply(app: &AppHandle, action: Action) {
    let handle = app.clone();
    let queued = app.run_on_main_thread(move || {
        for (_, window) in windows(&handle) {
            match action {
                Action::Show => show(&window),
                Action::Hide => hide(&window),
            }
        }
    });
    if let Err(err) = queued {
        log::warn!("bubble visibility could not reach the main thread: {err}");
    }
}

/// Show every ready bubble again and put it back on top, unless the
/// bubbles are meant to be hidden. Another always-on-top window, or a shell
/// action that hid ours, can otherwise leave a bubble covered or hidden for
/// good. A bubble still waiting for its page is left to the ready handshake.
/// Visibility is read on the main thread, after any hide queued before.
pub fn bring_back(app: &AppHandle) {
    let handle = app.clone();
    let queued = app.run_on_main_thread(move || {
        let shows = runtime_of(&handle).is_some_and(|r| r.with_state(|s| s.visibility.shows()));
        if !shows {
            return;
        }
        for (_, window) in windows(&handle) {
            if with_pending(|p| p.contains_key(window.label())) {
                continue;
            }
            show(&window);
            raise(&window);
        }
    });
    if let Err(err) = queued {
        log::warn!("bubble bring-back could not reach the main thread: {err}");
    }
}

fn raise(window: &WebviewWindow) {
    #[cfg(windows)]
    let result = platform::windows::raise_topmost(window).map_err(|e| e.to_string());
    // Re-setting the same level is a no-op, so drop it and set it again.
    #[cfg(not(windows))]
    let result = window
        .set_always_on_top(false)
        .and_then(|()| window.set_always_on_top(true))
        .map_err(|e| e.to_string());
    if let Err(err) = result {
        log::warn!("{} not raised: {err}", window.label());
    }
}

/// Make every bubble ignore the mouse, or take it again.
pub fn set_click_through(app: &AppHandle, ignore: bool) {
    let handle = app.clone();
    let queued = app.run_on_main_thread(move || {
        for (_, window) in windows(&handle) {
            set_ignore_cursor(&window, ignore);
        }
    });
    if let Err(err) = queued {
        log::warn!("bubble click-through could not reach the main thread: {err}");
    }
}

fn set_ignore_cursor(window: &WebviewWindow, ignore: bool) {
    if let Err(err) = window.set_ignore_cursor_events(ignore) {
        log::warn!("{} click-through not set: {err}", window.label());
    }
}

fn show(window: &WebviewWindow) {
    #[cfg(windows)]
    let result = platform::windows::show_no_activate(window).map_err(|e| e.to_string());
    #[cfg(not(windows))]
    let result = window.show().map_err(|e| e.to_string());
    if let Err(err) = result {
        log::warn!("{} not shown: {err}", window.label());
    }
}

fn hide(window: &WebviewWindow) {
    #[cfg(windows)]
    let result = platform::windows::hide(window).map_err(|e| e.to_string());
    #[cfg(not(windows))]
    let result = window.hide().map_err(|e| e.to_string());
    if let Err(err) = result {
        log::warn!("{} not hidden: {err}", window.label());
    }
}

// ---------- Window events ----------

/// Handle a bubble's window events; other windows are ignored.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    let label = window.label();
    let Some(provider) = label
        .strip_prefix(BUBBLE_LABEL_PREFIX)
        .and_then(ProviderId::from_slug)
    else {
        return;
    };
    match event {
        // Bubbles ignore close requests (Alt+F4); they are only destroyed.
        WindowEvent::CloseRequested { api, .. } => api.prevent_close(),
        WindowEvent::Destroyed => on_destroyed(window, provider),
        WindowEvent::ScaleFactorChanged { .. } => {
            let app = window.app_handle().clone();
            defer("bubble-dpi", move || fit_to_dpi(&app, provider));
        }
        #[cfg(not(windows))]
        WindowEvent::Moved(_) => drag::moved(label),
        // Windows reads the theme from the registry on WM_SETTINGCHANGE;
        // tao's theme follows the apps setting, not the system one.
        #[cfg(target_os = "macos")]
        WindowEvent::ThemeChanged(theme) => {
            let is_dark = *theme != tauri::Theme::Light;
            let app = window.app_handle().clone();
            defer("bubble-theme", move || {
                if let Some(runtime) = runtime_of(&app) {
                    runtime.set_dark(is_dark);
                }
            });
        }
        // On Linux tao sends ThemeChanged with a placeholder window id that
        // tauri drops, so it never arrives here; `platform::linux_theme`
        // follows the portal's SettingChanged signal instead.
        _ => {}
    }
}

/// A bubble destroyed while its provider is enabled again (a fast off/on
/// toggle) is recreated once the old window is gone.
fn on_destroyed(window: &Window, provider: ProviderId) {
    with_pending(|p| p.remove(window.label()));
    #[cfg(not(windows))]
    drag::disarm(window.label());
    let Some(runtime) = runtime_of(window.app_handle()) else {
        return;
    };
    if runtime.is_exiting() {
        return;
    }
    // The old window is already out of the manager here, so the existence
    // check in `create` sees it gone.
    if runtime.with_state(|s| s.settings.is_provider_enabled(provider)) {
        spawn_create(window.app_handle(), provider);
    }
}

/// Size a bubble for its current DPI and send it a fresh payload.
fn fit_to_dpi(app: &AppHandle, provider: ProviderId) {
    let Some(window) = app.get_webview_window(&runtime::bubble_label(provider)) else {
        return;
    };
    let Some(runtime) = runtime_of(app) else {
        return;
    };
    let size_logical = runtime.with_state(|s| s.settings.bubble_size_logical);
    let wanted = geom::bubble_size_px(size_logical, dpi_of(&window));
    if outer_rect(&window).is_some_and(|r| (r.w(), r.h()) != wanted) {
        set_size(&window, wanted);
    }
    runtime.push_bubbles();
}

// ---------- Drag, snap and clamp ----------

/// A drag started on `provider`'s bubble. On Windows the move-loop hook
/// reports its end, so nothing is armed.
pub fn drag_start(app: &AppHandle, provider: ProviderId) {
    #[cfg(windows)]
    let _ = (app, provider);
    #[cfg(not(windows))]
    drag::arm(app, provider);
}

/// The user dropped `provider`'s bubble: snap it, then save where it ended.
/// Programmatic moves never get here, so they neither snap nor save.
fn move_end(app: &AppHandle, provider: ProviderId) {
    if !platform::positioning_supported() {
        return;
    }
    let Some(window) = app.get_webview_window(&runtime::bubble_label(provider)) else {
        return;
    };
    let Some(rect) = outer_rect(&window) else {
        return;
    };
    let peers = windows(app)
        .into_iter()
        .filter(|(p, _)| *p != provider)
        .filter_map(|(_, w)| outer_rect(&w))
        .collect();
    #[cfg(windows)]
    let taskbar = platform::windows::taskbar();
    #[cfg(not(windows))]
    let taskbar = None;
    let snapped = geom::snap(&SnapInput {
        rect,
        work_area: screen_of(&window).work,
        dpi: dpi_of(&window),
        taskbar,
        peers,
    });
    if snapped != Point::new(rect.l, rect.t) {
        set_position(&window, snapped);
    }
    log::debug!(
        "{} dropped at ({}, {}), snapped to ({}, {})",
        provider.slug(),
        rect.l,
        rect.t,
        snapped.x,
        snapped.y
    );
    if let Some(runtime) = runtime_of(app) {
        runtime.save_bubble_position(provider, (snapped.x, snapped.y));
    }
}

/// Pull a bubble back into its monitor's work area, without saving. Runs
/// on the Windows setting and display change messages.
#[cfg(windows)]
fn reclamp(app: &AppHandle, provider: ProviderId) {
    if !platform::positioning_supported() {
        return;
    }
    let Some(window) = app.get_webview_window(&runtime::bubble_label(provider)) else {
        return;
    };
    let Some(rect) = outer_rect(&window) else {
        return;
    };
    let clamped = geom::clamp_into_work_area(rect, screen_of(&window).work, provider);
    if clamped != Point::new(rect.l, rect.t) {
        log::info!(
            "{} clamped from ({}, {}) to ({}, {})",
            provider.slug(),
            rect.l,
            rect.t,
            clamped.x,
            clamped.y
        );
        set_position(&window, clamped);
    }
}

/// Resize every bubble to `size_logical` around its own centre, at its own
/// DPI, kept inside the work area of the monitor it was on. Nothing snaps
/// and nothing is saved here; returns where each moved bubble now is, empty
/// where windows cannot be positioned.
pub fn resize_all(app: &AppHandle, size_logical: i32) -> Vec<(ProviderId, Point)> {
    let placeable = platform::positioning_supported();
    let mut moved = Vec::new();
    for (provider, window) in windows(app) {
        let Some(rect) = outer_rect(&window) else {
            continue;
        };
        // The monitor before the resize: a bubble grown past an edge may
        // overlap a neighbour more than its own monitor afterwards.
        let work = screen_of(&window).work;
        let (w, h) = geom::bubble_size_px(size_logical, dpi_of(&window));
        let (cx, cy) = ((rect.l + rect.r) / 2, (rect.t + rect.b) / 2);
        set_size(&window, (w, h));
        if placeable {
            let centred = Rect::from_xywh(cx - w / 2, cy - h / 2, w, h);
            let pos = geom::clamp_into_work_area(centred, work, provider);
            set_position(&window, pos);
            moved.push((provider, pos));
        }
    }
    moved
}

/// Move every bubble to its default spot on the primary screen. The
/// windows are moved, not recreated.
pub fn move_all_to_defaults(app: &AppHandle) {
    if !platform::positioning_supported() {
        return;
    }
    let primary = primary_screen(app, &screens(app));
    let Some(runtime) = runtime_of(app) else {
        return;
    };
    let size_logical = runtime.with_state(|s| s.settings.bubble_size_logical);
    for (provider, window) in windows(app) {
        let placement = reset_placement(provider, size_logical, primary);
        // Move first so a DPI-change resize from crossing monitors happens
        // before the explicit size, then re-place at the final size.
        set_position(&window, placement.pos);
        set_size(&window, placement.size);
        set_position(&window, placement.pos);
    }
}

/// Where Reset position puts a bubble: the default spot on the primary
/// screen, sized for the primary's DPI whatever screen the bubble is on now.
fn reset_placement(provider: ProviderId, size_logical: i32, primary: Screen) -> Placement {
    initial_placement(provider, size_logical, None, &[primary], primary)
}

/// Move-end detection where the OS reports no end of a window drag: a
/// [`MOVE_END_DEBOUNCE`] quiet period after the last `Moved` event, armed
/// only by a drag the page started. On X11 the period restarts while
/// Button1 is still held, so pausing mid-drag does not snap.
#[cfg(not(windows))]
mod drag {
    use super::*;

    /// Deadline of each armed bubble, by label.
    static ARMED: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

    fn with_armed<T>(f: impl FnOnce(&mut HashMap<String, Instant>) -> T) -> T {
        let mut guard = ARMED.lock().unwrap_or_else(PoisonError::into_inner);
        f(guard.get_or_insert_with(HashMap::new))
    }

    /// Arm the debounce and start its watcher, unless one already runs.
    pub fn arm(app: &AppHandle, provider: ProviderId) {
        let label = runtime::bubble_label(provider);
        let fresh = with_armed(|armed| {
            armed
                .insert(label.clone(), Instant::now() + MOVE_END_DEBOUNCE)
                .is_none()
        });
        if !fresh {
            return;
        }
        let app = app.clone();
        defer("bubble-drag", move || {
            if wait_for_end(&label) {
                move_end(&app, provider);
            }
        });
    }

    /// A move pushes the deadline out, while armed.
    pub fn moved(label: &str) {
        with_armed(|armed| {
            if let Some(deadline) = armed.get_mut(label) {
                *deadline = Instant::now() + MOVE_END_DEBOUNCE;
            }
        });
    }

    pub fn disarm(label: &str) {
        with_armed(|armed| armed.remove(label));
    }

    /// Sleep until the deadline stops moving and the button is up, then
    /// disarm. False when the bubble was disarmed meanwhile (destroyed).
    fn wait_for_end(label: &str) -> bool {
        loop {
            let Some(deadline) = with_armed(|armed| armed.get(label).copied()) else {
                return false;
            };
            let now = Instant::now();
            if now < deadline {
                std::thread::sleep(deadline - now);
                continue;
            }
            if platform::primary_button_down() {
                with_armed(|armed| armed.insert(label.to_string(), now + MOVE_END_DEBOUNCE));
                continue;
            }
            // Disarm before the snap moves the window, so its own `Moved`
            // cannot re-arm.
            with_armed(|armed| armed.remove(label));
            return true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(bounds: Rect, work: Rect, dpi: u32) -> Screen {
        Screen { bounds, work, dpi }
    }

    const PRIMARY: Screen = Screen {
        bounds: Rect::new(0, 0, 1920, 1080),
        work: Rect::new(0, 0, 1920, 1040),
        dpi: 96,
    };

    #[test]
    fn screen_for_prefers_the_largest_overlap() {
        let right = screen(
            Rect::new(1920, 0, 3840, 1080),
            Rect::new(1920, 0, 3840, 1040),
            144,
        );
        let screens = [PRIMARY, right];
        // 80 px on the primary, 120 px on the right one.
        assert_eq!(
            screen_for(Rect::from_xywh(1840, 100, 200, 66), &screens),
            Some(1)
        );
        assert_eq!(
            screen_for(Rect::from_xywh(1800, 100, 200, 66), &screens),
            Some(0)
        );
        // A tie keeps the earlier screen.
        assert_eq!(
            screen_for(Rect::from_xywh(1820, 100, 200, 66), &screens),
            Some(0)
        );
    }

    #[test]
    fn screen_for_falls_back_to_the_nearest_screen() {
        let right = screen(
            Rect::new(1920, 0, 3840, 1080),
            Rect::new(1920, 0, 3840, 1040),
            96,
        );
        let screens = [PRIMARY, right];
        assert_eq!(
            screen_for(Rect::from_xywh(5000, 100, 200, 66), &screens),
            Some(1)
        );
        assert_eq!(
            screen_for(Rect::from_xywh(-900, 2000, 200, 66), &screens),
            Some(0)
        );
        assert_eq!(screen_for(Rect::from_xywh(0, 0, 10, 10), &[]), None);
    }

    #[test]
    fn default_placement_is_bottom_right_of_the_primary_work_area() {
        let p = initial_placement(ProviderId::Claude, 200, None, &[PRIMARY], PRIMARY);
        assert_eq!(
            p,
            Placement {
                pos: Point::new(1920 - 200 - 24, 1040 - 66 - 24),
                size: (200, 66),
                dpi: 96,
            }
        );
        let codex = initial_placement(ProviderId::ChatGpt, 200, None, &[PRIMARY], PRIMARY);
        assert_eq!(codex.pos, Point::new(1696, 1040 - 66 - 24 - 90));
    }

    #[test]
    fn saved_placement_takes_the_dpi_of_its_screen_and_is_clamped() {
        let right = screen(
            Rect::new(1920, 0, 3840, 1080),
            Rect::new(1920, 0, 3840, 1040),
            144,
        );
        let screens = [PRIMARY, right];
        let p = initial_placement(
            ProviderId::Claude,
            200,
            Some((2500, 400)),
            &screens,
            PRIMARY,
        );
        assert_eq!(
            p,
            Placement {
                pos: Point::new(2500, 400),
                size: (300, 99),
                dpi: 144,
            }
        );
        // Partly below the work area: pulled back in.
        let low = initial_placement(
            ProviderId::Claude,
            200,
            Some((100, 1030)),
            &screens,
            PRIMARY,
        );
        assert_eq!(low.pos, Point::new(100, 1040 - 66));
    }

    #[test]
    fn saved_placement_on_no_screen_lands_on_the_nearest() {
        let p = initial_placement(
            ProviderId::Claude,
            200,
            Some((5000, 5000)),
            &[PRIMARY],
            PRIMARY,
        );
        assert_eq!(p.pos, Point::new(1720, 974));
        let codex = initial_placement(
            ProviderId::ChatGpt,
            200,
            Some((5000, 5000)),
            &[PRIMARY],
            PRIMARY,
        );
        assert_eq!(codex.pos, Point::new(1720, 974 - 66 - 24));
    }

    #[test]
    fn reset_placement_is_sized_for_the_primary_dpi() {
        let hidpi_primary = screen(
            Rect::new(0, 0, 2880, 1620),
            Rect::new(0, 0, 2880, 1560),
            144,
        );
        let claude = reset_placement(ProviderId::Claude, 200, hidpi_primary);
        assert_eq!(
            claude,
            Placement {
                pos: Point::new(2880 - 300 - 24, 1560 - 99 - 24),
                size: (300, 99),
                dpi: 144,
            }
        );
        let codex = reset_placement(ProviderId::ChatGpt, 200, hidpi_primary);
        assert_eq!(codex.size, (300, 99));
        assert_eq!(codex.pos, Point::new(2556, 1560 - 99 - 24 - 99 - 24));
        // Fully inside the work area and clear of the Claude bubble.
        assert!(claude.pos.x + claude.size.0 <= 2880);
        assert!(codex.pos.y + codex.size.1 < claude.pos.y);

        let low_dpi = reset_placement(ProviderId::Claude, 200, PRIMARY);
        assert_eq!(low_dpi.size, (200, 66));
        assert_eq!(low_dpi.pos, Point::new(1920 - 200 - 24, 1040 - 66 - 24));
    }

    #[test]
    fn fallback_screen_is_a_plain_1080p_monitor() {
        assert_eq!(FALLBACK.work, Rect::new(0, 0, 1920, 1080));
        assert_eq!(FALLBACK.dpi, 96);
    }
}
