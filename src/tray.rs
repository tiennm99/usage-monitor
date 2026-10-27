//! One tray icon per enabled live provider, with a usage badge, a
//! four-line tooltip and the context menu. A left click toggles the widget.
//!
//! [`Trays::sync`] runs on the main thread with owned [`TrayView`]s; the
//! state lock is never held while a tray is touched.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use tauri::image::Image;
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::badge;
use crate::core::i18n::LocaleStrings;
use crate::core::menu_model::MenuEntry;
use crate::core::state::{tray_tooltip, UsageState};
use crate::menu;
use crate::runtime::Runtime;
use crate::usage::ProviderId;

/// The tooltip limit of the Windows notification area (`szTip` holds 128
/// UTF-16 units including the terminator).
pub const TOOLTIP_MAX_UTF16: usize = 127;

/// What one provider's tray icon shows.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayView {
    pub provider: ProviderId,
    /// Session usage, or `None` (track only) until a poll succeeded.
    pub percent: Option<f64>,
    pub tooltip: String,
}

/// The tray icons for the enabled live providers, in display order. The
/// badge shows usage only while the last poll succeeded, as before.
pub fn views(enabled: &[ProviderId], usage: &UsageState, strings: &LocaleStrings) -> Vec<TrayView> {
    enabled
        .iter()
        .map(|&provider| {
            let entry = usage.snapshots.get(&provider);
            TrayView {
                provider,
                percent: if usage.last_poll_ok {
                    entry.map(|e| e.windows.primary.utilization)
                } else {
                    None
                },
                tooltip: truncate_utf16(
                    &tray_tooltip(strings.provider_label(provider), entry, strings),
                    TOOLTIP_MAX_UTF16,
                ),
            }
        })
        .collect()
}

/// Cut `text` to at most `max` UTF-16 units without splitting a character.
pub fn truncate_utf16(text: &str, max: usize) -> String {
    let mut units = 0;
    let mut end = 0;
    for (i, c) in text.char_indices() {
        units += c.len_utf16();
        if units > max {
            break;
        }
        end = i + c.len_utf8();
    }
    text[..end].to_string()
}

/// What an icon currently shows, so unchanged icons and tooltips are not
/// pushed again.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    /// Percent bits and fill colour of the drawn badge.
    badge: (Option<u64>, [u8; 3]),
    tooltip: String,
}

impl Shown {
    fn of(view: &TrayView) -> Self {
        let colour = view
            .percent
            .map(|p| {
                let c = crate::core::usage_color::bar_fill_color(view.provider, true, p);
                [c.r, c.g, c.b]
            })
            .unwrap_or_default();
        Self {
            badge: (view.percent.map(f64::to_bits), colour),
            tooltip: view.tooltip.clone(),
        }
    }
}

/// The diff between the icons that exist and the ones wanted.
#[derive(Debug, Default, PartialEq)]
struct Plan {
    remove: Vec<ProviderId>,
    create: Vec<ProviderId>,
    /// Existing icons with `(new badge?, new tooltip?)`.
    update: Vec<(ProviderId, bool, bool)>,
}

fn plan(shown: &HashMap<ProviderId, Shown>, views: &[TrayView]) -> Plan {
    let mut plan = Plan {
        remove: ProviderId::ALL
            .into_iter()
            .filter(|p| shown.contains_key(p) && !views.iter().any(|v| v.provider == *p))
            .collect(),
        ..Plan::default()
    };
    for view in views {
        let next = Shown::of(view);
        match shown.get(&view.provider) {
            None => plan.create.push(view.provider),
            Some(current) => {
                let badge = current.badge != next.badge;
                let tooltip = current.tooltip != next.tooltip;
                if badge || tooltip {
                    plan.update.push((view.provider, badge, tooltip));
                }
            }
        }
    }
    plan
}

/// The tray icons this process created and what they show.
#[derive(Debug, Default)]
pub struct Trays {
    shown: Mutex<HashMap<ProviderId, Shown>>,
}

