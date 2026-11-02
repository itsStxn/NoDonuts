# Installing NoDonuts

NoDonuts is built from source and installed as a single file,
`~/.local/bin/nodonuts`. No `sudo` needed.

You need the Brazil CLI (`brazil`), which comes with Amazon Toolbox. Pick
your system, copy the commands into a terminal, and press Enter. The source
goes into a Brazil workspace at `~/NoDonuts`.

## Linux (Ubuntu)

```bash
cd ~
brazil ws create --name NoDonuts --versionSet NoDonuts/development
cd NoDonuts
brazil ws use --package NoDonuts
brazil ws use --platform AL2023_x86_64      # ARM laptop (`uname -m` says aarch64)? Use AL2023_aarch64
cd src/NoDonuts
brazil-build release
mkdir -p ~/.local/bin
install -m 755 build/bin/nodonuts.orig ~/.local/bin/nodonuts
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
nodonuts --version
```

The last line prints `nodonuts 0.1.0`. The build takes a few minutes the
first time.

Install `nodonuts.orig`, not `build/bin/nodonuts`: the latter is a wrapper
that only works inside Brazil.

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
cd ~
brazil ws create --name NoDonuts --versionSet NoDonuts/development
cd NoDonuts
brazil ws use --package NoDonuts
cd src/NoDonuts
cargo build --release
mkdir -p ~/.local/bin
install -m 755 target/release/nodonuts ~/.local/bin/nodonuts
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
nodonuts --version
```

The last line prints `nodonuts 0.1.0`.

- The `curl` line installs Rust. Skip it if you already have Rust.
- `source ~/.cargo/env` makes `cargo` work in this terminal. New
  terminals pick it up on their own.
- The `PATH` line uses `$HOME` rather than `~` on purpose: `~` isn't
  expanded inside double quotes, so `"~/.local/bin"` would not work.
- `cd src/NoDonuts` matters: `Cargo.toml` is there, not in the workspace
  root.
- Brazil can't build for macOS, so the build uses Cargo.

## After installing

```bash
nodonuts enroll
nodonuts config set command 'loginctl lock-session'    # Linux: lock the screen
nodonuts start
nodonuts status
```

On macOS, use `pmset displaysleepnow` as the command instead.

To start NoDonuts at every login:

```bash
nodonuts config set autostart true
```

See [README.md](README.md) for every command and setting.

## Update

Linux:

```bash
cd ~/NoDonuts/src/NoDonuts
git pull
brazil ws sync --metadata
brazil-build release
nodonuts stop
install -m 755 build/bin/nodonuts.orig ~/.local/bin/nodonuts
nodonuts start
```

macOS:

```bash
source ~/.cargo/env
cd ~/NoDonuts/src/NoDonuts
git pull
cargo build --release
nodonuts stop
install -m 755 target/release/nodonuts ~/.local/bin/nodonuts
nodonuts start
```

Your enrolled face and settings are kept.

- Re-enroll (`nodonuts enroll`) only if an update replaces the face model
  (`models/sface.onnx`).
- **Moving the program:** `autostart` stores the program's full path. If you
  move `nodonuts`, run `nodonuts refresh` from its new location.
  `nodonuts status` warns when the stored path is broken.

## Uninstall

```bash
nodonuts stop
nodonuts config set autostart false
rm ~/.local/bin/nodonuts
```

To also delete your settings, enrolled face and log:

```bash
rm -rf ~/.config/nodonuts ~/.local/share/nodonuts         # Linux
rm -rf ~/Library/Application\ Support/nodonuts            # macOS
```

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
| `cargo: command not found` (macOS) | Run `source ~/.cargo/env` |
| `could not find Cargo.toml` | Run `cd ~/NoDonuts/src/NoDonuts` first |
| `brazil ws create` says the workspace exists | You already installed once. Use [Update](#update) |
| A `brazil` command fails with an authentication error | Run `mwinit`, then try again |
| `nodonuts: command not found` | Open a new terminal |

## Where files live

| | Linux | macOS |
|---|---|---|
| Program | `~/.local/bin/nodonuts` | `~/.local/bin/nodonuts` |
| Settings | `~/.config/nodonuts/config.toml` | `~/Library/Application Support/nodonuts/config.toml` |
| Enrolled face | `~/.local/share/nodonuts/owner_face.toml` | `~/Library/Application Support/nodonuts/owner_face.toml` |
| Log | `~/.local/share/nodonuts/nodonuts.log` | `~/Library/Application Support/nodonuts/nodonuts.log` |
| Login entry (`autostart`) | `~/.config/autostart/nodonuts.desktop` | `~/Library/LaunchAgents/com.amazon.nodonuts.plist` |
