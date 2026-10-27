//! App runtime: start-up, the shared state, the poll worker and the bridge
//! that pushes state to the webviews.
//!
//! Locking rule: no Tauri window, tray, menu or emit API is called while the
//! [`AppState`] mutex is held. Code reads state through
//! [`Runtime::with_state`], which returns owned data, and calls Tauri after
//! the guard is gone. Tauri calls from a worker block on the main thread, so
//! holding the lock across one can deadlock against a main-thread handler.

use std::error::Error;
use std::ffi::OsString;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, RunEvent};
use tauri_plugin_notification::NotificationExt;

use crate::cli::{self, CliArgs};
use crate::core::geom::{self, Rect, DEFAULT_BUBBLE_SIZE, MAX_BUBBLE_SIZE, MIN_BUBBLE_SIZE};
use crate::core::i18n::{I18n, LocaleStrings};
use crate::core::menu_model::MenuCommand;
use crate::core::payload::{self, BubblePayload, Canvas, PanelPayload};
use crate::core::settings::{self, Settings};
use crate::core::state::{Alert, PollOutcome, UpdateStatus, UsageState};
use crate::core::visibility::{self, Visibility};
use crate::menu::{self, MenuCache, MenuSnapshot};
use crate::scheduler::{self, Action, Message, SchedulerHost};
use crate::tray::{self, Trays};
use crate::update::{self, CheckOutcome};
use crate::usage::{self, refresh, ProviderId, Registry};
use crate::{bubble, diag, fullscreen, handoff, net, panel, platform};

/// Event carrying a [`BubblePayload`] to a bubble webview.
pub const BUBBLE_UPDATE_EVENT: &str = "bubble:update";

/// Event telling every webview the system theme changed.
pub const THEME_EVENT: &str = "theme";

/// Label prefix of every bubble window.
pub const BUBBLE_LABEL_PREFIX: &str = "bubble-";

/// How long a relaunched instance waits for its parent to exit. The
/// single-instance plugin cannot retry, so this must outlast a slow
/// WebView2 teardown.
const WAIT_PID_TIMEOUT: Duration = Duration::from_secs(15);

/// Per-provider budget for the CLI credential refresh.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(8);

/// Name of the start-at-login entry: the Windows `Run` value name of every
/// earlier release, so an existing entry is already the plugin's entry.
pub const AUTOSTART_NAME: &str = "UsageMonitor";

#[cfg(windows)]
const WEBVIEW2_MISSING_TITLE: &str = "Usage Monitor";
#[cfg(windows)]
const WEBVIEW2_MISSING_TEXT: &str = "Usage Monitor needs the Microsoft Edge \
WebView2 Runtime.\n\nDownload the Evergreen runtime from:\n\
https://developer.microsoft.com/microsoft-edge/webview2/";

/// Everything the app knows, behind one mutex in [`Runtime`].
#[derive(Debug)]
pub struct AppState {
    pub settings: Settings,
    pub strings: LocaleStrings,
    pub is_dark: bool,
    /// Why the bubbles are hidden, if they are.
    pub visibility: Visibility,
    pub usage: UsageState,
    pub update_status: UpdateStatus,
    /// How this copy was installed, which decides whether an available
    /// update can be applied.
    pub update_channel: update::Channel,
    /// The "update applied" alert from `--updated-to`, shown once the
    /// notification backend is up.
    pub pending_update_alert: Option<Alert>,
}

/// Holds the `AppHandle`; every push to a webview goes through it.
pub struct UiBridge {
    app: AppHandle,
}

impl UiBridge {
    pub fn app(&self) -> &AppHandle {
        &self.app
    }

    /// Send each bubble its payload. The window lookups and emits run on the
    /// main thread; `snapshot` is owned, so no state lock is involved.
    fn emit_bubble_updates(&self, snapshot: BubbleSnapshot) {
        let app = self.app.clone();
        let dispatched = self.app.run_on_main_thread(move || {
            let now = SystemTime::now();
            for provider in &snapshot.providers {
                let label = bubble_label(*provider);
                let Some(window) = app.get_webview_window(&label) else {
                    continue;
                };
                let scale = match window.scale_factor() {
                    Ok(scale) => scale,
                    Err(err) => {
                        log::warn!("scale factor of {label} unavailable: {err}");
                        continue;
                    }
                };
                let payload = snapshot.payload(*provider, scale, now);
                if let Err(err) = app.emit_to(label.as_str(), BUBBLE_UPDATE_EVENT, payload) {
                    log::warn!("emit {BUBBLE_UPDATE_EVENT} to {label} failed: {err}");
                }
            }
        });
        if let Err(err) = dispatched {
            log::warn!("bubble update could not reach the main thread: {err}");
        }
    }
}

/// Payload of [`THEME_EVENT`].
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThemePayload {
    is_dark: bool,
}

/// An owned copy of what the bubble payloads need.
struct BubbleSnapshot {
    providers: Vec<ProviderId>,
    usage: UsageState,
    strings: LocaleStrings,
    is_dark: bool,
    size_logical: i32,
}

impl BubbleSnapshot {
    fn of(state: &AppState) -> Self {
        Self {
            providers: enabled_providers(&state.settings),
            usage: state.usage.clone(),
            strings: state.strings.clone(),
            is_dark: state.is_dark,
            size_logical: state.settings.bubble_size_logical,
        }
    }

