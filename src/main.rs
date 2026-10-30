//! Command-line entry point for amzn-no-donuts.
//!
//! Lint levels live in the `[lints]` table of `Cargo.toml`, which applies to
//! every target in the package (including this binary), so the crate root needs
//! its own `//!` doc comment to satisfy `missing_docs`.

// This program is exported as a binary named `amzn-no-donuts`.
//
// You can run it via Brazil:
//
// ```console
// $ brazil-build # needed once
// $ brazil-runtime-exec amzn-no-donuts
// ```

use amzn_no_donuts::hello;

fn main() {
    println!("{}", hello("NoDonuts"));
}
