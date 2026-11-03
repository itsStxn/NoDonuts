//! How `nodonuts stop` and the running watcher talk.
//!
//! The watcher listens on a local socket (a file in the NoDonuts data
//! folder, owner-only). `nodonuts stop` connects and sends `STOP`. The
//! watcher then runs the face checkout and, if needed, asks for the
//! fallback password, sending one line per step so `stop` can show progress
//! and prompt at the right moment:
//!
//! ```text
//! stop    -> watcher : STOP
//! watcher -> stop    : LOOKING 1 3      (looking at frame 1 of 3)
//! watcher -> stop    : NOT_SEEN 1 3     (frame 1 didn't show the owner)
//! ...
//! watcher -> stop    : PASSWORD 1 3     (type the password, try 1 of 3)
//! stop    -> watcher : <the password>
//! watcher -> stop    : WRONG            (or DONE ...)
//! watcher -> stop    : DONE owner       (how it ended; then the watcher exits)
//! ```
//!
//! The password is checked by the watcher against the hash it loaded at
//! start, so editing files on disk while it runs changes nothing.

/// What `stop` sends first.
pub const STOP_REQUEST: &str = "STOP";

/// A line from the watcher to `stop`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Looking at the camera: face checkout frame `attempt` of `of`.
    Looking {
        /// This attempt, from 1.
        attempt: u32,
        /// How many attempts there are.
        of: u32,
    },
    /// That frame didn't show the owner.
    NotSeen {
        /// This attempt, from 1.
        attempt: u32,
        /// How many attempts there are.
        of: u32,
    },
    /// Waiting for the fallback password: try `attempt` of `of`.
    Password {
        /// This try, from 1.
        attempt: u32,
        /// How many tries there are.
        of: u32,
    },
    /// The password was wrong.
    Wrong,
    /// The checkout is over; the word says how (see `StopOutcome::as_str`).
    Done(String),
}

impl Reply {
    /// The line to send, including the newline.
    pub fn to_line(&self) -> String {
        match self {
            Reply::Looking { attempt, of } => format!("LOOKING {attempt} {of}\n"),
            Reply::NotSeen { attempt, of } => format!("NOT_SEEN {attempt} {of}\n"),
            Reply::Password { attempt, of } => format!("PASSWORD {attempt} {of}\n"),
            Reply::Wrong => "WRONG\n".to_string(),
            Reply::Done(outcome) => format!("DONE {outcome}\n"),
        }
    }

    /// Reads a line back. `None` for anything unrecognized.
    pub fn parse(line: &str) -> Option<Reply> {
        let mut words = line.split_whitespace();
        let kind = words.next()?;
        // A small helper: the next word as a number.
        let mut number = || -> Option<u32> { words.next()?.parse().ok() };
        match kind {
            "LOOKING" => Some(Reply::Looking {
                attempt: number()?,
                of: number()?,
            }),
            "NOT_SEEN" => Some(Reply::NotSeen {
                attempt: number()?,
                of: number()?,
            }),
            "PASSWORD" => Some(Reply::Password {
                attempt: number()?,
                of: number()?,
            }),
            "WRONG" => Some(Reply::Wrong),
            "DONE" => Some(Reply::Done(line.split_whitespace().nth(1)?.to_string())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reply_round_trips() {
        for reply in [
            Reply::Looking { attempt: 1, of: 3 },
            Reply::NotSeen { attempt: 2, of: 3 },
            Reply::Password { attempt: 3, of: 3 },
            Reply::Wrong,
            Reply::Done("owner".to_string()),
        ] {
            assert_eq!(Reply::parse(&reply.to_line()), Some(reply));
        }
    }

    #[test]
    fn garbage_is_not_a_reply() {
        for line in ["", "HELLO", "LOOKING", "LOOKING x 3", "DONE"] {
            assert_eq!(Reply::parse(line), None, "{line:?}");
        }
    }
}