    fn payload(&self, provider: ProviderId, scale_factor: f64, now: SystemTime) -> BubblePayload {
        payload::bubble_payload(
            provider,
            self.usage.snapshots.get(&provider),
            &self.strings,
            self.is_dark,
            bubble_canvas(self.size_logical, scale_factor),
            now,
        )
    }
}

/// Shared runtime, managed by Tauri as `Arc<Runtime>`.
pub struct Runtime {
    ui: UiBridge,
    state: Mutex<AppState>,
    http: net::Client,
    registry: Mutex<Registry>,
    /// At most one poll runs at a time; a Refresh spam or a timer firing
    /// during a slow poll is folded into one follow-up poll through
    /// `poll_requested` instead of stacking requests.
    poll_in_flight: AtomicBool,
    /// A poll was asked for while one was in flight.
    poll_requested: AtomicBool,
    /// The release the last check found, applied by the version menu item.
    /// Never locked together with `state`.
    pending_update: Mutex<Option<tauri_plugin_updater::Update>>,
    scheduler: Sender<Message>,
    trays: Trays,
    menu_cache: MenuCache,
    /// Set once the process is ending, so destroyed bubbles are not
    /// recreated during shutdown.
    exiting: AtomicBool,
}

impl Runtime {
    /// Run `f` on the state and return what it returns. Return owned data
    /// and call Tauri only after this returns (see the module docs).
    pub fn with_state<T>(&self, f: impl FnOnce(&mut AppState) -> T) -> T {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard)
    }

    pub fn ui(&self) -> &UiBridge {
        &self.ui
    }

    /// Hand a message to the scheduler thread.
    pub fn send(&self, message: Message) {
        if self.scheduler.send(message).is_err() {
            log::warn!("scheduler is not running; message dropped");
        }
    }

    /// The payload for one bubble at the given window scale factor.
    pub fn bubble_payload(&self, provider: ProviderId, scale_factor: f64) -> BubblePayload {
        let snapshot = self.with_state(|s| BubbleSnapshot::of(s));
        snapshot.payload(provider, scale_factor, SystemTime::now())
    }

    /// The details-panel payload for `provider`.
    pub fn panel_payload(&self, provider: ProviderId) -> PanelPayload {
        self.with_state(|s| panel_payload_of(s, provider))
    }

    /// Push the current state to every UI surface, then re-arm the
    /// countdown for the next visible change.
    pub fn propagate(&self) {
        let panel_provider = panel::shown_provider();
        let (snapshot, panel_payload, countdown) = self.with_state(|s| {
            (
                BubbleSnapshot::of(s),
                panel_provider.map(|p| panel_payload_of(s, p)),
                s.usage.countdown_delay(SystemTime::now()),
            )
        });
        self.ui.emit_bubble_updates(snapshot);
        if let Some(payload) = panel_payload {
            panel::refresh_if_visible(&self.ui.app, payload);
        }
        self.refresh_shell(false);
        self.send(Message::RecomputeCountdown(countdown));
    }

    /// Send every bubble its payload, sized for its current window.
    pub fn push_bubbles(&self) {
        let snapshot = self.with_state(|s| BubbleSnapshot::of(s));
        self.ui.emit_bubble_updates(snapshot);
    }

    /// Record the system theme. A change reaches every open webview at
    /// once and is followed by fresh payloads, whose fills depend on it.
    pub fn set_dark(&self, is_dark: bool) {
        let changed = self.with_state(|s| std::mem::replace(&mut s.is_dark, is_dark) != is_dark);
        if !changed {
            return;
        }
        log::info!("theme is now {}", if is_dark { "dark" } else { "light" });
        if let Err(err) = self.ui.app.emit(THEME_EVENT, ThemePayload { is_dark }) {
            log::warn!("emit {THEME_EVENT} failed: {err}");
        }
        self.propagate();
    }

    /// Bring the tray icons in line with state and rebuild their menus when
    /// the menu tree changed, or always with `force_menu` (after a menu
    /// click, whose native check mark may now be wrong). Runs on the main
    /// thread and snapshots the state there, so a refresh queued by a worker
    /// can never apply a view older than one a menu click applied inline.
    pub fn refresh_shell(&self, force_menu: bool) {
        let app = self.ui.app.clone();
        let dispatched = self.ui.app.run_on_main_thread(move || {
            let Some(runtime) = app.try_state::<Arc<Runtime>>() else {
                return;
            };
            let (views, menu_snapshot) = runtime.with_state(|s| {
                (
                    tray::views(&enabled_providers(&s.settings), &s.usage, &s.strings),
                    MenuSnapshot::of(s),
                )
            });
            let entries = menu_snapshot.entries(menu::autostart_enabled(&app));
            runtime.trays.sync(&app, &views, &entries);
            if runtime.menu_cache.update(&entries, force_menu) {
                runtime.trays.set_menus(&app, &entries);
            }
        });
        if let Err(err) = dispatched {
            log::warn!("tray refresh could not reach the main thread: {err}");
        }
    }

    /// Start a poll on a worker thread. While one is running the request is
    /// queued instead, and the worker polls once more when it is done, so a
    /// provider enabled mid-poll still gets its data.
    ///
    /// The flags use `SeqCst`: the requester stores `poll_requested` then
    /// loads `poll_in_flight`, while the worker stores `poll_in_flight` then
    /// swaps `poll_requested`, so at least one side always sees the other.
    pub fn spawn_poll(self: &Arc<Self>) {
        while self
            .poll_in_flight
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            self.poll_requested.store(true, Ordering::SeqCst);
            if self.poll_in_flight.load(Ordering::SeqCst) {
                log::debug!("poll already in flight; queued another");
                return;
            }
            // The worker finished between the failed claim and the queued
            // request, so nobody will pick the request up: claim it again.
        }
        // The poll started here covers any request still queued.
        self.poll_requested.store(false, Ordering::SeqCst);
        let runtime = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("poll".into())
            .spawn(move || runtime.run_polls());
        if let Err(err) = spawned {
            log::error!("poll thread could not start: {err}");
            self.poll_in_flight.store(false, Ordering::SeqCst);
        }
    }

    /// Body of the poll worker: poll until no request is queued, then hand
    /// the in-flight flag back.
    fn run_polls(self: &Arc<Self>) {
        loop {
            self.poll_once();
            self.propagate();
            if !self.poll_requested.swap(false, Ordering::SeqCst) {
                break;
            }
        }
        self.poll_in_flight.store(false, Ordering::SeqCst);
        // A request that came after the last check but before the flag was
        // cleared was only queued; start it now.
        if self.poll_requested.swap(false, Ordering::SeqCst) {
            self.spawn_poll();
        }
    }

    /// Poll every enabled provider, apply the results, then run the CLI
    /// credential refresh for the providers whose credentials were rejected.
    /// Blocking; runs on the poll worker.
    fn poll_once(&self) {
        let settings = self.with_state(|s| s.settings.clone());
        let results = self
            .registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .poll_enabled(&self.http, &settings);
        let outcomes = results
            .into_iter()
            .map(|(provider, result)| (provider, poll_outcome(provider, result)))
            .collect();
        let applied = self.with_state(|s| {
            s.usage
                .apply_results(outcomes, &s.strings, Instant::now(), SystemTime::now())
        });
        self.deliver_alerts(applied.alerts);
        if !applied.auth_failures.is_empty() {
            self.refresh_credentials(&applied.auth_failures);
        }
    }

    /// Run the CLI refresh for each provider in order. The session-expired
    /// alert names the first provider that did not refresh; the next poll
    /// picks the new credentials up, there is no immediate re-poll.
    fn refresh_credentials(&self, failures: &[ProviderId]) {
        let orchestrator = refresh::Orchestrator::new(REFRESH_TIMEOUT);
        let mut expired: Option<ProviderId> = None;
        for &provider in failures {
            let outcome = self
                .registry
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .try_refresh(provider, &orchestrator);
            log::info!("credential refresh for {}: {outcome:?}", provider.slug());
            if outcome != refresh::Outcome::Refreshed {
                expired.get_or_insert(provider);
            }
        }
        if let Some(provider) = expired {
            let alert =
                self.with_state(|s| s.usage.session_expired_alert(provider, Instant::now()));
            self.deliver_alerts(alert.into_iter().collect());
        }
    }

    /// Show alerts as desktop notifications. The cooldown was applied when
    /// they were raised; nothing here filters them.
    fn deliver_alerts(&self, alerts: Vec<Alert>) {
        if alerts.is_empty() {
            return;
        }
        let strings = self.with_state(|s| s.strings.clone());
        for alert in alerts {
            let (title, body) = alert.text(&strings);
            log::info!("alert: {title}: {body}");
            let shown = self
                .ui
                .app
                .notification()
                .builder()
                .title(&title)
                .body(&body)
                .show();
            if let Err(err) = shown {
                log::warn!("notification {title:?} could not be shown: {err}");
            }
        }
    }

    /// Show the "update applied" alert from `--updated-to` once, after the
    /// tray work queued before it. It ignores the alert cooldown.
    fn deliver_pending_update_alert(self: &Arc<Self>) {
        let Some(alert) = self.with_state(|s| s.pending_update_alert.take()) else {
            return;
        };
        let runtime = Arc::clone(self);
        let dispatched = self
            .ui
            .app
            .run_on_main_thread(move || runtime.deliver_alerts(vec![alert]));
        if let Err(err) = dispatched {
            log::warn!("update alert could not reach the main thread: {err}");
        }
    }

    /// Run one menu command. The caller rebuilds the menus afterwards.
    pub fn on_menu_command(self: &Arc<Self>, command: MenuCommand) {
        log::debug!("menu command {}", command.id());
        match command {
            MenuCommand::Refresh => self.spawn_poll(),
            MenuCommand::PollInterval(ms) => self.set_poll_interval(ms),
            MenuCommand::ToggleProvider(provider) => self.toggle_provider(provider),
            MenuCommand::Autostart => menu::toggle_autostart(&self.ui.app),
            MenuCommand::ResetPosition => self.reset_positions(),
            MenuCommand::VersionAction => self.version_action(),
            MenuCommand::UpdateInterval(secs) => self.set_update_interval(secs),
            MenuCommand::SizeSmaller => self.resize_bubbles(-geom::RESIZE_STEP_LOGICAL),
            MenuCommand::SizeLarger => self.resize_bubbles(geom::RESIZE_STEP_LOGICAL),
            MenuCommand::SizeReset => self.set_bubble_size(DEFAULT_BUBBLE_SIZE),
            MenuCommand::ToggleWidget => self.toggle_widget(),
            MenuCommand::ToggleClickThrough => self.toggle_click_through(),
            MenuCommand::Restart => self.restart(),
            MenuCommand::Exit => self.exit(),
        }
    }

    /// Change a setting under the lock, then save a copy outside it.
    fn update_settings<T>(&self, f: impl FnOnce(&mut Settings) -> T) -> T {
        let (result, snapshot) = self.with_state(|s| {
            let result = f(&mut s.settings);
            (result, s.settings.clone())
        });
        if let Err(err) = settings::save(&snapshot) {
            log::warn!("settings save failed: {err}");
        }
        result
    }

    fn set_poll_interval(&self, ms: u32) {
        self.update_settings(|s| s.poll_interval_ms = ms);
        self.send(Message::SetPollInterval(Duration::from_millis(u64::from(
            ms,
        ))));
    }

    fn set_update_interval(&self, interval_secs: Option<u64>) {
        let last_check_unix = self.update_settings(|s| {
            s.update_check_interval_secs = interval_secs;
            s.last_update_check_unix
        });
        self.send(Message::SetUpdateInterval {
            interval_secs,
            last_check_unix,
        });
    }

    /// Toggle a provider, keeping at least one enabled, then create or
    /// destroy its bubble, update the tray and poll.
    fn toggle_provider(self: &Arc<Self>, provider: ProviderId) {
        let enabled = self.update_settings(|s| s.toggle_provider(provider));
        if enabled {
            bubble::spawn_create(&self.ui.app, provider);
        } else if let Some(window) = self.ui.app.get_webview_window(&bubble_label(provider)) {
            if let Err(err) = window.destroy() {
                log::warn!(
                    "bubble for {} could not be destroyed: {err}",
                    provider.slug()
                );
            }
        }
        self.propagate();
        self.spawn_poll();
    }

    /// Show or hide every bubble and remember the choice.
    pub fn toggle_widget(&self) {
        let event = if self.with_state(|s| s.visibility.user_hidden) {
            visibility::Event::UserShow
        } else {
            visibility::Event::UserHide
        };
        self.apply_visibility(event);
    }

    /// Flip whether the bubbles let the mouse through, and remember it.
    fn toggle_click_through(&self) {
        let ignore = self.update_settings(|s| {
            s.click_through = !s.click_through;
            s.click_through
        });
        bubble::set_click_through(&self.ui.app, ignore);
    }

    /// Show the bubbles again and put them back on top, unless they are
    /// meant to be hidden.
    pub fn bring_back_bubbles(&self) {
        bubble::bring_back(&self.ui.app);
    }

    /// Apply a visibility event to every bubble. A user show or hide is
    /// also saved as `widget_visible`.
    pub fn apply_visibility(&self, event: visibility::Event) {
        let (action, saved) = self.with_state(|s| {
            let (next, action) = visibility::next(s.visibility, event);
            s.visibility = next;
            let user_event = matches!(
                event,
                visibility::Event::UserShow | visibility::Event::UserHide
            );
            let saved = user_event.then(|| {
                s.settings.widget_visible = !next.user_hidden;
                s.settings.clone()
            });
            (action, saved)
        });
        if let Some(snapshot) = saved {
            if let Err(err) = settings::save(&snapshot) {
                log::warn!("settings save failed: {err}");
            }
        }
        if let Some(action) = action {
            log::debug!("bubbles {action:?} after {event:?}");
            bubble::apply(&self.ui.app, action);
        }
    }

    /// Forget the saved positions, move the bubbles to their default spots,
    /// push the cached state and poll.
    fn reset_positions(self: &Arc<Self>) {
        self.update_settings(|s| s.bubble_positions.reset_all());
        bubble::move_all_to_defaults(&self.ui.app);
        self.propagate();
        self.spawn_poll();
    }

    /// Save where the user dropped a bubble.
    pub fn save_bubble_position(&self, provider: ProviderId, pos: (i32, i32)) {
        self.update_settings(|s| s.bubble_positions.set(provider, pos));
    }

    /// Grow or shrink the bubbles by `delta` logical px.
    pub fn resize_bubbles(self: &Arc<Self>, delta: i32) {
        let current = self.with_state(|s| s.settings.bubble_size_logical);
        self.set_bubble_size(current.saturating_add(delta));
    }

    /// Clamp and store the bubble size, then resize every bubble around its
    /// centre, kept inside its work area (no snap); a no-op when it does not
    /// change. A bubble with a saved position has it updated to where the
    /// resize put it; one still at its default spot stays unsaved.
    pub fn set_bubble_size(self: &Arc<Self>, size_logical: i32) {
        let size = size_logical.clamp(MIN_BUBBLE_SIZE, MAX_BUBBLE_SIZE);
        let changed = self.with_state(|s| s.settings.bubble_size_logical != size);
        if !changed {
            return;
        }
        self.update_settings(|s| s.bubble_size_logical = size);
        for (provider, pos) in bubble::resize_all(&self.ui.app, size) {
            let saved = self.with_state(|s| s.settings.bubble_positions.get(provider).is_some());
            if saved {
                self.save_bubble_position(provider, (pos.x, pos.y));
            }
        }
        self.propagate();
    }

    /// A left click on a bubble toggles the details panel for it.
    pub fn bubble_clicked(&self, provider: ProviderId) {
        log::debug!("bubble click on {}", provider.slug());
        panel::toggle(&self.ui.app, provider);
    }

    /// Re-format the countdown texts after time passed and push them out.
    fn refresh_countdowns(&self) {
        self.with_state(|s| s.usage.refresh_texts(&s.strings));
        self.propagate();
    }

    /// The version menu item: apply the update the last check found, or
    /// check again.
    fn version_action(self: &Arc<Self>) {
        let (status, can_apply) =
            self.with_state(|s| (s.update_status, s.update_channel.can_apply()));
        if status == UpdateStatus::Available && can_apply {
            self.apply_update();
        } else {
            self.check_for_updates();
        }
    }

    /// Run an update check on a worker thread, automatic or from the menu.
    /// The scheduler re-arms the automatic check once it reports back.
    fn check_for_updates(self: &Arc<Self>) {
        let (previous, interval_secs) = self.with_state(|s| {
            let previous = s.update_status;
            if !previous.is_busy() {
                s.update_status = UpdateStatus::Checking;
            }
            (previous, s.settings.update_check_interval_secs)
        });
        match previous {
            UpdateStatus::Checking => {
                // The running check re-arms the schedule when it finishes.
                log::debug!("update check already running");
                return;
            }
            UpdateStatus::Applying => {
                // The apply may fail and keep this process running, so the
                // automatic check must stay armed.
                log::debug!("update check skipped while an update is applied");
                // Not a failure: re-arm the full interval, as the apply
                // either relaunches or reports its own error.
                self.send(Message::UpdateCheckDone {
                    interval_secs,
                    succeeded: true,
                });
                return;
            }
            _ => {}
        }
        self.refresh_shell(false);
        let runtime = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || runtime.run_update_check());
        if let Err(err) = spawned {
            log::error!("update check thread could not start: {err}");
            self.with_state(|s| s.update_status = UpdateStatus::Failed);
            self.send(Message::UpdateCheckDone {
                interval_secs,
                succeeded: false,
            });
            self.refresh_shell(false);
        }
    }

    /// Blocking body of [`Self::check_for_updates`]. Only a check that got
    /// an answer is stamped as the last check; a failed one leaves the
    /// stamp alone and has the scheduler retry sooner.
    fn run_update_check(&self) {
        let proxy = update::proxy_url(net::env_proxy().as_ref(), self.http.system_proxy());
        let outcome = update::check(&self.ui.app, proxy);
        let succeeded = outcome.is_ok();
        let (status, found) = match outcome {
            Ok(CheckOutcome::UpToDate) => {
                log::info!("update check: up to date");
                (UpdateStatus::UpToDate, None)
            }
            Ok(CheckOutcome::Available(found)) => {
                log::info!("update check: {} available", found.version);
                (UpdateStatus::Available, Some(found))
            }
            Err(err) => {
                log::warn!("update check failed: {err}");
                (UpdateStatus::Failed, None)
            }
        };
        // Stored before the status, so "Update available" always has a
        // release behind it.
        *self
            .pending_update
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = found;
        let (snapshot, interval_secs) = self.with_state(|s| {
            s.update_status = status;
            let snapshot = succeeded.then(|| {
                s.settings.last_update_check_unix = Some(scheduler::unix_now());
                s.settings.clone()
            });
            (snapshot, s.settings.update_check_interval_secs)
        });
        if let Some(snapshot) = snapshot {
            if let Err(err) = settings::save(&snapshot) {
                log::warn!("settings save failed: {err}");
            }
        }
        self.send(Message::UpdateCheckDone {
            interval_secs,
            succeeded,
        });
        self.refresh_shell(false);
    }

    /// Download and install the release the last check found on a worker
    /// thread, then relaunch into it.
    fn apply_update(self: &Arc<Self>) {
        // Claim the apply in one step, so a check that started since the
        // click cannot run alongside it.
        let channel = self.with_state(|s| {
            (s.update_status == UpdateStatus::Available).then(|| {
                s.update_status = UpdateStatus::Applying;
                s.update_channel
            })
        });
        let Some(channel) = channel else {
            log::debug!("update apply skipped, no update is available any more");
            return;
        };
        let found = self
            .pending_update
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(found) = found else {
            // Nothing to apply after all; a fresh check sorts the state out.
            self.with_state(|s| s.update_status = UpdateStatus::Idle);
            self.check_for_updates();
            return;
        };
        self.refresh_shell(false);
        let runtime = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("update-apply".into())
            .spawn(move || runtime.run_update_apply(found, channel));
        if let Err(err) = spawned {
            log::error!("update apply thread could not start: {err}");
            self.update_failed(&update::Error::Io(err));
        }
    }

    /// Blocking body of [`Self::apply_update`].
    fn run_update_apply(&self, found: tauri_plugin_updater::Update, channel: update::Channel) {
        let installed = match update::apply(found, channel) {
            Ok(version) => version,
            Err(err) => {
                self.update_failed(&err);
                return;
            }
        };
        log::info!("update {installed} installed; relaunching");
        let snapshot = self.with_state(|s| s.settings.clone());
        if let Err(err) = settings::save(&snapshot) {
            log::warn!("settings save before the update relaunch failed: {err}");
        }
        let args = [
            OsString::from("--updated-to"),
            OsString::from(installed.to_string()),
        ];
        if let Err(err) = self.relaunch_and_exit(&args) {
            // The new binary is in place but not running; the next restart
            // or login starts it.
            self.update_failed(&update::Error::Relaunch(err));
        }
    }

    /// Record a failed apply: Failed status plus `update-error.log`.
    fn update_failed(&self, err: &update::Error) {
        log::error!("update apply failed: {err}");
        update::install::write_update_error(err);
        self.with_state(|s| s.update_status = UpdateStatus::Failed);
        self.refresh_shell(false);
    }

    /// Spawn the next instance with `--wait-pid` plus `extra_args`, then
    /// exit. On a failed spawn this instance keeps running.
    pub fn relaunch_and_exit(&self, extra_args: &[OsString]) -> io::Result<()> {
        handoff::relaunch(extra_args)?;
        self.exit();
        Ok(())
    }

    /// Save the settings and start a fresh instance.
    pub fn restart(&self) {
        let snapshot = self.with_state(|s| s.settings.clone());
        if let Err(err) = settings::save(&snapshot) {
            log::warn!("restart: settings save failed: {err}");
        }
        if let Err(err) = self.relaunch_and_exit(&[]) {
            log::error!("restart: relaunch failed, keeping this instance: {err}");
        }
    }

    /// Whether the process is shutting down.
    pub fn is_exiting(&self) -> bool {
        self.exiting.load(Ordering::Acquire)
    }

    /// End the process. The scheduler stops from the `Exit` run event.
    pub fn exit(&self) {
        self.exiting.store(true, Ordering::Release);
        self.ui.app.exit(0);
    }
}

