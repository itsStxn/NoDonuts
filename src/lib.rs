//! One-sentence summary of your crate.
//!
//! Followed by more detailed Markdown documentation of your crate.
//!
//! Lint levels for this crate live in the `[lints]` table of `Cargo.toml`
//! (a single source of truth across all targets), not in a `#![warn(...)]`
//! attribute here.

// Third-party dependencies are available once they're specified within Cargo.toml.
#[allow(unused_imports)]
use serde::Serialize;
// First-party dependencies are also available once they're specified within Cargo.toml (and your Config).
// use amzn_metrics::unit_of_work::metrics;
// use amzn_metric_writer::Entry;

/// Generates a personalized greeting.
///
/// # Example
///
/// ```
/// use amzn_no_donuts::hello;
///
/// let greeting = hello("Doc-tests");
/// println!("{}", greeting);
/// ```
pub fn hello(name: &str) -> String {
    format!("Hello new friend: {name}")
}

// An example unit test testing nothing in particular.
//
// Unit tests have access to private types, methods, and fields. The Rust
// Book provides more details:
//
//   https://doc.rust-lang.org/book/ch11-03-test-organization.html#unit-tests
//
#[cfg(test)]
mod tests {
    use super::hello;

    #[test]
    fn unit_test() {
        assert_eq!(hello("Brazil"), "Hello new friend: Brazil");
    }
}