impl Trays {
    fn shown(&self) -> HashMap<ProviderId, Shown> {
        self.shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn record(&self, provider: ProviderId, shown: Option<Shown>) {
        let mut map = self.shown.lock().unwrap_or_else(PoisonError::into_inner);
        match shown {
            Some(s) => map.insert(provider, s),
            None => map.remove(&provider),
        };
    }

    /// The providers that currently have an icon.
    pub fn providers(&self) -> Vec<ProviderId> {
        let shown = self.shown();
        ProviderId::ALL
            .into_iter()
            .filter(|p| shown.contains_key(p))
            .collect()
    }

    /// Bring the icons in line with `views`: remove the ones no longer
    /// wanted, create new ones with a menu from `entries`, and push a badge
    /// or tooltip only when it changed. Main thread only. On Linux nothing
    /// happens without an AppIndicator library.
    pub fn sync(&self, app: &AppHandle, views: &[TrayView], entries: &[MenuEntry]) {
        if !tray_supported() {
            return;
        }
        let plan = plan(&self.shown(), views);
        for provider in plan.remove {
            app.remove_tray_by_id(provider.slug());
            self.record(provider, None);
        }
        for view in views {
            if plan.create.contains(&view.provider) {
                match create(app, view, entries) {
                    Ok(_) => self.record(view.provider, Some(Shown::of(view))),
                    Err(err) => log::error!(
                        "tray icon for {} could not be created: {err}",
                        view.provider.slug()
                    ),
                }
            }
        }
        for (provider, badge_changed, tooltip_changed) in plan.update {
            let Some(view) = views.iter().find(|v| v.provider == provider) else {
                continue;
            };
            let Some(tray) = app.tray_by_id(provider.slug()) else {
                self.record(provider, None);
                continue;
            };
            let mut ok = true;
            if badge_changed {
                if let Err(err) = tray.set_icon(Some(badge_image(view))) {
                    log::warn!("tray icon of {} not updated: {err}", provider.slug());
                    ok = false;
                }
            }
            if tooltip_changed {
                if let Err(err) = tray.set_tooltip(Some(&view.tooltip)) {
                    log::warn!("tray tooltip of {} not updated: {err}", provider.slug());
                    ok = false;
                }
            }
            if ok {
                self.record(provider, Some(Shown::of(view)));
            }
        }
    }

    /// Give every icon a fresh menu built from `entries`. Main thread only.
    pub fn set_menus(&self, app: &AppHandle, entries: &[MenuEntry]) {
        for provider in self.providers() {
            let Some(tray) = app.tray_by_id(provider.slug()) else {
                continue;
            };
            let result = menu::build(app, entries).and_then(|m| tray.set_menu(Some(m)));
            if let Err(err) = result {
                log::warn!("tray menu of {} not updated: {err}", provider.slug());
            }
        }
    }
}

/// Whether a tray can be created on this system without aborting.
fn tray_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        crate::platform::linux::appindicator_available()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

fn badge_image(view: &TrayView) -> Image<'static> {
    let pixmap = badge::render_pixmap(view.provider, view.percent);
    Image::new_owned(
        badge::demultiplied_rgba(&pixmap),
        badge::BADGE_PX,
        badge::BADGE_PX,
    )
}

fn create(app: &AppHandle, view: &TrayView, entries: &[MenuEntry]) -> tauri::Result<TrayIcon<Wry>> {
    TrayIconBuilder::with_id(view.provider.slug())
        .icon(badge_image(view))
        .tooltip(&view.tooltip)
        .menu(&menu::build(app, entries)?)
        .show_menu_on_left_click(false)
        .build(app)
}