impl SchedulerHost for Arc<Runtime> {
    fn run(&self, action: Action) {
        match action {
            Action::Poll => self.spawn_poll(),
            Action::Countdown => self.refresh_countdowns(),
            Action::CheckForUpdates => self.check_for_updates(),
        }
    }
}

/// Map a provider error onto the portable outcome, logging transient ones.
fn poll_outcome(
    provider: ProviderId,
    result: Result<usage::UsageWindows, usage::Error>,
) -> PollOutcome {
    match result {
        Ok(windows) => {
            log::info!(
                "poll {}: session {:.1}%, weekly {:.1}%",
                provider.slug(),
                windows.primary.utilization,
                windows.secondary.utilization
            );
            PollOutcome::Ok(windows)
        }
        Err(usage::Error::AuthRequired | usage::Error::TokenExpired) => {
            log::info!("poll {}: credentials rejected", provider.slug());
            PollOutcome::AuthFailed
        }
        Err(err) => {
            log::warn!("poll {} failed: {err}", provider.slug());
            PollOutcome::Failed
        }
    }
}

/// The details-panel payload for `provider` from the state.
fn panel_payload_of(state: &AppState, provider: ProviderId) -> PanelPayload {
    payload::panel_payload(
        provider,
        state.usage.snapshots.get(&provider),
        &state.strings,
        state.is_dark,
    )
}

