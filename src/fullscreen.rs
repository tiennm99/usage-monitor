//! Fullscreen auto-hide: a background thread checks every 1.5 s whether a
//! fullscreen window of another process is in the foreground, and hides the
//! bubbles while one is. They come back without activation when it leaves,
//! unless the user hid them in the meantime.
//!
//! The same thread brings the bubbles back every minute while they should
//! show, in case another window covered or hid them.

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::core::visibility::Event;
use crate::platform::{self, FullscreenCheck};
use crate::runtime::Runtime;

/// How often the foreground window is checked.
pub const CHECK_INTERVAL: Duration = Duration::from_millis(1500);

/// How often shown bubbles are brought back on top.
pub const BRING_BACK_INTERVAL: Duration = Duration::from_secs(60);

/// Start the check thread. It runs until the process starts exiting.
pub fn spawn(runtime: Arc<Runtime>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("fullscreen".into())
        .spawn(move || {
            let mut last_bring_back = Instant::now();
            while !runtime.is_exiting() {
                tick(&runtime);
                if last_bring_back.elapsed() >= BRING_BACK_INTERVAL {
                    last_bring_back = Instant::now();
                    runtime.bring_back_bubbles();
                }
                std::thread::sleep(CHECK_INTERVAL);
            }
        })
        .map(drop)
}

/// One check: read the foreground state and, when it differs from what the
/// bubbles reflect, apply the matching visibility event. The state is read
/// and released before any window call (`apply_visibility` does the same).
fn tick(runtime: &Runtime) {
    let check = platform::foreground_is_fullscreen();
    let visibility = runtime.with_state(|s| s.visibility);
    let Some(event) = event_for(visibility.hidden_by_fullscreen, check.fullscreen) else {
        return;
    };
    log_decision(event, &check, visibility.user_hidden);
    runtime.apply_visibility(event);
}

/// The visibility event for a check result, or `None` when the bubbles
/// already reflect it.
pub fn event_for(hidden_by_fullscreen: bool, fullscreen: bool) -> Option<Event> {
    match (hidden_by_fullscreen, fullscreen) {
        (false, true) => Some(Event::FullscreenOn),
        (true, false) => Some(Event::FullscreenOff),
        _ => None,
    }
}

fn log_decision(event: Event, check: &FullscreenCheck, user_hidden: bool) {
    let action = if event == Event::FullscreenOn {
        "hide"
    } else {
        "show"
    };
    log::info!(
        "bubble fullscreen {action} reason={} {} user_hidden={user_hidden}",
        check.reason,
        check.detail
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::visibility::{self, Action, Visibility};

    #[test]
    fn only_a_change_produces_an_event() {
        assert_eq!(event_for(false, true), Some(Event::FullscreenOn));
        assert_eq!(event_for(true, false), Some(Event::FullscreenOff));
        assert_eq!(event_for(false, false), None);
        assert_eq!(event_for(true, true), None);
    }

    /// Feed a sequence of check results through the state machine and
    /// collect the actions the bubbles receive.
    fn run(start: Visibility, checks: &[bool]) -> (Visibility, Vec<Action>) {
        let mut state = start;
        let mut actions = Vec::new();
        for &fullscreen in checks {
            if let Some(event) = event_for(state.hidden_by_fullscreen, fullscreen) {
                let (next, action) = visibility::next(state, event);
                state = next;
                actions.extend(action);
            }
        }
        (state, actions)
    }

    #[test]
    fn a_fullscreen_spell_hides_once_and_shows_once() {
        let (state, actions) = run(Visibility::default(), &[false, true, true, true, false]);
        assert_eq!(actions, [Action::Hide, Action::Show]);
        assert_eq!(state, Visibility::default());
    }

    #[test]
    fn user_hidden_bubbles_stay_hidden_after_fullscreen() {
        let start = Visibility::from_widget_visible(false);
        let (state, actions) = run(start, &[true, false]);
        assert_eq!(actions, [Action::Hide]);
        assert_eq!(state, start);
    }

    #[test]
    fn a_user_show_during_fullscreen_is_not_undone_by_the_next_check() {
        let (hidden, _) = run(Visibility::default(), &[true]);
        let (shown, action) = visibility::next(hidden, Event::UserShow);
        assert_eq!(action, Some(Action::Show));
        // Fullscreen is still on: no event, so the bubbles stay shown.
        let (state, actions) = run(shown, &[true, true]);
        assert!(actions.is_empty());
        // Leaving fullscreen clears the flag; the bubbles are already shown.
        let (state, actions) = run(state, &[false]);
        assert_eq!(actions, [Action::Show]);
        assert!(state.shows());
    }
}
