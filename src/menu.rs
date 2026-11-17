//! A small checkbox menu in the terminal, for `nodonuts alerts`:
//! move with the arrow keys, tick with space, confirm with Enter.
//!
//! The terminal normally hands a program whole lines, after Enter. To react
//! to single key presses, the menu switches the terminal to "raw" mode while
//! it runs (no line buffering, no echo) and always switches it back, even
//! on an error.
//!
//! What a key press does ([`Menu::press`]) and how bytes become keys
//! ([`parse_key`]) are plain functions, so they're unit tested without a
//! terminal.

use anyhow::{Context, Result, bail};
use nix::sys::termios::{self, LocalFlags, SetArg, SpecialCharacterIndices, Termios};
use signal_hook::consts::{SIGHUP, SIGTERM};
use signal_hook::iterator::Signals;
use std::io::{self, ErrorKind, IsTerminal, Read, Write};
use std::thread;

/// A key press, as far as the menu cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Move up.
    Up,
    /// Move down.
    Down,
    /// Tick or untick the highlighted line.
    Toggle,
    /// Tick or untick line number `n`, counting from 0 (the key `1` is 0).
    Number(usize),
    /// Save the choice.
    Confirm,
    /// Leave without saving.
    Cancel,
    /// Anything else: ignored.
    Other,
}

/// Turns the bytes of one key press into a [`Key`]. Arrow keys arrive as
/// three bytes (`ESC [ A`), plain keys as one.
pub fn parse_key(bytes: &[u8]) -> Key {
    match bytes {
        b"\x1b[A" | b"\x1bOA" | b"k" => Key::Up,
        b"\x1b[B" | b"\x1bOB" | b"j" => Key::Down,
        b" " => Key::Toggle,
        b"\r" | b"\n" => Key::Confirm,
        // A lone Escape, `q`, Ctrl+C, Ctrl+D.
        b"\x1b" | b"q" | b"\x03" | b"\x04" => Key::Cancel,
        [digit @ b'1'..=b'9'] => Key::Number(usize::from(digit - b'1')),
        _ => Key::Other,
    }
}

/// What a key press led to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Keep going.
    Continue,
    /// The owner confirmed.
    Done,
    /// The owner left without saving.
    Cancelled,
}

/// The lines of a menu, which ones are ticked, and where the cursor is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    labels: Vec<String>,
    checked: Vec<bool>,
    cursor: usize,
}

impl Menu {
    /// A menu from `(label, ticked)` pairs. The cursor starts on the first.
    pub fn new(items: &[(&str, bool)]) -> Menu {
        Menu {
            labels: items.iter().map(|(label, _)| label.to_string()).collect(),
            checked: items.iter().map(|(_, checked)| *checked).collect(),
            cursor: 0,
        }
    }

    /// Which lines are ticked, in order.
    pub fn checked(&self) -> &[bool] {
        &self.checked
    }

    /// Applies one key press.
    pub fn press(&mut self, key: Key) -> Step {
        let count = self.labels.len();
        match key {
            // Wrap around at the ends.
            Key::Up => self.cursor = (self.cursor + count - 1) % count,
            Key::Down => self.cursor = (self.cursor + 1) % count,
            Key::Toggle => self.checked[self.cursor] = !self.checked[self.cursor],
            Key::Number(line) if line < count => {
                self.cursor = line;
                self.checked[line] = !self.checked[line];
            }
            Key::Confirm => return Step::Done,
            Key::Cancel => return Step::Cancelled,
            Key::Number(_) | Key::Other => {}
        }
        Step::Continue
    }

    /// The menu as text lines: `> [x] label` for the highlighted line, and
    /// `  [ ] label` for the others.
    pub fn lines(&self) -> Vec<String> {
        self.labels
            .iter()
            .enumerate()
            .map(|(line, label)| {
                let pointer = if line == self.cursor { ">" } else { " " };
                let tick = if self.checked[line] { "x" } else { " " };
                format!("{pointer} [{tick}] {label}")
            })
            .collect()
    }
}

/// The terminal in raw mode, until dropped (which puts it back, whatever
/// happened in between).
struct RawMode {
    original: Termios,
}

impl RawMode {
    fn enter() -> Result<RawMode> {
        let stdin = io::stdin();
        let original =
            termios::tcgetattr(&stdin).context("could not read the terminal settings")?;
        let mut raw = original.clone();
        // No line buffering, no echo, and Ctrl+C arrives as a key (handled
        // as "cancel") instead of killing the program with the terminal in
        // raw mode.
        raw.local_flags
            .remove(LocalFlags::ICANON | LocalFlags::ECHO | LocalFlags::ISIG);
        // `read` returns as soon as one byte is there.
        raw.control_chars[SpecialCharacterIndices::VMIN as usize] = 1;
        raw.control_chars[SpecialCharacterIndices::VTIME as usize] = 0;
        termios::tcsetattr(&stdin, SetArg::TCSANOW, &raw)
            .context("could not switch the terminal to raw mode")?;
        Ok(RawMode { original })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = termios::tcsetattr(io::stdin(), SetArg::TCSANOW, &self.original);
    }
}

/// Reads one key press. End of input counts as cancel.
fn read_key() -> Result<Key> {
    let mut buffer = [0u8; 8];
    match io::stdin().lock().read(&mut buffer) {
        Ok(0) => Ok(Key::Cancel),
        Ok(read) => Ok(parse_key(&buffer[..read])),
        // A signal (e.g. the terminal being resized) interrupted the read.
        Err(error) if error.kind() == ErrorKind::Interrupted => Ok(Key::Other),
        Err(error) => Err(error).context("could not read a key"),
    }
}