/// Window label of a provider's bubble.
pub fn bubble_label(provider: ProviderId) -> String {
    format!("{BUBBLE_LABEL_PREFIX}{}", provider.slug())
}

/// Providers the user has enabled, in display order.
pub fn enabled_providers(settings: &Settings) -> Vec<ProviderId> {
    ProviderId::ALL
        .into_iter()
        .filter(|p| settings.is_provider_enabled(*p))
        .collect()
}

/// The DPI for a window or monitor scale factor; 96 for a bad factor.
pub fn dpi_for_scale(scale_factor: f64) -> u32 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        (scale_factor * f64::from(geom::BASE_DPI)).round() as u32
    } else {
        geom::BASE_DPI
    }
}

/// The bubble canvas in physical px for a window scale factor.
pub fn bubble_canvas(size_logical: i32, scale_factor: f64) -> Canvas {
    let dpi = dpi_for_scale(scale_factor);
    let (w, h) = geom::bubble_size_px(size_logical, dpi);
    Canvas {
        w: u32::try_from(w).unwrap_or(1),
        h: u32::try_from(h).unwrap_or(1),
        dpi,
    }
}

/// Whether to run GTK on X11 (Xwayland) under a Wayland session:
/// positioning, snapping and always-on-top only work there. An explicit
/// `GDK_BACKEND` wins, and without an X server (`DISPLAY` unset) GTK would
/// fail to start, so the native Wayland backend stays.
pub fn needs_x11_backend(
    wayland_display: Option<OsString>,
    gdk_backend: Option<OsString>,
    display: Option<OsString>,
) -> bool {
    let set = |v: &Option<OsString>| v.as_ref().is_some_and(|v| !v.is_empty());
    set(&wayland_display) && !set(&gdk_backend) && set(&display)
}

