//! The presence state machine: decides *when* to run the command.
//!
//! This module is pure logic. It never touches the camera or the clock: the
//! caller passes in whether the owner was seen and what time it is. That
//! makes every situation (lid closed, clock jumps, flicker) easy to test.

use std::time::{Duration, SystemTime};

/// Where we are in the "is the owner here?" story.
// `Clone, Copy` lets us pass a `State` around by value, like a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The owner was seen at the last check.
    Present,
    /// The owner has not been seen since `since`.
    Away {
        /// When the current absence started (or when the laptop woke up).
        since: SystemTime,
    },
    /// The command already ran for this absence. It stays quiet until the
    /// owner is seen again.
    Fired,
}

/// The state plus the little memory needed to notice sleep/wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tracker {
    /// Current state.
    pub state: State,
    /// The last moment we know the laptop was awake. `None` before the
    /// first check.
    pub last_awake: Option<SystemTime>,
}

impl Tracker {
    /// A fresh tracker. Starting as `Present` gives the owner a full grace
    /// period even if they walk away right after `nodonuts start`.
    pub fn new() -> Tracker {
        Tracker {
            state: State::Present,
            last_awake: None,
        }
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Tracker::new()
    }
}

/// Timing rules, taken from the config.
#[derive(Debug, Clone, Copy)]
pub struct Rules {
    /// How long the owner may be away before the command runs.
    pub grace: Duration,
    /// A gap between checks longer than this means the laptop was asleep.
    pub wake_gap: Duration,
}

impl Rules {
    /// Builds the rules from config values. A wake is a gap longer than 5
    /// check intervals.
    pub fn new(grace_seconds: u64, check_every_ms: u64) -> Rules {
        Rules {
            grace: Duration::from_secs(grace_seconds),
            wake_gap: Duration::from_millis(check_every_ms.saturating_mul(5)),
        }
    }
}

/// Computes the next tracker and whether to run the command now.
///
/// Returns a tuple: `(next tracker, fire)`.
pub fn next(current: Tracker, owner_seen: bool, now: SystemTime, rules: &Rules) -> (Tracker, bool) {
    let mut state = current.state;

    // 1. Did the laptop just wake up? `Instant` would not tell us: it stops
    //    counting while the machine sleeps. The wall clock keeps going.
    if just_woke_up(current.last_awake, now, rules.wake_gap) {
        // Give the owner a full grace period after opening the lid.
        // A `Fired` state stays `Fired`: no second run for the same absence.
        if let State::Away { .. } = state {
            state = State::Away { since: now };
        }
    }

    // 2. The normal transitions. `match` on a tuple handles each
    //    (state, seen) combination explicitly.
    let (state, fire) = match (state, owner_seen) {
        (_, true) => (State::Present, false),
        (State::Present, false) => (State::Away { since: now }, false),
        (State::Away { since }, false) => {
            // `duration_since` fails if `since` is in the future. That
            // cannot happen after step 1, but treat it as "no time passed".
            let away_for = now.duration_since(since).unwrap_or(Duration::ZERO);
            if away_for >= rules.grace {
                (State::Fired, true)
            } else {
                (State::Away { since }, false)
            }
        }
        (State::Fired, false) => (State::Fired, false),
    };

    let next = Tracker {
        state,
        last_awake: Some(now),
    };
    (next, fire)
}