/// Prints `menu` (after the `header` lines) and lets the owner change it,
/// redrawing it in place after every key. Returns true if they confirmed
/// with Enter, false if they cancelled; the ticks are in `menu` either way.
pub fn run(header: &[&str], menu: &mut Menu) -> Result<bool> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("this is interactive: run it in a terminal");
    }
    let raw = RawMode::enter()?;

    // A kill or a closed terminal can't run `drop`, so a helper thread puts
    // the terminal back for those (like the password prompt does).
    let mut signals =
        Signals::new([SIGTERM, SIGHUP]).context("could not watch for a kill signal")?;
    let signal_handle = signals.handle();
    let saved = raw.original.clone();
    let watcher = thread::spawn(move || {
        if signals.forever().next().is_some() {
            let _ = termios::tcsetattr(io::stdin(), SetArg::TCSANOW, &saved);
            println!();
            std::process::exit(130);
        }
    });

    let result = interact(header, menu);

    signal_handle.close();
    let _ = watcher.join();
    drop(raw); // terminal back to normal before anything else is printed
    result
}

/// The draw-read-apply loop of [`run`].
fn interact(header: &[&str], menu: &mut Menu) -> Result<bool> {
    let mut out = io::stdout();
    for line in header {
        write!(out, "{line}\r\n")?;
    }
    let height = menu.lines().len();
    let mut first_draw = true;
    loop {
        if !first_draw {
            // Back up over the menu and draw it again.
            write!(out, "\x1b[{height}A")?;
        }
        first_draw = false;
        for line in menu.lines() {
            // `\x1b[2K` clears the line first.
            write!(out, "\r\x1b[2K{line}\r\n")?;
        }
        out.flush()?;
        match menu.press(read_key()?) {
            Step::Continue => {}
            Step::Done => return Ok(true),
            Step::Cancelled => return Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_lines() -> Menu {
        Menu::new(&[("first", true), ("second", false)])
    }

    #[test]
    fn keys_are_recognized() {
        assert_eq!(parse_key(b"\x1b[A"), Key::Up);
        assert_eq!(parse_key(b"\x1b[B"), Key::Down);
        assert_eq!(parse_key(b"\x1bOA"), Key::Up, "some terminals");
        assert_eq!(parse_key(b"k"), Key::Up);
        assert_eq!(parse_key(b"j"), Key::Down);
        assert_eq!(parse_key(b" "), Key::Toggle);
        assert_eq!(parse_key(b"\r"), Key::Confirm);
        assert_eq!(parse_key(b"\n"), Key::Confirm);
        assert_eq!(parse_key(b"1"), Key::Number(0));
        assert_eq!(parse_key(b"2"), Key::Number(1));
        for cancel in [&b"q"[..], b"\x1b", b"\x03", b"\x04"] {
            assert_eq!(parse_key(cancel), Key::Cancel, "{cancel:?}");
        }
        for other in [&b"x"[..], b"0", b"\x1b[C", b"\x1b[1;5A", b""] {
            assert_eq!(parse_key(other), Key::Other, "{other:?}");
        }
    }

    #[test]
    fn space_ticks_the_highlighted_line() {
        let mut menu = two_lines();
        assert_eq!(menu.press(Key::Toggle), Step::Continue);
        assert_eq!(menu.checked(), [false, false]);
        menu.press(Key::Down);
        menu.press(Key::Toggle);
        assert_eq!(menu.checked(), [false, true]);
        menu.press(Key::Toggle);
        assert_eq!(menu.checked(), [false, false]);
    }

    #[test]
    fn both_lines_can_be_ticked() {
        let mut menu = Menu::new(&[("a", false), ("b", false)]);
        menu.press(Key::Toggle);
        menu.press(Key::Down);
        menu.press(Key::Toggle);
        assert_eq!(menu.checked(), [true, true]);
        assert_eq!(menu.press(Key::Confirm), Step::Done);
    }

    #[test]
    fn the_cursor_wraps_around() {
        let mut menu = two_lines();
        menu.press(Key::Up);
        assert!(
            menu.lines()[1].starts_with('>'),
            "up from the top: last line"
        );
        menu.press(Key::Down);
        assert!(menu.lines()[0].starts_with('>'), "down from the end: first");
    }

    #[test]
    fn a_number_key_ticks_that_line_and_moves_there() {
        let mut menu = two_lines();
        menu.press(Key::Number(1));
        assert_eq!(menu.checked(), [true, true]);
        assert!(menu.lines()[1].starts_with('>'));
        // A number beyond the menu does nothing.
        let before = menu.clone();
        assert_eq!(menu.press(Key::Number(5)), Step::Continue);
        assert_eq!(menu, before);
    }

    #[test]
    fn enter_confirms_and_cancel_keeps_nothing_decided() {
        assert_eq!(two_lines().press(Key::Confirm), Step::Done);
        assert_eq!(two_lines().press(Key::Cancel), Step::Cancelled);
        assert_eq!(two_lines().press(Key::Other), Step::Continue);
    }

    #[test]
    fn lines_show_the_ticks_and_the_cursor() {
        let mut menu = two_lines();
        assert_eq!(menu.lines(), ["> [x] first", "  [ ] second"]);
        menu.press(Key::Down);
        assert_eq!(menu.lines(), ["  [x] first", "> [ ] second"]);
    }
}