/// Process entry: everything that must happen before the Tauri builder,
/// then the builder itself.
pub fn run() {
    let cli = cli::from_env();
    match diag::init(cli.diagnose) {
        Ok(Some(path)) => log::info!(
            "startup args={:?} log_path={}",
            std::env::args_os().collect::<Vec<_>>(),
            path.display()
        ),
        Ok(None) => {}
        Err(err) => eprintln!("diagnostic log could not be opened: {err}"),
    }

    #[cfg(windows)]
    if !platform::windows::webview2_available() {
        log::error!("WebView2 runtime missing; exiting");
        platform::windows::message_box_error(WEBVIEW2_MISSING_TITLE, WEBVIEW2_MISSING_TEXT);
        std::process::exit(0);
    }

    if let Some(pid) = cli.wait_pid {
        log::info!("waiting up to {WAIT_PID_TIMEOUT:?} for pid {pid} to exit");
        let started = Instant::now();
        if platform::wait_pid(pid, WAIT_PID_TIMEOUT) {
            log::info!("pid {pid} gone after {:?}", started.elapsed());
        } else {
            log::warn!("pid {pid} still running after {WAIT_PID_TIMEOUT:?}; starting anyway");
        }
    }

    #[cfg(target_os = "linux")]
    if needs_x11_backend(
        std::env::var_os("WAYLAND_DISPLAY"),
        std::env::var_os("GDK_BACKEND"),
        std::env::var_os("DISPLAY"),
    ) {
        log::info!("Wayland session with Xwayland: using GDK_BACKEND=x11");
        // No other thread exists yet, so changing the environment is safe.
        std::env::set_var("GDK_BACKEND", "x11");
    }

    let builder = tauri::Builder::default()
        // First, so a second instance exits before anything else starts.
        .plugin(tauri_plugin_single_instance::init(|_app, _argv, _cwd| {}));
    // Windows and Linux write their own entries (see `menu::autostart_enabled`).
    #[cfg(not(any(windows, target_os = "linux")))]
    let builder = {
        let autostart = tauri_plugin_autostart::Builder::new().app_name(AUTOSTART_NAME);
        #[cfg(target_os = "macos")]
        let autostart =
            autostart.macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent);
        builder.plugin(autostart.build())
    };

    let app = builder
        .plugin(tauri_plugin_notification::init())
        // Driven from Rust only; no capability exposes it to the webviews.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            crate::commands::get_bubble_state,
            crate::commands::get_panel_state,
            crate::commands::get_strings,
            crate::commands::log_js,
            crate::commands::bubble_ready,
            crate::commands::bubble_click,
            crate::commands::bubble_drag_start,
            crate::commands::bubble_resize_step,
            crate::menu::show_context_menu,
        ])
        .on_window_event(|window, event| {
            bubble::on_window_event(window, event);
            panel::on_window_event(window, event);
        })
        .on_menu_event(menu::on_menu_event)
        .on_tray_icon_event(tray::on_tray_icon_event)
        .setup(move |app| setup(app, &cli))
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(err) => {
            log::error!("tauri builder failed: {err}");
            eprintln!("Usage Monitor failed to start: {err}");
            std::process::exit(1);
        }
    };

    app.run(|app, event| match event {
        // Destroying the last window must not end the process; only an
        // explicit `exit` (which carries an exit code) does.
        RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        RunEvent::ExitRequested { .. } => {
            if let Some(runtime) = app.try_state::<Arc<Runtime>>() {
                runtime.exiting.store(true, Ordering::Release);
            }
        }
        RunEvent::Exit => {
            if let Some(runtime) = app.try_state::<Arc<Runtime>>() {
                runtime.send(Message::Shutdown);
            }
        }
        _ => {}
    });
}