/// App-level tray click handler, registered on the Tauri builder rather than
/// per icon. Tauri calls a per-icon handler while holding the per-icon
/// listener table, and the refresh below runs inline on the main thread and
/// may build a missing tray icon, which locks that same table to register
/// itself: a same-thread deadlock. The app-level table is never touched by
/// icon creation.
pub fn on_tray_icon_event(app: &AppHandle, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        if let Some(runtime) = app.try_state::<Arc<Runtime>>() {
            let runtime = Arc::clone(&runtime);
            runtime.toggle_widget();
            runtime.refresh_shell(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::i18n::I18n;
    use crate::core::state::ProviderUiState;
    use crate::usage::{UsageWindows, Window};

    fn strings() -> LocaleStrings {
        I18n::load().strings().clone()
    }

    fn usage(pct: f64, ok: bool) -> UsageState {
        let mut usage = UsageState {
            last_poll_ok: ok,
            ..UsageState::default()
        };
        usage.snapshots.insert(
            ProviderId::Claude,
            ProviderUiState {
                windows: UsageWindows {
                    primary: Window {
                        utilization: pct,
                        resets_at: None,
                    },
                    secondary: Window::default(),
                },
                primary_text: format!("{pct}%"),
                secondary_text: String::new(),
            },
        );
        usage
    }

    fn view(provider: ProviderId, percent: Option<f64>, tooltip: &str) -> TrayView {
        TrayView {
            provider,
            percent,
            tooltip: tooltip.into(),
        }
    }

    #[test]
    fn views_follow_enabled_providers_and_last_poll() {
        let s = strings();
        let v = views(&ProviderId::ALL, &usage(42.0, true), &s);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].provider, ProviderId::Claude);
        assert_eq!(v[0].percent, Some(42.0));
        assert_eq!(
            v[0].tooltip,
            "Claude Code\n5h: 42%\n7d: ...\nLeft-click: show/hide"
        );
        assert_eq!(v[1].provider, ProviderId::ChatGpt);
        assert_eq!(v[1].percent, None);
        // A failed last poll greys every badge to the track.
        let v = views(&ProviderId::ALL, &usage(42.0, false), &s);
        assert_eq!(v[0].percent, None);
        assert!(views(&[ProviderId::ChatGpt], &usage(1.0, true), &s)
            .iter()
            .all(|v| v.provider == ProviderId::ChatGpt));
    }

    #[test]
    fn tooltips_are_cut_to_the_utf16_limit() {
        assert_eq!(truncate_utf16("abc", 127), "abc");
        assert_eq!(truncate_utf16("abcdef", 3), "abc");
        // '·' is one unit, '😀' two: never split a surrogate pair.
        assert_eq!(truncate_utf16("a·😀b", 3), "a·");
        assert_eq!(truncate_utf16("a·😀b", 4), "a·😀");
        let long = "x".repeat(200);
        assert_eq!(truncate_utf16(&long, TOOLTIP_MAX_UTF16).len(), 127);
    }

    #[test]
    fn plan_creates_removes_and_updates_only_changes() {
        let mut shown = HashMap::new();
        let first = [view(ProviderId::Claude, None, "a")];
        let p = plan(&shown, &first);
        assert_eq!(p.create, [ProviderId::Claude]);
        assert!(p.remove.is_empty() && p.update.is_empty());

        shown.insert(ProviderId::Claude, Shown::of(&first[0]));
        assert_eq!(plan(&shown, &first), Plan::default(), "nothing changed");

        let p = plan(&shown, &[view(ProviderId::Claude, None, "b")]);
        assert_eq!(p.update, [(ProviderId::Claude, false, true)]);
        let p = plan(&shown, &[view(ProviderId::Claude, Some(10.0), "a")]);
        assert_eq!(p.update, [(ProviderId::Claude, true, false)]);

        let p = plan(&shown, &[view(ProviderId::ChatGpt, None, "c")]);
        assert_eq!(p.remove, [ProviderId::Claude]);
        assert_eq!(p.create, [ProviderId::ChatGpt]);
    }

    #[test]
    fn badge_key_tracks_percent_and_colour() {
        let a = Shown::of(&view(ProviderId::Claude, Some(59.0), "t"));
        let b = Shown::of(&view(ProviderId::Claude, Some(61.0), "t"));
        assert_ne!(a.badge, b.badge);
        assert_ne!(a.badge.1, b.badge.1, "crossing 60% changes the colour");
        let none = Shown::of(&view(ProviderId::Claude, None, "t"));
        assert_eq!(none.badge, (None, [0, 0, 0]));
    }
}
