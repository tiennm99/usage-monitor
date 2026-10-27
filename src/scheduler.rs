//! The single timer thread. It owns the poll, countdown and update-check
//! deadlines and is driven by messages over an `mpsc` channel.
//!
//! The deadline rules live in [`Deadlines`], which takes the current time as
//! an argument and so is tested without a thread, a window or a clock. The
//! thread only waits for the nearest deadline or the next message and hands
//! each due [`Action`] to its [`SchedulerHost`]. A host must return quickly:
//! slow work (HTTP, the CLI refresh) runs on worker threads it spawns.
//!
//! The poll and the update check run on local clock boundaries (see
//! [`clock_slot`]): a 5-minute poll at :00, :05, :10 and so on, a daily
//! update check at midnight. `Instant` stops while the machine sleeps, so
//! those deadlines are kept as unix timestamps, and the thread wakes at
//! least every [`MAX_WAIT`] to notice a resume (see [`resumed_from_sleep`]).

use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jiff::tz::TimeZone;

use crate::core::state::{clock_slot, update_check_due, UpdateCheckDue};

/// Shortest poll interval, guarding against a hand-edited `0` in the
/// settings file turning into a busy loop.
const MIN_POLL_INTERVAL: Duration = Duration::from_secs(10);

/// Shortest automatic update-check interval, for the same reason.
const MIN_UPDATE_INTERVAL: Duration = Duration::from_secs(60);

/// First retry delay after a failed update check; it doubles per failure
/// in a row and never exceeds the check interval.
const UPDATE_RETRY_BASE: Duration = Duration::from_secs(60);

/// Longest the thread sleeps between passes, so a resume from sleep or a
/// wall-clock deadline is noticed within about this long.
const MAX_WAIT: Duration = Duration::from_secs(60);

/// How far wall time may run ahead of `Instant` between two passes before
/// the gap counts as a sleep.
const RESUME_THRESHOLD: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// Poll at once. The periodic poll deadline is left as it is.
    PollNow,
    /// Change the poll interval and re-arm it at its next boundary, without
    /// polling.
    SetPollInterval(Duration),
    /// Re-render the countdowns after this delay; `None` clears the deadline.
    RecomputeCountdown(Option<Duration>),
    /// The auto-check interval or its last check changed: check now if one
    /// is due, otherwise wait for the remainder. `interval_secs: None`
    /// cancels the deadline.
    SetUpdateInterval {
        interval_secs: Option<u64>,
        last_check_unix: Option<u64>,
    },
    /// An update check finished. Only this re-arms the check: at the next
    /// boundary when it succeeded, after a growing retry delay capped at
    /// that boundary when it failed.
    UpdateCheckDone {
        interval_secs: Option<u64>,
        succeeded: bool,
    },
    /// Stop the thread.
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Poll,
    Countdown,
    CheckForUpdates,
}

/// Receives the due actions on the scheduler thread.
pub trait SchedulerHost {
    fn run(&self, action: Action);
}

/// Settings the deadlines start from.
#[derive(Clone, Debug)]
pub struct Config {
    pub poll_interval: Duration,
    pub update_interval_secs: Option<u64>,
    pub last_update_check_unix: Option<u64>,
    /// The zone whose clock the poll and the update check follow.
    pub tz: TimeZone,
}

#[derive(Debug)]
pub struct Deadlines {
    tz: TimeZone,
    poll_interval: Duration,
    /// Unix seconds the next poll is due at.
    poll_at_unix: u64,
    countdown_at: Option<Instant>,
    /// Unix seconds the next update check is due at.
    update_at_unix: Option<u64>,
    /// Failed update checks in a row, which set the retry delay.
    update_failures: u32,
}