fn setup(app: &mut tauri::App, cli: &CliArgs) -> Result<(), Box<dyn Error>> {
    let mut settings = settings::load();
    settings::drop_offscreen_positions(&mut settings, &monitor_rects(app.handle()));
    let is_dark = platform::system_is_dark();
    let http = net::Client::new(net::USER_AGENT)?;

    let state = AppState {
        strings: I18n::load().strings().clone(),
        is_dark,
        visibility: Visibility::from_widget_visible(settings.widget_visible),
        usage: UsageState::default(),
        update_status: UpdateStatus::Idle,
        update_channel: update::current_channel(),
        pending_update_alert: cli
            .updated_to
            .clone()
            .map(|version| Alert::UpdateApplied { version }),
        settings,
    };
    let config = scheduler::Config {
        poll_interval: Duration::from_millis(u64::from(state.settings.poll_interval_ms)),
        update_interval_secs: state.settings.update_check_interval_secs,
        last_update_check_unix: state.settings.last_update_check_unix,
    };
    let providers = enabled_providers(&state.settings);

    let (tx, rx) = mpsc::channel();
    let runtime = Arc::new(Runtime {
        ui: UiBridge {
            app: app.handle().clone(),
        },
        state: Mutex::new(state),
        http,
        registry: Mutex::new(Registry::with_defaults()),
        poll_in_flight: AtomicBool::new(false),
        poll_requested: AtomicBool::new(false),
        pending_update: Mutex::new(None),
        scheduler: tx,
        trays: Trays::default(),
        menu_cache: MenuCache::default(),
        exiting: AtomicBool::new(false),
    });
    app.manage(Arc::clone(&runtime));

    match std::env::current_exe() {
        Ok(exe) => handoff::cleanup_stale_files(&exe, handoff::updates_dir().as_deref()),
        Err(err) => log::warn!("startup cleanup skipped, no executable path: {err}"),
    }

    // Toasts from a portable exe need the app id registered first.
    #[cfg(windows)]
    register_notification_identity(app.handle());

    // The thread is detached; it stops on `Message::Shutdown` at exit.
    scheduler::spawn(config, rx, Arc::clone(&runtime))?;
    runtime.send(Message::PollNow);

    // tao's theme change never reaches a window on Linux; follow the portal.
    #[cfg(target_os = "linux")]
    platform::linux_theme::spawn(app.handle());

    // Without the thread the bubbles just stay up over fullscreen apps.
    if let Err(err) = fullscreen::spawn(Arc::clone(&runtime)) {
        log::error!("fullscreen check thread could not start: {err}");
    }

    for provider in providers {
        bubble::create(app.handle(), provider)?;
    }
    // The app still works without the panel; a bubble click then only logs.
    if let Err(err) = panel::create(app.handle(), is_dark) {
        log::error!("details panel could not be created: {err}");
    }
    runtime.refresh_shell(true);
    runtime.deliver_pending_update_alert();
    Ok(())
}

