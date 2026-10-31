//! Integration tests use only the crate's public API, the way `main.rs` does.
//!
//!   <https://doc.rust-lang.org/cargo/reference/cargo-targets.html#integration-tests>

use amzn_no_donuts::presence::{self, Rules, Sighting, State, Tracker};
use std::time::{Duration, SystemTime};

#[test]
fn a_full_absence_cycle() {
    let rules = Rules::new(10, 500, false, 2);
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let mut tracker = Tracker::new();
    let mut fired = 0;

    // Seen for 5 s, gone for 20 s, back for 5 s.
    for tenth in 0..60 {
        let now = start + Duration::from_millis(tenth * 500);
        let seen = if (10..50).contains(&tenth) {
            Sighting::NOBODY
        } else {
            Sighting::OWNER
        };
        let (next, fire) = presence::next(tracker, seen, now, &rules);
        tracker = next;
        if fire {
            fired += 1;
        }
    }
    assert_eq!(fired, 1);
    assert_eq!(tracker.state, State::Present);
}