impl Deadlines {
    /// Arm the deadlines at startup. Returns the actions due at once: the
    /// update check when the last one is older than its interval.
    pub fn new(config: Config, now_unix: u64) -> (Self, Vec<Action>) {
        let mut deadlines = Self {
            tz: config.tz,
            poll_interval: config.poll_interval.max(MIN_POLL_INTERVAL),
            poll_at_unix: 0,
            countdown_at: None,
            update_at_unix: None,
            update_failures: 0,
        };
        deadlines.poll_at_unix = deadlines.next_poll(now_unix);
        let actions = deadlines.arm_update(
            config.update_interval_secs,
            config.last_update_check_unix,
            now_unix,
        );
        (deadlines, actions)
    }

    /// Apply one message. Returns the actions to run at once.
    pub fn handle(&mut self, message: Message, now: Instant, now_unix: u64) -> Vec<Action> {
        match message {
            Message::PollNow => vec![Action::Poll],
            Message::SetPollInterval(interval) => {
                self.poll_interval = interval.max(MIN_POLL_INTERVAL);
                self.poll_at_unix = self.next_poll(now_unix);
                Vec::new()
            }
            Message::RecomputeCountdown(delay) => {
                self.countdown_at = delay.map(|d| now + d);
                Vec::new()
            }
            Message::SetUpdateInterval {
                interval_secs,
                last_check_unix,
            } => self.arm_update(interval_secs, last_check_unix, now_unix),
            Message::UpdateCheckDone {
                interval_secs,
                succeeded,
            } => {
                self.update_at_unix = interval_secs.map(|secs| {
                    let next = self.next_update(secs, now_unix);
                    if succeeded {
                        self.update_failures = 0;
                        next
                    } else {
                        let retry = now_unix.saturating_add(self.retry_delay().as_secs());
                        self.update_failures = self.update_failures.saturating_add(1);
                        retry.min(next)
                    }
                });
                Vec::new()
            }
            Message::Shutdown => Vec::new(),
        }
    }

    /// Fire every deadline that has passed. The poll re-arms at its next
    /// boundary; the countdown and update check stay disarmed until the
    /// host re-arms them (after the re-render, after the check finished).
    pub fn take_due(&mut self, now: Instant, now_unix: u64) -> Vec<Action> {
        let mut due = Vec::new();
        if self.poll_at_unix <= now_unix {
            due.push(Action::Poll);
            self.poll_at_unix = self.next_poll(now_unix);
        }
        if self.countdown_at.is_some_and(|at| at <= now) {
            due.push(Action::Countdown);
            self.countdown_at = None;
        }
        if self.update_at_unix.is_some_and(|at| at <= now_unix) {
            due.push(Action::CheckForUpdates);
            self.update_at_unix = None;
        }
        due
    }

    /// The machine woke from sleep: poll at once and re-arm the poll at its
    /// next boundary. The update check needs nothing here, since
    /// [`Self::take_due`] compares its wall-clock deadline with `now_unix`.
    pub fn resume(&mut self, now_unix: u64) -> Vec<Action> {
        self.poll_at_unix = self.next_poll(now_unix);
        vec![Action::Poll]
    }

