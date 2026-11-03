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
| `nodonuts stop` | Stops it (with `face_checkout` on, it needs your face) |
| `nodonuts status` | Shows whether it is running (and its PID), and whether it starts at login |
| `nodonuts refresh` | Updates the login entry to match `autostart`, pointing at the copy you run it from. Only while the watcher is stopped |
| `nodonuts config show` | Prints all settings |
| `nodonuts config get <key>` | Prints one setting |
| `nodonuts config set <key> <value>` | Validates and saves one setting |
| `nodonuts config reset` | Restores the defaults |

Settings can only be changed while the watcher is stopped: `config set` and
`config reset` refuse while it runs, like `enroll` does. Reading them
(`config show`, `config get`) always works. To change a setting:
`nodonuts stop`, `nodonuts config set ...`, `nodonuts start`.

## Settings

Stored in `~/.config/nodonuts/config.toml` (Linux) or
`~/Library/Application Support/nodonuts/config.toml` (macOS).

| Key | Default | Meaning |
|---|---|---|
| `aggressive_checks` | `2` | Aggressive mode fires after this many checks in a row see another face and not yours (1 or more). Counted in checks of `check_every_ms`: the default is 2 x 500 ms, about 1 s |
| `aggressive_mode` | `false` | `true` = run the command without waiting for the grace period when another face is seen and yours is not |
| `autostart` | `false` | `true` = start the watcher automatically when you log in |
| `check_every_ms` | `500` | How often the camera is checked |
| `command` | `echo 'The owner is gone!'` | Bash command to run (see [How the command runs](#how-the-command-runs)) |
| `face_checkout` | `true` | `true` = stopping the watcher needs your face; if you're not seen, the command runs first (see [Face checkout](#face-checkout)) |
| `grace_seconds` | `30` | How long you may be away before the command runs |
| `match_threshold` | `0.36` | Minimum similarity (0 to 1) a face needs to count as yours. **Higher is stricter:** closer to 1, strangers are rejected more reliably but your own face is missed more often (bad angle or light). Closer to 0, almost any face passes as you. Faces below it count as strangers in aggressive mode |

## How the command runs

NoDonuts runs your command with `bash -c "<command>"` and logs its exit code
and output.

**The camera is free while your command runs.** NoDonuts closes the camera,
waits for the command to finish, then reopens it and carries on watching. So
your command can use the camera itself, e.g. a script that takes a photo of
whoever is at your laptop. This matters on Linux, where only one program can
use the camera at a time.

- No checks happen while the command runs. That's fine: it only runs once
  per absence, and watching picks up again (to re-arm when you're back) as
  soon as it's done.
- A command that never finishes pauses watching for good, so make sure your
  script exits. To start something that keeps running without holding
  NoDonuts up, send it to the background with its output redirected:
  `my-long-thing >/dev/null 2>&1 &`. Without the redirect, NoDonuts still
  waits for it, because it waits for the command's output to close too.
- `nodonuts stop` still works while a command runs. NoDonuts stops waiting,
  and it won't start a second copy of a command that's still running.

It's a **non-interactive** shell, so it doesn't read `~/.bashrc`, `~/.zshrc`
or similar files:

- **Aliases and shell functions are not available.** With
  `alias lock='loginctl lock-session'` in your `.bashrc`, the command `lock`
  fails with "command not found" (exit code 127 in the log). Write the full
  command instead (`loginctl lock-session`), or put longer logic in a script
  and call it by its path.
- **Changes your rc files make to `PATH` or other variables do not apply.**
  The command gets the environment `nodonuts start` was run from (or your
  login session's, with `autostart`). Use absolute paths for anything outside
  the standard system folders.

### Quoting: single quotes outside, double quotes inside

Wrap the command in **single quotes** (`'...'`) when you set it, and use
**double quotes** (`"..."`) for anything inside it:

```bash
nodonuts config set command 'echo "some stuff"'
nodonuts config set command 'echo "Gone at $(date +%T)!" >> ~/away.log'
```

- **Single quotes keep the command exactly as you typed it.** Your shell
  doesn't touch anything between them, so `$`, `$(...)`, backticks and `!`
  are saved as-is and run later, when the command fires. In the second
  example, `$(date +%T)` gives the time you left, not the time you ran
  `config set`.
- **Double quotes inside work as usual,** because a `"` inside `'...'` is
  just a normal character. Use them for text with spaces, like the message
  above.
- **Avoid double quotes on the outside.** Your shell expands `$` and
  backticks inside them right away, and `!` can fail with
  `event not found`. So `"echo 'Gone at $(date)'"` would save a fixed time.
- **A single quote can't go inside single quotes.** If you need one, as in
  `It's`, write `'\''` in its place:
  `'echo "It'\''s me"'` saves `echo "It's me"`. Usually it's simpler to
  rephrase, or to move the logic into a script.

Check what was saved with `nodonuts config get command`.

**For anything longer, use a script.** Put the steps in a file, make it
executable (`chmod +x`), and set its full path as the command. No quoting
to worry about, and you can test it on its own:

```bash
nodonuts config set command '/home/me/scripts/away.sh'
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
  your desktop session. `nodonuts stop` only stops it until the next login;
  set `autostart false` to turn it off. Nothing starts at login if no face is
  enrolled.
  - **The entry stores the full path of the program you ran `config set`
    from.** If that program moves or is deleted, nothing starts at login, and
    nothing tells you at login time, since there is no terminal. This is easy
    to hit by setting autostart from `build/bin/` and later moving, renaming
    or deleting the project folder. Set it from the installed copy
    (`~/.local/bin/nodonuts`, see [INSTALLATION.md](INSTALLATION.md)) and
    moving the project doesn't matter.
  - `nodonuts status` shows the program the entry runs, and warns if it no
    longer exists. To fix it, run `nodonuts refresh` from the program's new
    location.
  - Only `config set autostart` and `nodonuts refresh` change the entry, and
    both refuse while the watcher is running.
    `nodonuts start` never does, so starting a test copy from `build/bin/`
    can't redirect autostart to it. `start` only warns when the entry is out
    of date: missing, pointing at a deleted program, or not matching the
    `autostart` setting after a hand edit of `config.toml`.
- Face checkout: see [below](#face-checkout).
- Closing the lid suspends the watcher. On wake you get a full grace period.
- Clamshell mode (lid closed, external monitor) disables the built-in
  camera, so the command fires. Use an external camera instead.
- No liveness check: a photo of you could fool it.

## Face checkout

Without it, anyone at your unlocked laptop could type `nodonuts stop` (or
`pkill nodonuts`) during the grace period, and nothing would happen. So it's
on by default: only your face can stop the watcher quietly. To turn it off,
run `nodonuts stop` (this stop still needs your face), then
`nodonuts config set face_checkout false` and `nodonuts start`.

- When the watcher is asked to stop, it looks at the camera for about 3
  seconds.
  - **You're seen:** it stops quietly.
  - **You're not seen:** it runs your command (e.g. locks the screen), then
    stops.
- `nodonuts stop` prints "look at the camera...", then which of the two
  happened.
- **The watcher does the check, not `nodonuts stop`.** So `kill <pid>`, `pkill
  nodonuts` and `pkill -<almost any signal> nodonuts` all go through it. That
  includes signals that would normally pause it (`pkill -TSTP`). `SIGHUP`
  (closing a terminal) is ignored, as before.
- **What can't be blocked:** `kill -9`, `kill -STOP`, and crash signals such
  as `kill -SEGV`. No program can catch these.
- **It also happens on logout and shutdown.** The system asks every program
  to stop, so if you're not in front of the camera then, your command runs.
  Logout waits for it: the face checkout, plus up to 5 s for your command.
- **A camera that can't see you counts as "not seen".** A covered camera, a
  dark room, or (on Linux) a camera busy in a video call means stopping runs
  your command. That's harmless with a lock command, but think twice with
  something like `reboot`.
- Changing settings needs your face too, since it means stopping the watcher
  first.

## Requirements

- Linux: camera access. The logged-in desktop user usually has it; if
  `nodonuts enroll` cannot open the camera, join the `video` group
  (`sudo usermod -aG video $USER`, then log out and in).
- macOS: your terminal app needs camera permission
  (System Settings > Privacy & Security > Camera).

Details for both are in [INSTALLATION.md](INSTALLATION.md#camera-access).

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
