//! Input on macOS, always in the background: runs the exact `xdotool`
//! command lines `server.rs` builds without touching the user's pointer,
//! keyboard focus or frontmost app.
//!
//! - `type` / `key`: CGEvents posted to the agent's target app process
//!   (`CGEventPostToPid`), so they reach that app while another one is in
//!   front. Text goes in as Unicode key events, independent of the keyboard
//!   layout; chords map xdotool keysyms to ANSI key codes (`super` is
//!   Command).
//! - `click` 1/3 at the last `mousemove` point: an AX action on the target
//!   app's element under that point (`ax_tree::click_at`).
//! - `click` 4-7 (wheel): moves the scroll bar of the area under the point.
//! - `mousedown` / `mouseup` (drags) cannot be done without the pointer and
//!   are refused.
//!
//! Coordinates are global screen points (top-left origin), the space AX
//! bounds and screenshots use.

use crate::macos::ax_tree;
use objc2_application_services::AXIsProcessTrusted;
use objc2_core_graphics::{CGEvent, CGEventFlags, CGKeyCode};
use std::{thread::sleep, time::Duration};

/// Pixels one wheel "click" scrolls; xdotool's 5 clicks are one page step.
const PIXELS_PER_WHEEL_CLICK: f64 = 100.0;

/// Runs one xdotool argument vector. Errors name the token that failed.
pub(crate) fn run(args: &[String]) -> Result<(), String> {
    if !unsafe { AXIsProcessTrusted() } {
        return Err(
            "Accessibility permission missing: macOS drops synthetic input from this binary"
                .to_string(),
        );
    }
    let target = ax_tree::target_pid()
        .ok_or("no target app: call get_app_state or act on an element first")?;
    eprintln!(
        "input: {} -> target pid {target}, frontmost pid {:?}",
        args.first().map(String::as_str).unwrap_or_default(),
        ax_tree::frontmost_pid()
    );
    let mut at: Option<(f64, f64)> = None;
    let mut tokens = args.iter().map(String::as_str).peekable();
    while let Some(command) = tokens.next() {
        let mut options = Options::default();
        while let Some(option) = tokens.peek().copied().filter(|t| t.starts_with("--")) {
            tokens.next();
            match option {
                "--" => break,
                "--delay" => options.delay_ms = number(tokens.next(), option)?,
                "--repeat" => options.repeat = number(tokens.next(), option)?,
                "--clearmodifiers" | "--sync" => {}
                other => return Err(format!("unsupported xdotool option {other}")),
            }
        }
        match command {
            "mousemove" => {
                let x: f64 = number(tokens.next(), "mousemove x")?;
                let y: f64 = number(tokens.next(), "mousemove y")?;
                at = Some((x, y));
            }
            "click" => {
                let button: u32 = number(tokens.next(), "click button")?;
                let (x, y) = at.ok_or("a background click needs a point (x and y)")?;
                click(x, y, button, options)?;
            }
            "mousedown" | "mouseup" => {
                return Err(
                    "drag needs the real pointer, which background mode never takes on macOS; use element actions instead"
                        .to_string(),
                );
            }
            "sleep" => {
                let seconds: f64 = number(tokens.next(), "sleep")?;
                sleep(Duration::from_secs_f64(seconds.max(0.0)));
            }
            "type" => {
                let text = tokens.next().ok_or("type needs text")?;
                type_text(target, text, options.delay_ms)?;
            }
            "key" => {
                let spec = tokens.next().ok_or("key needs a key")?;
                key_chord(target, spec)?;
            }
            other => return Err(format!("unsupported xdotool command {other}")),
        }
    }
    Ok(())
}

#[derive(Default, Clone, Copy)]
struct Options {
    delay_ms: u64,
    repeat: u32,
}

fn number<T: std::str::FromStr>(token: Option<&str>, what: &str) -> Result<T, String> {
    token
        .and_then(|token| token.parse().ok())
        .ok_or_else(|| format!("{what} needs a number"))
}