/// Register the app id the notification plugin shows toasts under, with
/// the app name and an icon PNG under the local data dir.
#[cfg(windows)]
fn register_notification_identity(app: &AppHandle) {
    const ICON_PNG: &[u8] = include_bytes!("../icons/128x128.png");
    let Some(icon_path) =
        dirs::data_local_dir().map(|d| d.join(settings::APP_DIR_NAME).join("icon.png"))
    else {
        log::warn!("no local data dir; toasts may not show");
        return;
    };
    let config = app.config();
    let name = config.product_name.as_deref().unwrap_or("Usage Monitor");
    match platform::windows::register_aumid(&config.identifier, name, ICON_PNG, &icon_path) {
        Ok(()) => log::info!("notification app id {} registered", config.identifier),
        Err(err) => log::warn!("notification app id registration incomplete: {err}"),
    }
}

/// Monitor rects in physical px. Empty when they cannot be enumerated,
/// which keeps every saved position.
pub fn monitor_rects(app: &AppHandle) -> Vec<Rect> {
    match app.available_monitors() {
        Ok(monitors) => monitors
            .iter()
            .map(|m| {
                let pos = m.position();
                let size = m.size();
                Rect::from_xywh(
                    pos.x,
                    pos.y,
                    i32::try_from(size.width).unwrap_or(i32::MAX),
                    i32::try_from(size.height).unwrap_or(i32::MAX),
                )
            })
            .collect(),
        Err(err) => {
            log::warn!("monitors could not be enumerated: {err}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &str) -> Option<OsString> {
        Some(OsString::from(v))
    }

    #[test]
    fn x11_backend_only_under_wayland_with_xwayland_and_no_override() {
        assert!(needs_x11_backend(os("wayland-0"), None, os(":0")));
        assert!(needs_x11_backend(os("wayland-0"), os(""), os(":0")));
        assert!(!needs_x11_backend(os("wayland-0"), os("wayland"), os(":0")));
        assert!(!needs_x11_backend(None, None, os(":0")));
        assert!(!needs_x11_backend(os(""), None, os(":0")));
        // No X server: forcing x11 would stop GTK from starting.
        assert!(!needs_x11_backend(os("wayland-0"), None, None));
    }

    #[test]
    fn canvas_follows_the_scale_factor() {
        assert_eq!(
            bubble_canvas(200, 1.0),
            Canvas {
                w: 200,
                h: 66,
                dpi: 96
            }
        );
        assert_eq!(
            bubble_canvas(200, 1.5),
            Canvas {
                w: 300,
                h: 99,
                dpi: 144
            }
        );
        assert_eq!(bubble_canvas(200, 1.25).dpi, 120);
        assert_eq!(bubble_canvas(200, f64::NAN).dpi, 96);
        assert_eq!(bubble_canvas(200, 0.0).dpi, 96);
        assert_eq!(dpi_for_scale(1.75), 168);
        assert_eq!(dpi_for_scale(-1.0), 96);
    }

    #[test]
    fn enabled_providers_keep_display_order() {
        let mut settings = Settings {
            show_codex: true,
            ..Settings::default()
        };
        assert_eq!(
            enabled_providers(&settings),
            [ProviderId::Claude, ProviderId::ChatGpt]
        );
        settings.show_claude_code = false;
        assert_eq!(enabled_providers(&settings), [ProviderId::ChatGpt]);
    }

    #[test]
    fn bubble_labels_use_the_slug() {
        assert_eq!(bubble_label(ProviderId::ChatGpt), "bubble-chatgpt");
    }

    #[test]
    fn auth_errors_become_auth_failures() {
        assert!(matches!(
            poll_outcome(ProviderId::Claude, Err(usage::Error::TokenExpired)),
            PollOutcome::AuthFailed
        ));
        assert!(matches!(
            poll_outcome(ProviderId::Claude, Err(usage::Error::AuthRequired)),
            PollOutcome::AuthFailed
        ));
        assert!(matches!(
            poll_outcome(ProviderId::ChatGpt, Err(usage::Error::NoCredentials)),
            PollOutcome::Failed
        ));
    }

    #[test]
    fn snapshot_payload_uses_the_window_canvas() {
        let state = AppState {
            settings: Settings::default(),
            strings: I18n::load().strings().clone(),
            is_dark: true,
            visibility: Visibility::default(),
            usage: UsageState::default(),
            update_status: UpdateStatus::Idle,
            update_channel: update::Channel::Portable,
            pending_update_alert: None,
        };
        let snapshot = BubbleSnapshot::of(&state);
        assert_eq!(snapshot.providers, [ProviderId::Claude]);
        let p = snapshot.payload(ProviderId::Claude, 2.0, SystemTime::now());
        assert_eq!((p.canvas_w, p.canvas_h, p.dpi), (400, 132, 192));
        assert_eq!(p.head_label, "…");
    }
}
