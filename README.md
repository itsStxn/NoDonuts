# NoDonuts

Walked away from your desk without locking your laptop? That's donuts for the
team. NoDonuts watches the laptop camera for your face and runs a command of
your choice (lock the screen, suspend, anything in bash) once you've been gone
for a while.

Everything runs offline. No image ever leaves the laptop, and only a face
embedding (128 numbers, never a photo) is stored.

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
| `command` | `echo 'The owner is gone!'` | Bash command to run (`bash -c`) |
| `match_threshold` | `0.36` | Minimum cosine similarity (0 to 1) to count a face as yours |
| `check_every_ms` | `500` | How often the camera is checked |
| `aggressive_mode` | `false` | `true` = run the command instantly, skipping the grace period, when another face is seen and yours is not |

## How it behaves

- The command runs once per absence. It re-arms when you're seen again.
- A covered camera, a dark room, a camera error, or a camera in use by
  another app all count as "not seen", so the command fires. This is
  deliberate: it fails toward locking.
- The detector needs a roughly front-facing face. Turning fully sideways or
  sitting with strong light behind you counts as "not seen". Brief turns are
  covered by the grace period.
- Aggressive mode: if a face that isn't yours is in view while yours isn't,
  the command runs at once. If you and someone else are both in view, nothing
  happens. Camera errors never count as a stranger. Caveat: your own face at
  a sharp angle or in poor light can score below `match_threshold` and be
  treated as a stranger, so this mode can fire while you're at your desk.
- Closing the lid suspends the watcher. On wake you get a full grace period.
- Clamshell mode (lid closed, external monitor) disables the built-in
  camera, so the command fires. Use an external camera instead.
- No liveness check: a photo of you could fool it.

## Requirements

- Linux: your user must be in the `video` group (`sudo usermod -aG video $USER`).
- macOS: your terminal app needs camera permission
  (System Settings > Privacy & Security > Camera).

## Files and security

The config holds a command that bash runs, so NoDonuts creates the config and
the face file with `0600` permissions. It refuses to start if either is
writable by others or not owned by you. The watcher logs to
`~/.local/share/nodonuts/nodonuts.log` (Linux).

## Development

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

Models (`models/`) are embedded in the binary, both from
[OpenCV Zoo](https://github.com/opencv/opencv_zoo) (Apache-2.0):
`face_detection_yunet_2023mar.onnx` and `face_recognition_sface_2021dec.onnx`.

## Useful links

- [Code Browser](https://code.amazon.com/packages/NoDonuts/)