/// X11 buttons: 1 left, 3 right as AX actions; 4 up, 5 down, 6 left,
/// 7 right as scroll-bar moves.
fn click(x: f64, y: f64, button: u32, options: Options) -> Result<(), String> {
    let repeat = options.repeat.max(1);
    let wheel = |right: f64, down: f64| -> Result<(), String> {
        for _ in 0..repeat {
            ax_tree::scroll_at(x, y, right, down)?;
            sleep(Duration::from_millis(options.delay_ms));
        }
        Ok(())
    };
    let kind = if button <= 3 {
        crate::macos::cursor::Kind::Click
    } else {
        crate::macos::cursor::Kind::Move
    };
    crate::macos::cursor::show(x, y, kind);
    match button {
        4 => wheel(0.0, -PIXELS_PER_WHEEL_CLICK),
        5 => wheel(0.0, PIXELS_PER_WHEEL_CLICK),
        6 => wheel(-PIXELS_PER_WHEEL_CLICK, 0.0),
        7 => wheel(PIXELS_PER_WHEEL_CLICK, 0.0),
        _ => {
            let done = ax_tree::click_at(x, y, button, repeat)?;
            eprintln!("input: {done}");
            Ok(())
        }
    }
}

/// Types `text` as Unicode key events; CR is the Return key and tab the Tab
/// key, as `xdotool type` sends them.
fn type_text(pid: u32, text: &str, delay_ms: u64) -> Result<(), String> {
    for character in text.chars() {
        match character {
            '\r' | '\n' => tap(pid, KEY_RETURN, CGEventFlags(0))?,
            '\t' => tap(pid, KEY_TAB, CGEventFlags(0))?,
            other => {
                let mut units = [0u16; 2];
                let units = other.encode_utf16(&mut units);
                // The ANSI key (and Shift) for characters that have one, so
                // apps reading key codes (Calculator, games, shortcuts) get
                // the key too; the Unicode string carries the character for
                // text fields on any layout.
                let (code, shift) = ansi_character(other).unwrap_or((0, false));
                let flags = if shift {
                    CGEventFlags::MaskShift
                } else {
                    CGEventFlags(0)
                };
                for down in [true, false] {
                    let event = CGEvent::new_keyboard_event(None, code, down)
                        .ok_or("CGEventCreateKeyboardEvent failed")?;
                    CGEvent::set_flags(Some(&event), flags);
                    unsafe {
                        CGEvent::keyboard_set_unicode_string(
                            Some(&event),
                            units.len() as _,
                            units.as_ptr(),
                        )
                    };
                    CGEvent::post_to_pid(pid as libc::pid_t, Some(&event));
                }
            }
        }
        sleep(Duration::from_millis(delay_ms));
    }
    Ok(())
}

/// One `xdotool key` spec such as `ctrl+a`, `super+v`, `Return` or a bare
/// modifier.
fn key_chord(pid: u32, spec: &str) -> Result<(), String> {
    let parts: Vec<&str> = spec.split('+').collect();
    let (key, modifiers) = parts.split_last().ok_or("empty key")?;
    let mut flags = 0u64;
    for modifier in modifiers {
        flags |= modifier_key(modifier)
            .ok_or_else(|| format!("unsupported modifier {modifier}"))?
            .1
             .0;
    }
    if let Some((code, _)) = modifier_key(key).filter(|_| modifiers.is_empty()) {
        return tap(pid, code, CGEventFlags(0));
    }
    let (code, shifted) = key_code(key).ok_or_else(|| format!("unsupported key {key}"))?;
    if shifted {
        flags |= CGEventFlags::MaskShift.0;
    }
    tap(pid, code, CGEventFlags(flags))
}

fn tap(pid: u32, code: CGKeyCode, flags: CGEventFlags) -> Result<(), String> {
    for down in [true, false] {
        let event = CGEvent::new_keyboard_event(None, code, down)
            .ok_or("CGEventCreateKeyboardEvent failed")?;
        CGEvent::set_flags(Some(&event), flags);
        CGEvent::post_to_pid(pid as libc::pid_t, Some(&event));
    }
    Ok(())
}

const KEY_RETURN: CGKeyCode = 36;
const KEY_TAB: CGKeyCode = 48;

/// Key code and flag of an xdotool modifier name.
fn modifier_key(name: &str) -> Option<(CGKeyCode, CGEventFlags)> {
    Some(match name {
        "ctrl" => (59, CGEventFlags::MaskControl),
        "alt" => (58, CGEventFlags::MaskAlternate),
        "shift" => (56, CGEventFlags::MaskShift),
        "super" => (55, CGEventFlags::MaskCommand),
        _ => return None,
    })
}

