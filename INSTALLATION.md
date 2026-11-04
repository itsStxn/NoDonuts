# Installing NoDonuts

NoDonuts is built from source and installed as a single file,
`~/.local/bin/nodonuts`.

This guide uses only `git` and Rust's `cargo`. Pick your system, copy the commands into
a terminal, and press Enter. The source goes into `~/NoDonuts`.

## Linux (Ubuntu)

```bash
sudo apt install -y git curl build-essential libclang-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env
git clone https://github.com/itsStxn/NoDonuts ~/NoDonuts        # Feel free to choose a different location than ~/
cd ~/NoDonuts                                                   # If you chose a different location, make sure to use that
cargo build --release
mkdir -p ~/.local/bin
install -m 755 target/release/nodonuts ~/.local/bin/nodonuts
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
nodonuts --version
```

The last line prints `nodonuts <version>`. The build takes a few minutes the
first time.

- The `apt` line installs a C compiler and libclang: the camera library
  needs them to build on Linux. It's the only line that needs `sudo`.
- The `curl` line installs Rust. Skip it if you already have Rust 1.85 or
  newer (`rustc --version`).
- `source ~/.cargo/env` makes `cargo` work in this terminal. New terminals
  pick it up on their own.

## macOS

First, install Apple's command line tools (skip if you already have them),
and wait for the install window to finish:

```bash
xcode-select --install
```

Then:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env
git clone https://github.com/itsStxn/NoDonuts ~/NoDonuts        # Feel free to choose a different location than ~/
cd ~/NoDonuts                                                   # If you chose a different location, make sure to use that
cargo build --release
mkdir -p ~/.local/bin
install -m 755 target/release/nodonuts ~/.local/bin/nodonuts
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
nodonuts --version
```

The last line prints `nodonuts 0.2.0`.

- The `curl` line installs Rust. Skip it if you already have Rust 1.85 or
  newer.
- `source ~/.cargo/env` makes `cargo` work in this terminal.

On both systems, the `PATH` line uses `$HOME` rather than `~` on purpose:
`~` isn't expanded inside double quotes, so `"~/.local/bin"` would not work.

## After installing

```bash
nodonuts enroll
nodonuts config set command 'loginctl lock-session'    # Linux: lock the screen
nodonuts config set autostart true                     # optional: start at every login
nodonuts start
nodonuts status
```

On macOS, use `pmset displaysleepnow` as the command instead.

Settings can only be changed while NoDonuts is stopped, so set them before
`nodonuts start`.

The first `nodonuts start` asks you to choose a fallback password. You'll
type it in `nodonuts stop` if your face isn't recognized.

See [README.md](README.md) for every command and setting.

## Update

Linux and macOS:

```bash
source ~/.cargo/env
cd ~/NoDonuts
git pull
cargo build --release
nodonuts stop
install -m 755 target/release/nodonuts ~/.local/bin/nodonuts
nodonuts start
```

Your enrolled face and settings are kept.

- Stay in front of the camera when running these: `nodonuts stop` checks for
  your face first (`face_checkout`, on by default). If it doesn't see you, it
  asks for your fallback password. If that fails too, your command runs and
  the watcher keeps running; run `nodonuts stop` again. The same goes for
  uninstalling.
- Re-enroll (`nodonuts enroll`) only if an update replaces the face model
  (`models/sface.onnx`).
- **Moving the program:** `autostart` stores the program's full path. If you
  move `nodonuts`, stop the watcher, then run `nodonuts refresh` from its
  new location. `nodonuts status` warns when the stored path is broken.

## Uninstall

```bash
nodonuts stop
nodonuts config set autostart false
rm ~/.local/bin/nodonuts
```

To also delete your settings, enrolled face, fallback password and log:

```bash
rm -rf ~/.config/nodonuts ~/.local/share/nodonuts         # Linux
rm -rf ~/Library/Application\ Support/nodonuts            # macOS
```

If your command ever ran, also delete `NODONUTS ALERT.txt` from your desktop
(or from your `alert_location` folder).

To delete the source too: `rm -rf ~/NoDonuts`.

## Camera access

- **Linux:** the logged-in desktop user usually has access. If
  `nodonuts enroll` can't open the camera, run
  `sudo usermod -aG video "$USER"`, then log out and back in.
- **macOS:** the first `nodonuts enroll` asks for camera permission for your
  terminal app. If you declined, allow it in System Settings > Privacy &
  Security > Camera, then reopen the terminal. If the watcher started by
  `autostart` can't use the camera, allow `nodonuts` in the same place.

## Troubleshooting

| Problem | Fix |
|---|---|
| `cargo: command not found` | Run `source ~/.cargo/env` |
| `could not find Cargo.toml` | Run `cd ~/NoDonuts` first |
| `Unable to find libclang` (Linux) | Run `sudo apt install libclang-dev`, then build again |
| `linker 'cc' not found` (Linux) | Run `sudo apt install build-essential` |
| `feature edition2024 is required` | Your Rust is too old: run `rustup update` |
| `git clone` fails with an authentication error | Run `mwinit`, then try again |
| `git clone` says the destination already exists | You already installed once. Use [Update](#update) |
| `nodonuts: command not found` | Open a new terminal |

## Where files live

| | Linux | macOS |
|---|---|---|
| Program | `~/.local/bin/nodonuts` | `~/.local/bin/nodonuts` |
| Source | `~/NoDonuts` | `~/NoDonuts` |
| Settings | `~/.config/nodonuts/config.toml` | `~/Library/Application Support/nodonuts/config.toml` |
| Enrolled face | `~/.local/share/nodonuts/owner_face.toml` | `~/Library/Application Support/nodonuts/owner_face.toml` |
| Fallback password (hash only) | `~/.local/share/nodonuts/fallback_checkout.toml` | `~/Library/Application Support/nodonuts/fallback_checkout.toml` |
| Log | `~/.local/share/nodonuts/nodonuts.log` | `~/Library/Application Support/nodonuts/nodonuts.log` |
| Login entry (`autostart`) | `~/.config/autostart/nodonuts.desktop` | `~/Library/LaunchAgents/com.amazon.nodonuts.plist` |
| Alert (`alert_location`), written when your command runs | `~/Desktop/NODONUTS ALERT.txt` | `~/Desktop/NODONUTS ALERT.txt` |