    /// Time until the nearest deadline. The poll deadline is always armed,
    /// so there always is one.
    pub fn next_wakeup(&self, now: Instant, now_unix: u64) -> Duration {
        let until = |at: u64| Duration::from_secs(at.saturating_sub(now_unix));
        [
            Some(until(self.poll_at_unix)),
            self.countdown_at
                .map(|at| at.saturating_duration_since(now)),
            self.update_at_unix.map(until),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(self.poll_interval)
    }

    fn arm_update(
        &mut self,
        interval_secs: Option<u64>,
        last_check_unix: Option<u64>,
        now_unix: u64,
    ) -> Vec<Action> {
        let interval_secs = interval_secs.map(|secs| secs.max(MIN_UPDATE_INTERVAL.as_secs()));
        match update_check_due(last_check_unix, interval_secs, now_unix, &self.tz) {
            None => {
                self.update_at_unix = None;
                Vec::new()
            }
            Some(UpdateCheckDue::Now) => {
                self.update_at_unix = None;
                vec![Action::CheckForUpdates]
            }
            Some(UpdateCheckDue::After(remaining)) => {
                self.update_at_unix = Some(now_unix.saturating_add(remaining.as_secs()));
                Vec::new()
            }
        }
    }

    /// The poll boundary after `now_unix`.
    fn next_poll(&self, now_unix: u64) -> u64 {
        clock_slot(self.poll_interval.as_secs(), now_unix, &self.tz).1
    }

    /// The update-check boundary after `now_unix`.
    fn next_update(&self, interval_secs: u64, now_unix: u64) -> u64 {
        let interval_secs = interval_secs.max(MIN_UPDATE_INTERVAL.as_secs());
        clock_slot(interval_secs, now_unix, &self.tz).1
    }

    /// Delay before the next try after the current run of failures:
    /// `UPDATE_RETRY_BASE * 2^failures`; the caller caps it at the next
    /// boundary.
    fn retry_delay(&self) -> Duration {
        let factor = 1u32.checked_shl(self.update_failures).unwrap_or(u32::MAX);
        UPDATE_RETRY_BASE.saturating_mul(factor)
    }
}

/// Whether the wall clock ran ahead of `Instant` by more than
/// [`RESUME_THRESHOLD`] between two passes, which means the machine slept
/// (or the clock was set forward). `wall_elapsed` is `None` when the wall
/// clock went backwards.
pub fn resumed_from_sleep(instant_elapsed: Duration, wall_elapsed: Option<Duration>) -> bool {
    wall_elapsed.is_some_and(|wall| wall.saturating_sub(instant_elapsed) > RESUME_THRESHOLD)
}

/// Seconds since the Unix epoch; 0 for a clock set before 1970.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Start the scheduler thread reading `rx`. Create the channel first, so
/// the sender exists before the host runs its first action.
/// [`Message::Shutdown`] or dropping every sender stops the thread.
pub fn spawn<H>(config: Config, rx: Receiver<Message>, host: H) -> io::Result<JoinHandle<()>>
where
    H: SchedulerHost + Send + 'static,
{
    std::thread::Builder::new()
        .name("scheduler".into())
        .spawn(move || {
            let (deadlines, initial) = Deadlines::new(config, unix_now());
            for action in initial {
                host.run(action);
            }
            run_loop(deadlines, &rx, &host);
            log::info!("scheduler stopped");
        })
}

fn run_loop<H: SchedulerHost>(mut deadlines: Deadlines, rx: &Receiver<Message>, host: &H) {
    let mut last_instant = Instant::now();
    let mut last_wall = SystemTime::now();
    loop {
        let (now, wall) = (Instant::now(), SystemTime::now());
        let instant_elapsed = now.saturating_duration_since(last_instant);
        let wall_elapsed = wall.duration_since(last_wall).ok();
        (last_instant, last_wall) = (now, wall);
        if resumed_from_sleep(instant_elapsed, wall_elapsed) {
            log::info!(
                "wall clock moved {wall_elapsed:?} while Instant moved {instant_elapsed:?}; \
                 treating it as a resume from sleep"
            );
            for action in deadlines.resume(unix_now()) {
                host.run(action);
            }
        }
        for action in deadlines.take_due(now, unix_now()) {
            host.run(action);
        }
        let wait = deadlines
            .next_wakeup(Instant::now(), unix_now())
            .min(MAX_WAIT);
        let message = match rx.recv_timeout(wait) {
            Ok(Message::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => continue,
        };
        for action in deadlines.handle(message, Instant::now(), unix_now()) {
            host.run(action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Sender};

    use jiff::tz;

    use crate::core::settings::{
        UPDATE_CHECK_DAILY_SECS, UPDATE_CHECK_HOURLY_SECS, UPDATE_CHECK_WEEKLY_SECS,
    };

    const MIN: Duration = Duration::from_secs(60);
    const M: u64 = 60;
    const HOUR: u64 = 60 * M;
    const DAY: u64 = 24 * HOUR;
    /// Friday 2027-01-15 08:00:00 UTC, a whole hour.
    const NOW_UNIX: u64 = 1_800_000_000;
    /// The Monday 00:00 UTC that starts `NOW_UNIX`'s week.
    const MONDAY: u64 = NOW_UNIX - 4 * DAY - 8 * HOUR;

    fn config(update: Option<u64>, last: Option<u64>) -> Config {
        Config {
            poll_interval: 5 * MIN,
            update_interval_secs: update,
            last_update_check_unix: last,
            tz: TimeZone::UTC,
        }
    }

    #[test]
    fn poll_fires_on_interval_boundaries() {
        let t0 = Instant::now();
        // Started at 08:02:10, a 5-minute poll is next due at 08:05:00.
        let start = NOW_UNIX + 2 * M + 10;
        let (mut d, initial) = Deadlines::new(config(None, None), start);
        assert!(initial.is_empty());
        assert_eq!(d.next_wakeup(t0, start), Duration::from_secs(2 * M + 50));
        assert!(d.take_due(t0, NOW_UNIX + 5 * M - 1).is_empty());
        assert_eq!(d.take_due(t0, NOW_UNIX + 5 * M), [Action::Poll]);
        assert_eq!(d.next_wakeup(t0, NOW_UNIX + 5 * M), 5 * MIN);
        // A late wake-up keeps the boundaries: 08:10:20 re-arms 08:15:00.
        assert_eq!(d.take_due(t0, NOW_UNIX + 10 * M + 20), [Action::Poll]);
        assert!(d.take_due(t0, NOW_UNIX + 15 * M - 1).is_empty());
        assert_eq!(d.take_due(t0, NOW_UNIX + 15 * M), [Action::Poll]);
    }

    #[test]
    fn poll_boundaries_follow_the_local_clock() {
        // At +05:45, 5-minute boundaries fall on the local :00, :05 and so
        // on, which is UTC :15, :20 and so on.
        let mut cfg = config(None, None);
        cfg.tz = TimeZone::fixed(tz::Offset::from_seconds(5 * 3_600 + 45 * 60).unwrap());
        cfg.poll_interval = 60 * MIN;
        let (d, _) = Deadlines::new(cfg, NOW_UNIX);
        assert_eq!(
            d.next_wakeup(Instant::now(), NOW_UNIX),
            Duration::from_secs(15 * M)
        );
    }

    #[test]
    fn poll_now_polls_without_moving_the_periodic_deadline() {
        let t0 = Instant::now();
        let (mut d, _) = Deadlines::new(config(None, None), NOW_UNIX);
        assert_eq!(
            d.handle(Message::PollNow, t0, NOW_UNIX + 2 * M),
            [Action::Poll]
        );
        assert_eq!(d.take_due(t0, NOW_UNIX + 5 * M), [Action::Poll]);
    }

    #[test]
    fn new_poll_interval_rearms_at_its_next_boundary_without_polling() {
        let t0 = Instant::now();
        let (mut d, _) = Deadlines::new(config(None, None), NOW_UNIX);
        let t1 = NOW_UNIX + 4 * M;
        assert!(d
            .handle(Message::SetPollInterval(MIN * 15), t0, t1)
            .is_empty());
        assert!(d.take_due(t0, NOW_UNIX + 5 * M).is_empty());
        assert_eq!(d.next_wakeup(t0, t1), 11 * MIN);
        assert_eq!(d.take_due(t0, NOW_UNIX + 15 * M), [Action::Poll]);
    }

    #[test]
    fn a_zero_poll_interval_is_floored() {
        let mut cfg = config(None, None);
        cfg.poll_interval = Duration::ZERO;
        let (d, _) = Deadlines::new(cfg, NOW_UNIX);
        assert_eq!(d.next_wakeup(Instant::now(), NOW_UNIX), MIN_POLL_INTERVAL);
    }

    #[test]
    fn countdown_fires_once_until_rearmed() {
        let t0 = Instant::now();
        let (mut d, _) = Deadlines::new(config(None, None), NOW_UNIX);
        let delay = Duration::from_secs(31);
        d.handle(Message::RecomputeCountdown(Some(delay)), t0, NOW_UNIX);
        assert_eq!(d.next_wakeup(t0, NOW_UNIX), delay);
        assert_eq!(d.take_due(t0 + delay, NOW_UNIX), [Action::Countdown]);
        assert!(d.take_due(t0 + 2 * delay, NOW_UNIX).is_empty());

        d.handle(Message::RecomputeCountdown(Some(delay)), t0, NOW_UNIX);
        d.handle(Message::RecomputeCountdown(None), t0, NOW_UNIX);
        assert!(d.take_due(t0 + 2 * delay, NOW_UNIX).is_empty());
    }

    /// Whether a check is due at `now_unix`.
    fn check_due(d: &mut Deadlines, now_unix: u64) -> bool {
        d.take_due(Instant::now(), now_unix)
            .contains(&Action::CheckForUpdates)
    }

    fn done(interval_secs: Option<u64>, succeeded: bool) -> Message {
        Message::UpdateCheckDone {
            interval_secs,
            succeeded,
        }
    }

    #[test]
    fn startup_checks_only_when_a_boundary_passed_since_the_last_check() {
        let hourly = Some(UPDATE_CHECK_HOURLY_SECS);
        let (_, never_checked) = Deadlines::new(config(hourly, None), NOW_UNIX + M);
        assert_eq!(never_checked, [Action::CheckForUpdates]);
        // Checked at 07:59:59, started at 08:01: 08:00 passed in between.
        let (_, passed) = Deadlines::new(config(hourly, Some(NOW_UNIX - 1)), NOW_UNIX + M);
        assert_eq!(passed, [Action::CheckForUpdates]);
        // Checked at 08:00:30, started at 08:59: no boundary yet, so it
        // waits for 09:00.
        let (mut d, waiting) =
            Deadlines::new(config(hourly, Some(NOW_UNIX + 30)), NOW_UNIX + 59 * M);
        assert!(waiting.is_empty());
        assert!(!check_due(&mut d, NOW_UNIX + HOUR - 1));
        assert!(check_due(&mut d, NOW_UNIX + HOUR));
        let (_, disabled) = Deadlines::new(config(None, None), NOW_UNIX);
        assert!(disabled.is_empty());
    }

    #[test]
    fn daily_and_weekly_checks_fall_on_local_midnight() {
        let daily = Some(UPDATE_CHECK_DAILY_SECS);
        let midnight = NOW_UNIX - 8 * HOUR;
        let (_, before) = Deadlines::new(config(daily, Some(midnight - 1)), NOW_UNIX);
        assert_eq!(before, [Action::CheckForUpdates]);
        let (mut d, after) = Deadlines::new(config(daily, Some(midnight)), NOW_UNIX);
        assert!(after.is_empty());
        assert!(!check_due(&mut d, midnight + DAY - 1));
        assert!(check_due(&mut d, midnight + DAY));

        let weekly = Some(UPDATE_CHECK_WEEKLY_SECS);
        let (mut d, initial) = Deadlines::new(config(weekly, Some(MONDAY + 1)), NOW_UNIX);
        assert!(initial.is_empty());
        assert!(!check_due(&mut d, MONDAY + 7 * DAY - 1));
        assert!(check_due(&mut d, MONDAY + 7 * DAY));

        // At UTC+07:00 local midnight is 17:00 UTC the day before.
        let mut cfg = config(daily, Some(NOW_UNIX));
        cfg.tz = TimeZone::fixed(tz::offset(7));
        let (mut d, _) = Deadlines::new(cfg, NOW_UNIX);
        assert!(!check_due(&mut d, NOW_UNIX + 9 * HOUR - 1));
        assert!(check_due(&mut d, NOW_UNIX + 9 * HOUR));
    }

    #[test]
    fn a_finished_check_rearms_at_the_next_boundary() {
        let hour = UPDATE_CHECK_HOURLY_SECS;
        let (mut d, initial) = Deadlines::new(config(Some(hour), None), NOW_UNIX);
        assert_eq!(initial, [Action::CheckForUpdates]);
        // While the check runs nothing else is scheduled for it.
        assert!(!check_due(&mut d, NOW_UNIX + 3 * hour));

        let t1 = NOW_UNIX + 3 * hour + 20 * M;
        d.handle(done(Some(hour), true), Instant::now(), t1);
        assert!(!check_due(&mut d, NOW_UNIX + 4 * hour - 1));
        assert!(check_due(&mut d, NOW_UNIX + 4 * hour));
        // It repeats every boundary, not just once.
        d.handle(done(Some(hour), true), Instant::now(), NOW_UNIX + 4 * hour);
        assert!(check_due(&mut d, NOW_UNIX + 5 * hour));
    }

    #[test]
    fn a_failed_check_retries_with_a_doubling_delay_capped_at_the_next_boundary() {
        let day = UPDATE_CHECK_DAILY_SECS;
        let (mut d, _) = Deadlines::new(config(Some(day), None), NOW_UNIX);
        let mut now = NOW_UNIX;
        for expected in [60, 120, 240, 480, 960, 1_920, 3_840] {
            d.handle(done(Some(day), false), Instant::now(), now);
            assert!(!check_due(&mut d, now + expected - 1));
            assert!(check_due(&mut d, now + expected));
            now += expected;
        }
        // The next delay would pass midnight, so it retries at midnight.
        let midnight = NOW_UNIX - 8 * HOUR + DAY;
        now = midnight - 10 * M;
        d.handle(done(Some(day), false), Instant::now(), now);
        assert!(check_due(&mut d, midnight));
        // A success resets the delay: the next failure retries after a minute.
        d.handle(done(Some(day), true), Instant::now(), midnight);
        d.handle(done(Some(day), false), Instant::now(), midnight + HOUR);
        assert!(check_due(&mut d, midnight + HOUR + 60));
    }

    #[test]
    fn a_failed_check_with_auto_check_off_stays_off() {
        let (mut d, _) = Deadlines::new(config(None, None), NOW_UNIX);
        d.handle(done(None, false), Instant::now(), NOW_UNIX);
        assert!(!check_due(&mut d, NOW_UNIX + 7 * DAY));
    }

    #[test]
    fn the_wall_clock_deadlines_survive_a_sleep() {
        // `Instant` does not advance while the machine sleeps, but the
        // deadlines are due once the wall clock passes them.
        let t0 = Instant::now();
        let day = UPDATE_CHECK_DAILY_SECS;
        let (mut d, _) = Deadlines::new(config(Some(day), Some(NOW_UNIX)), NOW_UNIX);
        let midnight = NOW_UNIX - 8 * HOUR + DAY;
        assert!(!d
            .take_due(t0, midnight - 1)
            .contains(&Action::CheckForUpdates));
        assert!(d.take_due(t0, midnight).contains(&Action::CheckForUpdates));
    }

    #[test]
    fn resume_polls_at_once_and_rearms_the_poll_at_its_boundary() {
        let t0 = Instant::now();
        let (mut d, _) = Deadlines::new(config(None, None), NOW_UNIX);
        let woke = NOW_UNIX + 7 * HOUR + 2 * M;
        assert_eq!(d.resume(woke), [Action::Poll]);
        assert!(d.take_due(t0, woke).is_empty());
        assert_eq!(d.next_wakeup(t0, woke), 3 * MIN);
        assert_eq!(d.take_due(t0, woke + 3 * M), [Action::Poll]);
    }

    #[test]
    fn a_wall_clock_jump_past_instant_counts_as_a_resume() {
        let secs = Duration::from_secs;
        assert!(!resumed_from_sleep(MIN, Some(MIN)));
        // Ordinary scheduling slack is not a resume.
        assert!(!resumed_from_sleep(MIN, Some(MIN + secs(5))));
        assert!(!resumed_from_sleep(MIN, Some(MIN + RESUME_THRESHOLD)));
        assert!(resumed_from_sleep(
            MIN,
            Some(MIN + RESUME_THRESHOLD + secs(1))
        ));
        assert!(resumed_from_sleep(secs(1), Some(secs(8 * 3_600))));
        // A clock set back is not a resume.
        assert!(!resumed_from_sleep(MIN, None));
        assert!(!resumed_from_sleep(MIN, Some(Duration::ZERO)));
    }

    #[test]
    fn weekly_to_daily_checks_at_once_when_midnight_passed_since_the_last_check() {
        let last = Some(NOW_UNIX - 2 * DAY);
        let (mut d, initial) =
            Deadlines::new(config(Some(UPDATE_CHECK_WEEKLY_SECS), last), NOW_UNIX);
        assert!(initial.is_empty());
        assert_eq!(
            d.handle(
                Message::SetUpdateInterval {
                    interval_secs: Some(UPDATE_CHECK_DAILY_SECS),
                    last_check_unix: last,
                },
                Instant::now(),
                NOW_UNIX,
            ),
            [Action::CheckForUpdates]
        );
    }

    #[test]
    fn daily_to_weekly_waits_for_monday() {
        let last = Some(NOW_UNIX - 2 * DAY);
        let (mut d, _) = Deadlines::new(config(Some(UPDATE_CHECK_HOURLY_SECS), last), NOW_UNIX);
        let actions = d.handle(
            Message::SetUpdateInterval {
                interval_secs: Some(UPDATE_CHECK_WEEKLY_SECS),
                last_check_unix: last,
            },
            Instant::now(),
            NOW_UNIX,
        );
        assert!(actions.is_empty());
        assert!(!check_due(&mut d, MONDAY + 7 * DAY - 1));
        assert!(check_due(&mut d, MONDAY + 7 * DAY));
    }

    #[test]
    fn disabling_auto_check_cancels_the_deadline() {
        let (mut d, _) = Deadlines::new(
            config(Some(UPDATE_CHECK_HOURLY_SECS), Some(NOW_UNIX + 60)),
            NOW_UNIX + 600,
        );
        d.handle(
            Message::SetUpdateInterval {
                interval_secs: None,
                last_check_unix: Some(NOW_UNIX + 60),
            },
            Instant::now(),
            NOW_UNIX + 600,
        );
        assert!(!check_due(&mut d, NOW_UNIX + 7 * DAY));
        // A check that finishes after auto-check was turned off stays off.
        d.handle(done(None, true), Instant::now(), NOW_UNIX);
        assert!(!check_due(&mut d, NOW_UNIX + 14 * DAY));
    }

    /// Records actions on a channel, standing in for the runtime.
    struct FakeHost(Sender<Action>);

    impl SchedulerHost for FakeHost {
        fn run(&self, action: Action) {
            let _ = self.0.send(action);
        }
    }

    #[test]
    fn thread_runs_actions_and_stops_on_shutdown() {
        let (action_tx, action_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let handle = spawn(
            config(Some(UPDATE_CHECK_HOURLY_SECS), None),
            rx,
            FakeHost(action_tx),
        )
        .expect("spawn scheduler");
        let wait = Duration::from_secs(5);
        assert_eq!(action_rx.recv_timeout(wait), Ok(Action::CheckForUpdates));
        tx.send(Message::PollNow).expect("send");
        assert_eq!(action_rx.recv_timeout(wait), Ok(Action::Poll));
        tx.send(Message::RecomputeCountdown(Some(Duration::from_millis(20))))
            .expect("send");
        assert_eq!(action_rx.recv_timeout(wait), Ok(Action::Countdown));
        tx.send(Message::Shutdown).expect("send");
        handle.join().expect("join scheduler");
    }

    #[test]
    fn thread_stops_when_every_sender_is_dropped() {
        let (action_tx, _action_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let handle = spawn(config(None, None), rx, FakeHost(action_tx)).expect("spawn");
        drop(tx);
        handle.join().expect("join scheduler");
    }
}
