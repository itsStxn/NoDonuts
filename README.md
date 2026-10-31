# NoDonuts

Walked away from your desk without locking your laptop? That's donuts for the
team. NoDonuts watches the laptop camera for your face and runs a command of
your choice (lock the screen, suspend, anything in bash) once you've been gone
for a while.

See [INSTALLATION.md](INSTALLATION.md) to build and install it on Linux or
macOS.

## Privacy: no faces are saved

- **No photos or video are ever saved.** Camera frames are looked at in
  memory and thrown away, during enrollment and while watching.
- **What is stored is a face embedding:** 128 numbers that describe your
  face, averaged over the enrollment frames. It is not a picture, and
  NoDonuts has no way to turn it back into one. It is used only to answer
  "is this the owner?".
- **Nothing leaves the laptop.** NoDonuts makes no network connections. The
  face models are built into the binary, so even they are not downloaded.
- **The embedding file is private** (`0600`, readable only by you), and you can
  delete it at any time (see [Where files live](INSTALLATION.md#where-files-live)).

## Quick start

```bash
nodonuts enroll                      # look at the camera for a few seconds
nodonuts config set command 'loginctl lock-session'   # Linux example
nodonuts config set grace_seconds 30
nodonuts start                       # watches in the background
nodonuts status
nodonuts stop
```

macOS lock example: `pmset displaysleepnow`.

## Commands

| Command | Does |
|---|---|
| `nodonuts enroll` | Captures 10 frames of your face and saves the average embedding |
| `nodonuts start` | Starts the background watcher |
| `nodonuts stop` | Stops it |
| `nodonuts status` | Shows whether it is running (and its PID) |
| `nodonuts config show` | Prints all settings |
| `nodonuts config get <key>` | Prints one setting |
| `nodonuts config set <key> <value>` | Validates and saves one setting |
| `nodonuts config reset` | Restores the defaults |

A running watcher keeps the settings it started with. After changing
settings, run `nodonuts stop && nodonuts start`.

## Settings

Stored in `~/.config/nodonuts/config.toml` (Linux) or
`~/Library/Application Support/nodonuts/config.toml` (macOS).

| Key | Default | Meaning |
|---|---|---|
| `grace_seconds` | `30` | How long you may be away before the command runs |
| `command` | `echo 'The owner is gone!'` | Bash command to run (see [How the command runs](#how-the-command-runs)) |
| `match_threshold` | `0.36` | Minimum cosine similarity (0 to 1) to count a face as yours |
| `check_every_ms` | `500` | How often the camera is checked |
| `aggressive_mode` | `false` | `true` = run the command without waiting for the grace period when another face is seen and yours is not |
| `aggressive_checks` | `2` | Aggressive mode fires after this many checks in a row see another face and not yours (1 or more). Counted in checks of `check_every_ms`: the default is 2 x 500 ms, about 1 s |
| `autostart` | `false` | `true` = start the watcher automatically when you log in |

## How the command runs

NoDonuts runs your command with `bash -c "<command>"`, in the background,
and logs its exit code and output. That is a **non-interactive** shell, so it
does not read `~/.bashrc`, `~/.zshrc` or similar files:

- **Aliases and shell functions are not available.** With
  `alias lock='loginctl lock-session'` in your `.bashrc`, the command `lock`
  fails with "command not found" (exit code 127 in the log). Write the full
  command instead (`loginctl lock-session`), or put longer logic in a script
  and call it by its path.
- **Changes your rc files make to `PATH` or other variables do not apply.**
  The command gets the environment `nodonuts start` was run from (or your
  login session's, with `autostart`). Use absolute paths for anything outside
  the standard system folders.

Quoting tip: wrap the command in single quotes when you set it. Inside
double quotes your own shell expands `$`, backticks and `!` before NoDonuts
ever sees them:

```bash
nodonuts config set command 'echo "Gone at $(date +%T)!" >> ~/away.log'
```

## How it behaves

- The command runs once per absence. It re-arms when you're seen again.
- A covered camera, a dark room, a camera error, or a camera in use by
  another app all count as "not seen", so the command fires. This is
  deliberate: it fails toward locking.
- The detector needs a roughly front-facing face. Turning fully sideways or
  sitting with strong light behind you counts as "not seen". Brief turns are
  covered by the grace period.
- Aggressive mode: if a face that isn't yours is in view while yours isn't,
  for `aggressive_checks` checks in a row, the command runs without waiting
  for the grace period.
  - The wait depends on `check_every_ms`: it is
    `aggressive_checks x check_every_ms`. 2 checks at 500 ms is about 1 s; the
    same 2 checks at 2000 ms is about 4 s.
  - "In a row" means any other check resets the count to 0: your face, an
    empty frame, a camera error, or the laptop sleeping in between.
  - If you and someone else are both in view, nothing happens. Camera errors
    never count as a stranger.
  - The grace period keeps running alongside. Whichever finishes first runs
    the command, and the other is cancelled: the command runs once per
    absence either way.
  - Caveat: your own face at a sharp angle or in poor light can score below
    `match_threshold` and look like a stranger. `aggressive_checks` is the
    guard against this. `1` fires on a single misread frame, so keep it at 2
    or more, and higher for destructive commands like `reboot`.
- Autostart: `nodonuts config set autostart true` adds a login entry
  that runs `nodonuts start` (Linux: `~/.config/autostart/nodonuts.desktop`,
  macOS: `~/Library/LaunchAgents/com.amazon.nodonuts.plist`). It starts at
  login rather than at boot, because the camera and your lock command need
  your desktop session. The entry points at the program you ran `config set`
  from, so run it from the installed copy. `nodonuts stop` only stops it until
  the next login; set `autostart false` to turn it off. Nothing
  starts at login if no face is enrolled.
- Closing the lid suspends the watcher. On wake you get a full grace period.
- Clamshell mode (lid closed, external monitor) disables the built-in
  camera, so the command fires. Use an external camera instead.
- No liveness check: a photo of you could fool it.

## Requirements

- Linux: camera access. The logged-in desktop user usually has it; if
  `nodonuts enroll` cannot open the camera, join the `video` group
  (`sudo usermod -aG video $USER`, then log out and in).
- macOS: your terminal app needs camera permission
  (System Settings > Privacy & Security > Camera).

Details for both are in [INSTALLATION.md](INSTALLATION.md#4-camera-access).

## Files and security

The config holds a command that bash runs, so NoDonuts creates the config,
the face file and the login entry with `0600` permissions. It refuses to
start if the config or the face file is writable by others or not owned by
you. The watcher logs to `~/.local/share/nodonuts/nodonuts.log` (Linux) or
`~/Library/Application Support/nodonuts/nodonuts.log` (macOS). The full list
of files is in [INSTALLATION.md](INSTALLATION.md#where-files-live).

## Development

Building, installing the binary onto your `PATH`, updating and uninstalling
are covered in [INSTALLATION.md](INSTALLATION.md).

```bash
brazil-build release        # fmt, clippy (warnings denied), tests, binary
cargo test                  # tests only
brazil-runtime-exec nodonuts --help
```

Code layout (`src/`):

| Module | Role |
|---|---|
| `main.rs`, `cli.rs` | Command-line parsing and dispatch |
| `config.rs` | Load, save, validate settings |
| `camera.rs` | Open the camera, grab frames, reopen after errors |
| `face.rs` | YuNet detection, SFace embeddings, cosine similarity |
| `presence.rs` | Pure state machine, including sleep/wake handling |
| `watcher.rs` | The background loop |
| `runner.rs` | Runs the command without blocking the loop |
| `daemon.rs` | start / stop / status and the PID file |
| `enroll.rs` | Enrollment and the saved face embedding |
| `files.rs` | File locations and private-permission checks |
| `autostart.rs` | The login entry behind the `autostart` setting |

Models (`models/`) are embedded in the binary, both from
[OpenCV Zoo](https://github.com/opencv/opencv_zoo) (Apache-2.0):
`face_detection_yunet_2023mar.onnx` and `face_recognition_sface_2021dec.onnx`.

## Useful links

- [Code Browser](https://code.amazon.com/packages/NoDonuts/)