/// ANSI virtual key code of an xdotool keysym, and whether it implies Shift
/// (an uppercase letter).
fn key_code(keysym: &str) -> Option<(CGKeyCode, bool)> {
    let named = match keysym {
        "Return" => 36,
        "Tab" => 48,
        "space" => 49,
        "BackSpace" => 51,
        "Escape" => 53,
        "Delete" => 117,
        "Home" => 115,
        "End" => 119,
        "Page_Up" => 116,
        "Page_Down" => 121,
        "Left" => 123,
        "Right" => 124,
        "Down" => 125,
        "Up" => 126,
        "F1" => 122,
        "F2" => 120,
        "F3" => 99,
        "F4" => 118,
        "F5" => 96,
        "F6" => 97,
        "F7" => 98,
        "F8" => 100,
        "F9" => 101,
        "F10" => 109,
        "F11" => 103,
        "F12" => 111,
        _ => {
            let mut chars = keysym.chars();
            let character = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let code = ansi_code(character.to_ascii_lowercase())?;
            return Some((code, character.is_ascii_uppercase()));
        }
    };
    Some((named, false))
}

/// US ANSI key and Shift for a printable ASCII character.
fn ansi_character(character: char) -> Option<(CGKeyCode, bool)> {
    if character.is_ascii_alphanumeric() {
        return ansi_code(character.to_ascii_lowercase())
            .map(|code| (code, character.is_ascii_uppercase()));
    }
    let shifted = |base: char| ansi_symbol(base).map(|code| (code, true));
    match character {
        ' ' => Some((49, false)),
        '!' => ansi_code('1').map(|code| (code, true)),
        '@' => ansi_code('2').map(|code| (code, true)),
        '#' => ansi_code('3').map(|code| (code, true)),
        '$' => ansi_code('4').map(|code| (code, true)),
        '%' => ansi_code('5').map(|code| (code, true)),
        '^' => ansi_code('6').map(|code| (code, true)),
        '&' => ansi_code('7').map(|code| (code, true)),
        '*' => ansi_code('8').map(|code| (code, true)),
        '(' => ansi_code('9').map(|code| (code, true)),
        ')' => ansi_code('0').map(|code| (code, true)),
        '_' => shifted('-'),
        '+' => shifted('='),
        '{' => shifted('['),
        '}' => shifted(']'),
        '|' => shifted('\\'),
        ':' => shifted(';'),
        '"' => shifted('\''),
        '<' => shifted(','),
        '>' => shifted('.'),
        '?' => shifted('/'),
        '~' => shifted('`'),
        other => ansi_symbol(other).map(|code| (code, false)),
    }
}

fn ansi_symbol(character: char) -> Option<CGKeyCode> {
    Some(match character {
        '=' => 24,
        '-' => 27,
        ']' => 30,
        '[' => 33,
        '\'' => 39,
        ';' => 41,
        '\\' => 42,
        ',' => 43,
        '/' => 44,
        '.' => 47,
        '`' => 50,
        _ => return None,
    })
}

fn ansi_code(character: char) -> Option<CGKeyCode> {
    Some(match character {
        'a' => 0,
        's' => 1,
        'd' => 2,
        'f' => 3,
        'h' => 4,
        'g' => 5,
        'z' => 6,
        'x' => 7,
        'c' => 8,
        'v' => 9,
        'b' => 11,
        'q' => 12,
        'w' => 13,
        'e' => 14,
        'r' => 15,
        'y' => 16,
        't' => 17,
        '1' => 18,
        '2' => 19,
        '3' => 20,
        '4' => 21,
        '6' => 22,
        '5' => 23,
        '9' => 25,
        '7' => 26,
        '8' => 28,
        '0' => 29,
        'o' => 31,
        'u' => 32,
        'i' => 34,
        'p' => 35,
        'l' => 37,
        'j' => 38,
        'k' => 40,
        'n' => 45,
        'm' => 46,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysyms_map_to_ansi_codes_with_implied_shift() {
        assert_eq!(key_code("Return"), Some((36, false)));
        assert_eq!(key_code("a"), Some((0, false)));
        assert_eq!(key_code("A"), Some((0, true)));
        assert_eq!(key_code("7"), Some((26, false)));
        assert_eq!(key_code("Page_Down"), Some((121, false)));
        assert_eq!(key_code("NotAKey"), None);
        assert_eq!(modifier_key("super").map(|(code, _)| code), Some(55));
        assert_eq!(ansi_character('+'), Some((24, true)));
        assert_eq!(ansi_character('='), Some((24, false)));
        assert_eq!(ansi_character('Q'), Some((12, true)));
        assert_eq!(ansi_character(' '), Some((49, false)));
        assert_eq!(ansi_character('ç'), None);
    }
}