/// True if the gap since the last check is too long, or the clock went
/// backwards (also a sign the laptop slept or the time was changed).
fn just_woke_up(last_awake: Option<SystemTime>, now: SystemTime, wake_gap: Duration) -> bool {
    match last_awake {
        None => false,
        Some(previous) => match now.duration_since(previous) {
            Ok(gap) => gap > wake_gap,
            Err(_) => true, // `now` is before `previous`: clock went backwards.
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHECK_MS: u64 = 500;
    const GRACE_S: u64 = 30;

    /// Drives the state machine like the watch loop does, with fake time.
    struct Sim {
        tracker: Tracker,
        now: SystemTime,
        rules: Rules,
        fired: u32,
    }

    impl Sim {
        fn new() -> Sim {
            Sim {
                tracker: Tracker::new(),
                now: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000),
                rules: Rules::new(GRACE_S, CHECK_MS),
                fired: 0,
            }
        }

        fn check(&mut self, seen: bool) {
            let (tracker, fire) = next(self.tracker, seen, self.now, &self.rules);
            self.tracker = tracker;
            if fire {
                self.fired += 1;
            }
        }

        /// Runs checks every CHECK_MS for `seconds`.
        fn run_for(&mut self, seconds: u64, seen: bool) {
            for _ in 0..(seconds * 1000 / CHECK_MS) {
                self.now += Duration::from_millis(CHECK_MS);
                self.check(seen);
            }
        }

        /// The laptop sleeps for `seconds`, then one check happens.
        fn sleep_then_check(&mut self, seconds: u64, seen: bool) {
            self.now += Duration::from_secs(seconds);
            self.check(seen);
        }
    }

    #[test]
    fn brief_absence_under_grace_does_not_fire() {
        let mut sim = Sim::new();
        sim.run_for(5, true);
        sim.run_for(GRACE_S - 5, false);
        sim.run_for(5, true);
        assert_eq!(sim.fired, 0);
        assert_eq!(sim.tracker.state, State::Present);
    }

    #[test]
    fn absence_over_grace_fires_once() {
        let mut sim = Sim::new();
        sim.run_for(5, true);
        sim.run_for(GRACE_S * 4, false);
        assert_eq!(sim.fired, 1);
        assert_eq!(sim.tracker.state, State::Fired);
    }

    #[test]
    fn return_after_fire_re_arms() {
        let mut sim = Sim::new();
        sim.run_for(GRACE_S + 5, false);
        assert_eq!(sim.fired, 1);
        sim.run_for(1, true);
        assert_eq!(sim.tracker.state, State::Present);
        sim.run_for(GRACE_S + 5, false);
        assert_eq!(sim.fired, 2);
    }

    #[test]
    fn flicker_resets_the_grace_period() {
        let mut sim = Sim::new();
        // Away 20 s, seen for one check, away 20 s, ... never 30 s in a row.
        for _ in 0..10 {
            sim.run_for(20, false);
            sim.check(true);
        }
        assert_eq!(sim.fired, 0);
    }

    #[test]
    fn wake_while_away_restarts_grace() {
        let mut sim = Sim::new();
        sim.run_for(10, false); // away for 10 s, then the lid closes
        sim.sleep_then_check(3600, false); // opened an hour later
        assert_eq!(sim.fired, 0, "the owner gets a full grace period on wake");
        sim.run_for(GRACE_S - 5, false);
        assert_eq!(sim.fired, 0);
        sim.run_for(10, false);
        assert_eq!(sim.fired, 1);
    }

    #[test]
    fn wake_while_present_and_owner_missing_starts_grace() {
        let mut sim = Sim::new();
        sim.run_for(5, true);
        sim.sleep_then_check(3600, false);
        assert_eq!(sim.fired, 0);
        sim.run_for(GRACE_S + 1, false);
        assert_eq!(sim.fired, 1);
    }

    #[test]
    fn wake_while_fired_stays_fired() {
        let mut sim = Sim::new();
        sim.run_for(GRACE_S + 5, false);
        assert_eq!(sim.fired, 1);
        sim.sleep_then_check(3600, false);
        sim.run_for(GRACE_S * 2, false);
        assert_eq!(sim.fired, 1);
        assert_eq!(sim.tracker.state, State::Fired);
    }

    #[test]
    fn clock_going_backwards_restarts_grace() {
        let mut sim = Sim::new();
        sim.run_for(25, false);
        sim.now -= Duration::from_secs(600);
        sim.check(false);
        // Without the reset, `since` would be in the future and never elapse;
        // with it, the owner gets a fresh full grace period.
        sim.run_for(GRACE_S - 5, false);
        assert_eq!(sim.fired, 0);
        sim.run_for(10, false);
        assert_eq!(sim.fired, 1);
    }

    #[test]
    fn short_gaps_are_not_wakes() {
        let mut sim = Sim::new();
        sim.run_for(20, false);
        // A 2 s hiccup (< 5 x 500 ms) still counts toward the grace period.
        sim.sleep_then_check(2, false);
        sim.run_for(9, false);
        assert_eq!(sim.fired, 1);
    }
}
