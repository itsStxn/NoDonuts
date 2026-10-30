# Contributing to NoDonuts

Thank you for your interest in contributing to the project.

## Setting up the package

This package is built with the [CargoBrazil] build system. Assuming that you already
have the package in a Brazil workspace, set up `cargo` integration by running `brazil-build`.

```console
$ brazil-build
```

Once the build succeeds, standard commands such as `cargo build` and `cargo run` will work. 
During the build step, CargoBrazil injected a rust-toolchain.toml file into your package 
that directs `cargo` to the correct location of the Brazil-supplied Rust toolchain. You 
will also have a `cargo` sub-command available, `cargo brazil`. 

`cargo brazil` provides several utilities for working with Brazil from within a Rust package.
You can retrieve a list of them by providing the `--help` flag.

[CargoBrazil]: https://code.amazon.com/packages/CargoBrazil

## Writing code

### Code style

This project follows the standard conventions for Rust projects that are imposed by 
[`rustfmt`](https://github.com/rust-lang/rustfmt). `rustfmt` is exposed via the 
`cargo fmt` sub-command.

```console
$ cargo fmt
```

To assist with writing idiomatic code, you should also regularly apply the `clippy`
code linter. This can also be invoked by `cargo`:

```console
$ cargo clippy
```

### First-party (1P) dependencies

To add an Amazon-internal dependency to the package, update both `Config` and 
`Cargo.toml` with the Brazil package name and crate's name, respectively.

### Third-party (3P) dependencies

To add third-party dependencies, add them to your package's `Cargo.toml` file.

_Note:_ Brazil builds do not have access to crates.io directly, but instead pull 
from an Amazon-internal mirror. Read the [Access to third-party crates] section 
of the CargoBrazil user guide for more details.

[Access to third-party crates]: https://docs.hub.amazon.dev/languages/rust/cargobrazil/#adding-a-3p-dependency
