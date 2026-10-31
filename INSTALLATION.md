# Installing NoDonuts

NoDonuts is a single binary: both face models are built into it, so
installing it means copying one file onto your `PATH`. This guide covers
building it, installing it for your user on Linux or macOS, updating it, and
uninstalling it.

## 1. Build

The binary must be built for the machine that runs it: an x86_64 build does
not run on an aarch64 (ARM) laptop, and a Linux build does not run on macOS.
Check your machine with `uname -sm`.

### Linux, with Brazil

#### Get the source

Create a workspace on the package's version set (`NoDonuts/development`),
then pull the package into it:

```bash
brazil ws create --name NoDonuts --versionSet NoDonuts/development
cd NoDonuts
brazil ws use --package NoDonuts
```

Pick the platform that matches the laptop you'll run NoDonuts on. The
version set builds `AL2023_x86_64` and `AL2023_aarch64`; on Ubuntu the
workspace otherwise defaults to `AL2_x86_64`, which the version set doesn't
build.

```bash
uname -m                                   # x86_64 or aarch64
brazil ws use --platform AL2023_x86_64     # or: AL2023_aarch64
```

The package source is now in `src/NoDonuts`, a git repository of its own:

```bash
cd src/NoDonuts
```

To pick up newer changes later, from `src/NoDonuts`:

```bash
git pull
brazil ws sync --metadata   # refreshes the version set's dependencies
```

#### Build

From the package directory (`src/NoDonuts`):

```bash
brazil-build release
```

This formats, lints, runs the tests and builds the release binary. The file
to install is:

```
build/bin/nodonuts.orig
```

Copy `nodonuts.orig`, not `build/bin/nodonuts`. The latter is a small wrapper
script that only works inside a Brazil runtime environment.

The binary matches the workspace platform you picked above. To build for the
other architecture, switch platform and rebuild.

### macOS (and Linux without Brazil)

Brazil only builds Linux targets, so on macOS clone the package and use
Cargo directly:

```bash
git clone ssh://git.amazon.com/pkg/NoDonuts
cd NoDonuts
```

```bash
# Rust, if you do not have it: https://rustup.rs
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# macOS needs Apple's command line tools for the camera bindings.
xcode-select --install

cargo build --release
```

The file to install is `target/release/nodonuts`.

> The macOS build has not been tested yet. If it fails, please report it.

## 2. Install

Both platforms install into `~/.local/bin`, a per-user folder that needs no
`sudo`.

### Linux

```bash
mkdir -p ~/.local/bin
install -m 755 build/bin/nodonuts.orig ~/.local/bin/nodonuts   # Brazil build
# or: install -m 755 target/release/nodonuts ~/.local/bin/nodonuts
```

Ubuntu adds `~/.local/bin` to your `PATH` automatically, but only if the
folder existed when you logged in. If `which nodonuts` prints nothing, log
out and back in, or add it to your shell yourself:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc   # zsh: ~/.zshrc
source ~/.bashrc
```

### macOS

```bash
mkdir -p ~/.local/bin
install -m 755 target/release/nodonuts ~/.local/bin/nodonuts
```

macOS does not put `~/.local/bin` on your `PATH`. The default shell is zsh:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

A binary you built yourself runs as is. If you copied it from another Mac
(AirDrop, a download) and macOS blocks it, remove the quarantine flag:

```bash
xattr -d com.apple.quarantine ~/.local/bin/nodonuts
```

### Optional: a short alias

```bash
echo "alias nd='nodonuts'" >> ~/.bashrc   # zsh: ~/.zshrc
```

## 3. Check it works

From any folder:

```bash
which nodonuts      # should print ~/.local/bin/nodonuts
nodonuts --version
nodonuts status
```

## 4. Camera access

### Linux

On a desktop session, the logged-in user normally has camera access already.
If `nodonuts enroll` cannot open the camera, add yourself to the `video`
group, then log out and back in:

```bash
ls -l /dev/video0                 # the camera should be listed
sudo usermod -aG video "$USER"
```

### macOS

The first `nodonuts enroll` asks for camera permission for your terminal app
(Terminal, iTerm2, ...). If you declined, allow it in
System Settings > Privacy & Security > Camera, then quit and reopen the
terminal.

The watcher started by `autostart` runs without a terminal, so macOS may ask
again, or list `nodonuts` separately in that same settings page. This has not
been tested yet.

## 5. First run

```bash
nodonuts enroll                                       # look at the camera
nodonuts config set command 'loginctl lock-session'   # Linux: lock the screen
# macOS: nodonuts config set command 'pmset displaysleepnow'
nodonuts start
```

To start it automatically at every login, run this from the installed copy
(the login entry points at the program you run it from):

```bash
nodonuts config set autostart true
```

The output shows the program the login entry will run. Check that it is
`~/.local/bin/nodonuts`.

See [README.md](README.md) for every command and setting.

## 6. Update

Stop the watcher, so the old version does not keep running, then copy the
new build over the old one:

```bash
nodonuts stop
install -m 755 build/bin/nodonuts.orig ~/.local/bin/nodonuts   # Linux, Brazil build
# or: install -m 755 target/release/nodonuts ~/.local/bin/nodonuts
nodonuts start
```

Your enrolled face and settings live outside the binary and are kept. The
`autostart` login entry keeps working, since the path is unchanged.

### Moving the program or the project

The `autostart` login entry stores the full path of the program it starts.
Moving or deleting that program breaks it silently: nothing starts at login.
Rebuilding or reinstalling in the same place is fine; a new path is not.

- Set autostart from the installed copy in `~/.local/bin`, not from the
  project's `build/bin/`, so moving or deleting the project folder doesn't
  affect it.
- After moving the program, run this from its new location to update the
  entry:

  ```bash
  nodonuts refresh
  ```

- `nodonuts status` shows which program the entry runs, and warns if it no
  longer exists.

Re-enroll (`nodonuts enroll`) only if an update replaces the face
recognition model (`models/sface.onnx`). Faces enrolled with one model do not
match another.

## 7. Uninstall

```bash
nodonuts stop
nodonuts config set autostart false   # removes the login entry
rm ~/.local/bin/nodonuts
```

To also delete your settings, the enrolled face and the log:

```bash
# Linux
rm -rf ~/.config/nodonuts ~/.local/share/nodonuts

# macOS
rm -rf ~/Library/Application\ Support/nodonuts
```

## Where files live

| | Linux | macOS |
|---|---|---|
| Program | `~/.local/bin/nodonuts` | `~/.local/bin/nodonuts` |
| Settings | `~/.config/nodonuts/config.toml` | `~/Library/Application Support/nodonuts/config.toml` |
| Enrolled face | `~/.local/share/nodonuts/owner_face.toml` | `~/Library/Application Support/nodonuts/owner_face.toml` |
| Log | `~/.local/share/nodonuts/nodonuts.log` | `~/Library/Application Support/nodonuts/nodonuts.log` |
| Login entry (`autostart`) | `~/.config/autostart/nodonuts.desktop` | `~/Library/LaunchAgents/com.amazon.nodonuts.plist` |
