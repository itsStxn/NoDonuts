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
  face, one set per enrollment pose (4 in all). It is not a picture, and
  NoDonuts has no way to turn it back into one. It is used only to answer
  "is this the owner?".
- **Nothing leaves the laptop.** NoDonuts makes no network connections. The
  face models are built into the binary, so even they are not downloaded.
- **The embedding file is private** (`0600`, readable only by you), and you can
  delete it at any time with `nodonuts enroll --remove` (see also
  [Where files live](INSTALLATION.md#where-files-live)).

## Quick start

```bash
nodonuts enroll                      # follow the prompts: 4 poses, 10 shots
nodonuts config set command 'loginctl lock-session'   # Linux example
nodonuts config set grace_seconds 30
nodonuts start                       # watches in the background; asks for a fallback password the first time
nodonuts status
nodonuts stop                        # look at the camera (or type the fallback password)
```

macOS lock example: `pmset displaysleepnow`.

## Commands

| Command | Does |
|---|---|
| `nodonuts enroll` | Guides you through 4 poses (10 shots) and saves your face (see [Enrolling](#enrolling)) |
| `nodonuts enroll --remove` | Deletes your enrolled face. `start` then refuses until you enroll again. Only while the watcher is stopped |
| `nodonuts start` | Starts the background watcher. Only one can run: a second `start` (even at the same moment, e.g. with autostart) says it's already watching |
| `nodonuts stop` | Stops it (with `face_checkout` on, it needs your face, or the fallback password) |
| `nodonuts status` | Shows whether it is running (and its PID), the last alert if its file is still there, whether it starts at login, and whether a fallback password is set |
| `nodonuts camera` | Lists the usable cameras and lets you choose one (see [Choosing a camera](#choosing-a-camera)). Only while the watcher is stopped |
| `nodonuts password` | Sets or changes the fallback password (`--remove` to delete it). Only while the watcher is stopped |
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
| `aggressive_checks` | `2` | Aggressive mode fires after this many checks in a row see another face and not yours (1 or more). Counted in checks of `check_every_ms`: the default is 2 x 1000 ms, about 2 s |
| `aggressive_mode` | `false` | `true` = run the command without waiting for the grace period when another face is seen and yours is not |
| `alert_location` | `desktop` | Where `NODONUTS ALERT.txt` goes when your command runs: `desktop`, a folder (`/...` or `~/...`, must exist and be yours), or `off` (no alert and no notification). See [The alert](#the-alert-alert_location) |
| `autostart` | `false` | `true` = start the watcher automatically when you log in |
| `camera` | `built-in` | Which camera to use: `built-in` (the laptop's own) or a camera's name. Choose it with `nodonuts camera`, not `config set` |
| `check_every_ms` | `1000` | How often the camera is checked. Each check costs about 100 ms of CPU, so this sets how busy NoDonuts is: about 10% of one core at 1000, 20% at 500, 6% at 2000. Checking less often also makes aggressive mode and the face checkout slower (see `aggressive_checks` and `face_checks`) |
| `command` | `echo 'The owner is gone!'` | Bash command to run (see [How the command runs](#how-the-command-runs)) |
| `command_once` | `true` | `true` = after the command runs, NoDonuts stops and turns autostart off. **Pair it with a command that locks you out** (see [Run once](#run-once-command_once)) |
| `face_checkout` | `true` | `true` = stopping the watcher needs your face; if you're not seen, the command runs first (see [Face checkout](#face-checkout)) |
| `face_checks` | `2` | `nodonuts stop` asks for the fallback password if you're not recognized within this many camera frames (1 to 10). Like `aggressive_checks`, it's counted in checks of `check_every_ms`: the default is 2 x 1000 ms, about 2 s |
| `grace_seconds` | `30` | How long you may be away before the command runs |
| `logs_maxdays` | `7` | Empty the watcher's log once it's this many days old. `0` keeps it forever. The running watcher does this, checking at start and about once a minute |
| `pwd_ms` | `3000` | How long `nodonuts stop` gives you to type the fallback password, in ms (1 to 60000). Too slow counts as a wrong password. **Keep it small, and your password short enough to type in that time** (see [The password time limit](#the-password-time-limit-pwd_ms)) |
| `threshold_owner` | `0.36` | Minimum similarity (0 to 1) a face needs to count as yours. **Higher is stricter:** closer to 1, strangers are rejected more reliably but your own face is missed more often (bad angle or light). Closer to 0, almost any face passes as you. Called `match_threshold` before; old config files still load |
| `threshold_stranger` | `0.2` | A face below this similarity (0 to 1) counts as a stranger, for aggressive mode. Faces between this and `threshold_owner` are "unsure" (see [How a face is classified](#how-a-face-is-classified)). Can't be higher than `threshold_owner` |

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

### Run once (`command_once`)

> **It is VERY recommended to pair `command_once` (on by default) with a
> command that locks you out**, such as `loginctl lock-session` (Linux),
> `pmset displaysleepnow` (macOS), suspending, logging out or rebooting.
> With a command that doesn't lock, like the default `echo`, NoDonuts stops
> protecting you after your first absence.

With `command_once = true`, once your command has run, NoDonuts:

1. **turns autostart off** (the setting and the login entry), *before* the
   command runs, so a `reboot` can't skip it;
2. runs the command;
3. **stops.**

This applies every time the command runs: the grace period, aggressive
mode, a refused `nodonuts stop`, or a stop signal while you're away.

Why: with a powerful command and autostart, a watcher that stops
recognizing you (new haircut, beard shaved off, a new camera) could reboot
or log you out at every login. With `command_once`, it does it once and
gets out of the way. `nodonuts status` then says it stopped after running
the command. To protect yourself again: `nodonuts start` (and re-enroll if
it no longer recognizes you), plus `nodonuts config set autostart true` if
you use autostart.

The flip side: anyone who sets it off (sitting down at your laptop) is
locked out, but autostart is off afterwards too, until you turn it back on.

With `command_once = false`, the watcher keeps going after the command, and
fires again after your next absence.

## The alert (`alert_location`)

With a lock command, all you see when you come back is a locked screen,
which looks the same as locking it yourself. So whenever your command runs,
NoDonuts also tells you:

- **A file on your desktop, `NODONUTS ALERT.txt`,** with one entry per run:
  ```
  === NoDonuts ran your command ===
  When:       2026-11-03 14:32:07
  Why:        someone else was at the laptop for 2 checks in a row (aggressive mode)
  Command:    loginctl lock-session
  Afterwards: NoDonuts stopped and turned autostart off (command_once). ...
  Full log:   /home/you/.local/share/nodonuts/nodonuts.log
  ```
  New runs are added at the end, so you'll see if it ran three times while
  you were in a meeting. **Delete the file once you've read it.**
- **A desktop notification** ("NODONUTS ALERT"). On Linux it stays on
  screen until dismissed, and GNOME keeps it in its notification list, so
  it's there after you unlock.
- **A line in `nodonuts status`:**
  `Last alert: 2026-11-03 14:32:07, aggressive mode (see .../NODONUTS ALERT.txt)`,
  until you delete the file.

All three happen just *before* the command runs, so a `reboot` or logout
can't skip them.

**Choosing the folder** (while the watcher is stopped, like any setting):

```bash
nodonuts config set alert_location desktop             # the default
nodonuts config set alert_location ~/Documents/alerts  # any folder of yours
nodonuts config set alert_location off                 # no file, no notification
```

- `desktop` is your desktop folder as the system knows it (`~/Desktop`,
  or e.g. `~/Bureau` on a French Ubuntu).
- A folder must be a full path (`/...` or `~/...`), already exist and be
  yours. Relative paths are refused: the watcher doesn't run from your
  terminal's folder, so they'd point somewhere unexpected.
- Changing the folder doesn't move an existing alert file; new alerts go
  to the new folder.
- If the folder is gone when the command runs (e.g. an unplugged drive),
  the alert goes to the NoDonuts data folder (`~/.local/share/nodonuts/` on
  Linux) and the log says so. It's never silently lost.

**Good to know:**

- On Ubuntu, files on the desktop are shown by the Desktop Icons extension,
  which is on by default. If you've turned it off, the file is still in
  `~/Desktop`, just not on screen; the notification and `status` still
  tell you.
- With a command that doesn't lock you out (like the default `echo`),
  whoever is at the laptop can simply delete the file. The alert is meant
  for the lock-out case.
- It says *that* the command ran and why, not who was there: no photos are
  ever taken.

## How it behaves

- The command runs once per absence. With `command_once` (the default),
  NoDonuts then stops and turns autostart off (see
  [Run once](#run-once-command_once)). With `command_once = false`, it keeps
  watching and re-arms when you're seen again.
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
    `aggressive_checks x check_every_ms`. 2 checks at 1000 ms (the default)
    is about 2 s; the same 2 checks at 500 ms is about 1 s.
  - "In a row" means any other check resets the count to 0: your face, an
    empty frame, a camera error, or the laptop sleeping in between.
  - If you and someone else are both in view, nothing happens. Camera errors
    never count as a stranger.
  - The grace period keeps running alongside. Whichever finishes first runs
    the command, and the other is cancelled: the command runs once per
    absence either way.
  - Only clear strangers count. Your own face at a sharp angle or in poor
    light scores lower than usual, but usually still above
    `threshold_stranger`, so it counts as "unsure", not a stranger, and
    doesn't trigger aggressive mode. `aggressive_checks` guards against the
    rest: `1` fires on a single misread frame, so keep it at 2 or more, and
    higher for destructive commands like `reboot`.

### How a face is classified

Each face the camera sees gets a similarity score against your enrolled
face, from 0 (nothing alike) to 1 (identical):

| Score | Counts as | Effect |
|---|---|---|
| `threshold_owner` (0.36) or more | **you** | you're present |
| between the two | **unsure** | not you (the grace period runs), but not a stranger either (no aggressive mode) |
| below `threshold_stranger` (0.2) | **stranger** | counts toward aggressive mode |

`threshold_stranger` can't be higher than `threshold_owner`. Set them equal
to remove the unsure zone, so every face that isn't you counts as a
stranger.
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
  camera, so the command fires. Use an external camera instead (choose it
  with `nodonuts camera`).
- No liveness check: a photo of you could fool it.

## Face checkout

Without it, anyone at your unlocked laptop could type `nodonuts stop` (or
`pkill nodonuts`) during the grace period, and nothing would happen. So it's
on by default: only your face, or your fallback password, can stop the
watcher quietly. To turn it off, run `nodonuts stop` (this stop still needs
your face or password), then `nodonuts config set face_checkout false` and
`nodonuts start`.

**With `nodonuts stop` in your terminal:**

1. The watcher looks at up to `face_checks` camera frames (default 2), one
   every `check_every_ms`, like `aggressive_checks`. So the wait is up to
   `face_checks x check_every_ms`: about 2 s at the defaults.
   `nodonuts stop` shows "Look at the camera...", then
   "Not recognized (1/2)." for each frame that doesn't show you.
2. **You're seen in any of those frames:** it stops quietly.
3. Not seen within `face_checks` frames: it asks for your **fallback password** once (hidden as
   you type, within `pwd_ms`: 3 s by default). **Right password:** it stops
   quietly. Too slow counts as wrong (see [The password time limit](#the-password-time-limit-pwd_ms)).
4. Otherwise (wrong password, no answer, Ctrl+C, or no password set): your
   command runs (e.g. locks the screen). Then:
   - with `command_once` (the default): the watcher stops and turns
     autostart off, and `nodonuts stop` says so;
   - with `command_once = false`: **the watcher keeps watching**.
     `nodonuts stop` says so and exits with an error. Run it again to retry:
     each `nodonuts stop` starts over, with full face checks and one new
     password try, and each failed one runs your command again.

**With anything else** (`kill <pid>`, `pkill nodonuts`, logout, shutdown):
there's nobody to type a password, so the watcher looks once (about 3 s,
whatever `face_checks` and `check_every_ms` are, so a logout isn't held up).
Seen: it stops quietly. Not seen: your command runs, then it stops (and
with `command_once`, autostart is turned off too).

The fallback password covers the times the camera can't confirm you: bad
light, an odd angle, or (on Linux) a video call holding the camera.

- **Setting it:** the first `nodonuts start` with `face_checkout` on asks for
  one, twice to catch typos. It can be anything. Change it any time the
  watcher is stopped with `nodonuts password`, or remove it with
  `nodonuts password --remove`. `nodonuts status` says whether one is set.
- **Autostart can't ask for it** (there's no terminal at login), so if none
  is set, the watcher starts without the fallback and logs a warning.
- **Only a hash is stored,** never the password. It's an Argon2 hash in
  `fallback_checkout.toml`, next to your face file, readable only by you.
  Argon2 is deliberately slow, so guessing from a copy of the file is slow
  too.
- **The watcher checks the password, not `nodonuts stop`.** It loads the hash
  when it starts and keeps it in memory, so changing the file while it runs
  does nothing.
- **Forgot it?** Stop the watcher with your face, or with `kill <pid>`
  (your command runs once, then it stops; `nodonuts status` shows the PID).
  Then set a new password with `nodonuts password`.

### The password time limit (`pwd_ms`)

While the watcher waits for the password, it isn't watching: no camera
checks, and your command can't run. Without a limit, someone at your
laptop could run `nodonuts stop`, leave the prompt open, and use that time
for anything else. So the password must be typed within `pwd_ms`
(3000 ms by default), and the prompt shows the limit:
`Fallback password (3 s):`. Too slow counts as a wrong password: the
command runs.

- **Keep `pwd_ms` small.** The smaller it is, the less free time a
  stranger gets.
- **The smaller `pwd_ms`, the shorter your password needs to be,** so you
  can type it in time. 3 s is enough for a short word or a few characters
  you type from memory. Something like `nodonuts config set pwd_ms 2000`
  with a 4-6 character password works well.
- A short password is fine here: there's only one try per `nodonuts stop`,
  and every wrong or late try runs your command (e.g. locks the screen).
  Guessing it would take many tries, and each one locks the laptop.
- Change it while the watcher is stopped:
  `nodonuts config set pwd_ms 2500`.

- **The watcher does the check, not `nodonuts stop`.** So `kill <pid>`, `pkill
  nodonuts` and `pkill -<almost any signal> nodonuts` all go through it. That
  includes signals that would normally pause it (`pkill -TSTP`). `SIGHUP`
  (closing a terminal) is ignored, as before.
- **What can't be blocked:** `kill -9`, `kill -STOP`, and crash signals such
  as `kill -SEGV`. No program can catch these.
- **`nodonuts stop` never forces its way past the check.** It only
  force-kills a watcher that confirmed a quiet stop (your face, your
  password, or `face_checkout` off) and then got stuck on its way out. If
  the watcher doesn't answer, `nodonuts stop` leaves it running and says so.
- **It also happens on logout and shutdown.** The system asks every program
  to stop, so if you're not in front of the camera then, your command runs.
  Logout waits for it: the face checkout, plus up to 5 s for your command.
- **A camera that can't see you counts as "not seen".** A covered camera, a
  dark room, or (on Linux) a camera busy in a video call means stopping needs
  the fallback password, or runs your command. That's harmless with a lock
  command, but think twice with something like `reboot`.
- Changing settings needs your face too, since it means stopping the watcher
  first.

## Enrolling

`nodonuts enroll` guides you through the poses you'll actually be in while
working, so you're recognized in them too. Press Enter before each pose; a
short pause before each shot lets you settle into its variation:

| Pose | Shots |
|---|---|
| 1. Straight at the camera | neutral; smile; a little closer; a little further back |
| 2. Head turned a little to your left | about 20°; about 30° |
| 3. Head turned a little to your right | about 20°; about 30° |
| 4. Head down, as if reading the keyboard | at the keyboard; at the bottom of the screen |

- **Live feedback.** If a shot doesn't work, it says why and tries again:
  no face found (in a turned pose: "too far, turn back a little"), face
  too small ("move a little closer"), or a face that doesn't match the
  first shots ("make sure only you are in front of the camera").
- **One description per pose is saved** (4 sets of 128 numbers, never a
  picture). A face counts as you if it matches any pose, so a turned head
  is matched against your turned poses rather than your straight one.
- Turn only partway (both eyes visible): past about 45°, the camera
  software can't find a face at all.
- Face files from before guided poses still work, as a single pose; the
  log suggests re-enrolling.
- Enrollment uses the camera chosen with `nodonuts camera`, and only works
  while the watcher is stopped.

## Choosing a camera

NoDonuts uses the laptop's built-in camera unless you choose another one:

```
$ nodonuts camera
Cameras:
  1. HP 5MP Camera: HP 5MP Camera   (built-in, current)
  2. Logitech C920
Choose a camera [1-2], or press Enter to keep the current one:
```

- **Only usable cameras are listed.** Infrared cameras (black and white,
  used for face login) and the extra metadata devices Linux shows for each
  camera are left out, since the face models need a color picture.
- **The built-in camera:** on Linux, it's the one on an internal port. On
  macOS, it's found by name (e.g. "FaceTime"), and an iPhone used as a
  webcam is never picked by default. If neither applies, the first camera
  is used.
- **What's saved:** the built-in camera is saved as `built-in`; any other is
  saved by name, so plugging cameras in or out doesn't change which one
  you chose.
- **If the chosen camera is missing** (e.g. an unplugged webcam), the
  watcher uses the built-in one instead and logs a warning, so you stay
  protected.
- **`nodonuts enroll` uses the chosen camera too.** After switching
  cameras, re-enroll: a different lens and angle can make your face harder
  to recognize.
- Two connected cameras with exactly the same name can't be told apart:
  the first one is used.

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
`~/Library/Application Support/nodonuts/nodonuts.log` (macOS), and empties
it every `logs_maxdays` days (7 by default). The full list of files is in
[INSTALLATION.md](INSTALLATION.md#where-files-live).

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
| `logs.rs` | Empties the log once it's older than `logs_maxdays` |
| `alert.rs` | The alert when the command runs: `NODONUTS ALERT.txt`, the notification, the `status` line |
| `autostart.rs` | The login entry behind the `autostart` setting |
| `control.rs` | The conversation between `nodonuts stop` and the watcher, over a local socket |
| `password.rs` | The fallback password: Argon2 hash, save, load, verify |
| `prompt.rs` | The hidden password prompt |

Models (`models/`) are embedded in the binary, both from
[OpenCV Zoo](https://github.com/opencv/opencv_zoo) (Apache-2.0):
`face_detection_yunet_2023mar.onnx` and `face_recognition_sface_2021dec.onnx`.
