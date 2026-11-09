//! Input on macOS: runs the exact `xdotool` command lines `server.rs` builds
//! (`mousemove`, `click`, `mousedown`, `mouseup`, `sleep`, `type`, `key`)
//! as CGEvents, so every pointer and keyboard path of the server works
//! unchanged. Coordinates are global screen points (top-left origin), the
//! space AX bounds and screenshots use.
//!
//! Text goes in as Unicode keyboard events, independent of the keyboard
//! layout, so non-ASCII typed text never needs the clipboard. Key chords map
//! xdotool keysyms to ANSI virtual key codes; `super` is Command.

use objc2_application_services::AXIsProcessTrusted;
use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton,
    CGScrollEventUnit,
};
use std::{thread::sleep, time::Duration};

/// Pixels one wheel "click" scrolls; xdotool's 5 clicks are one page step.
const PIXELS_PER_WHEEL_CLICK: i32 = 100;
/// Gap between the presses of a multi-click when no `--delay` is given;
/// short enough for the system to count a double click.
const MULTI_CLICK_GAP: Duration = Duration::from_millis(40);

/// Runs one xdotool argument vector. Errors name the token that failed.
pub(crate) fn run(args: &[String]) -> Result<(), String> {
    if !unsafe { AXIsProcessTrusted() } {
        return Err(
            "Accessibility permission missing: macOS drops synthetic input from this binary"
                .to_string(),
        );
    }
    eprintln!(
        "input: {} -> frontmost pid {:?}",
        args.first().map(String::as_str).unwrap_or_default(),
        crate::macos::ax_tree::frontmost_pid()
    );
    let mut input = Input::default();
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
                input.move_to(CGPoint { x, y })?;
            }
            "click" => {
                let button: u32 = number(tokens.next(), "click button")?;
                input.click(button, options)?;
            }
            "mousedown" | "mouseup" => {
                let button: u32 = number(tokens.next(), command)?;
                input.button(button, command == "mousedown")?;
            }
            "sleep" => {
                let seconds: f64 = number(tokens.next(), "sleep")?;
                sleep(Duration::from_secs_f64(seconds.max(0.0)));
            }
            "type" => {
                let text = tokens.next().ok_or("type needs text")?;
                type_text(text, options.delay_ms)?;
            }
            "key" => {
                let spec = tokens.next().ok_or("key needs a key")?;
                key_chord(spec)?;
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

/// Pointer state across one command line: where it is and which button is
/// held, so moves while held become drags.
#[derive(Default)]
struct Input {
    at: Option<CGPoint>,
    held: Option<u32>,
}

impl Input {
    fn position(&mut self) -> CGPoint {
        *self.at.get_or_insert_with(|| {
            CGEvent::new(None)
                .map(|event| CGEvent::location(Some(&event)))
                .unwrap_or_default()
        })
    }

    fn move_to(&mut self, point: CGPoint) -> Result<(), String> {
        self.at = Some(point);
        let (kind, button) = match self.held {
            Some(1) => (CGEventType::LeftMouseDragged, CGMouseButton::Left),
            Some(3) => (CGEventType::RightMouseDragged, CGMouseButton::Right),
            Some(_) => (CGEventType::OtherMouseDragged, CGMouseButton::Center),
            None => (CGEventType::MouseMoved, CGMouseButton::Left),
        };
        post_mouse(kind, point, button, 0)
    }

    fn button(&mut self, button: u32, down: bool) -> Result<(), String> {
        let (down_kind, up_kind, mouse) = mouse_button(button)?;
        let at = self.position();
        self.held = down.then_some(button);
        post_mouse(if down { down_kind } else { up_kind }, at, mouse, 1)
    }

    fn click(&mut self, button: u32, options: Options) -> Result<(), String> {
        let repeat = options.repeat.max(1);
        if (4..=7).contains(&button) {
            for _ in 0..repeat {
                scroll(button)?;
                sleep(Duration::from_millis(options.delay_ms));
            }
            return Ok(());
        }
        let (down, up, mouse) = mouse_button(button)?;
        let at = self.position();
        for state in 1..=repeat {
            post_mouse(down, at, mouse, state)?;
            post_mouse(up, at, mouse, state)?;
            if state < repeat {
                sleep(match options.delay_ms {
                    0 => MULTI_CLICK_GAP,
                    ms => Duration::from_millis(ms),
                });
            }
        }
        Ok(())
    }
}

fn mouse_button(button: u32) -> Result<(CGEventType, CGEventType, CGMouseButton), String> {
    Ok(match button {
        1 => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGMouseButton::Left,
        ),
        2 => (
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGMouseButton::Center,
        ),
        3 => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGMouseButton::Right,
        ),
        other => return Err(format!("unsupported mouse button {other}")),
    })
}

fn post_mouse(
    kind: CGEventType,
    at: CGPoint,
    button: CGMouseButton,
    click_state: u32,
) -> Result<(), String> {
    let event =
        CGEvent::new_mouse_event(None, kind, at, button).ok_or("CGEventCreateMouseEvent failed")?;
    if click_state > 0 {
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::MouseEventClickState,
            i64::from(click_state),
        );
    }
    CGEvent::set_flags(Some(&event), CGEventFlags(0));
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    Ok(())
}

/// X11 wheel buttons: 4 up, 5 down, 6 left, 7 right.
fn scroll(button: u32) -> Result<(), String> {
    let step = PIXELS_PER_WHEEL_CLICK;
    let (vertical, horizontal) = match button {
        4 => (step, 0),
        5 => (-step, 0),
        6 => (0, step),
        _ => (0, -step),
    };
    let event = CGEvent::new_scroll_wheel_event2(
        None,
        CGScrollEventUnit::Pixel,
        2,
        vertical,
        horizontal,
        0,
    )
    .ok_or("CGEventCreateScrollWheelEvent failed")?;
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    Ok(())
}

/// Types `text` as Unicode key events; CR is the Return key and tab the Tab
/// key, as `xdotool type` sends them.
fn type_text(text: &str, delay_ms: u64) -> Result<(), String> {
    for character in text.chars() {
        match character {
            '\r' | '\n' => tap(KEY_RETURN, CGEventFlags(0))?,
            '\t' => tap(KEY_TAB, CGEventFlags(0))?,
            other => {
                let mut units = [0u16; 2];
                let units = other.encode_utf16(&mut units);
                for down in [true, false] {
                    let event = CGEvent::new_keyboard_event(None, 0, down)
                        .ok_or("CGEventCreateKeyboardEvent failed")?;
                    CGEvent::set_flags(Some(&event), CGEventFlags(0));
                    unsafe {
                        CGEvent::keyboard_set_unicode_string(
                            Some(&event),
                            units.len() as _,
                            units.as_ptr(),
                        )
                    };
                    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
                }
            }
        }
        sleep(Duration::from_millis(delay_ms));
    }
    Ok(())
}

/// One `xdotool key` spec such as `ctrl+a`, `super+v`, `Return` or a bare
/// modifier.
fn key_chord(spec: &str) -> Result<(), String> {
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
        return tap(code, CGEventFlags(0));
    }
    let (code, shifted) = key_code(key).ok_or_else(|| format!("unsupported key {key}"))?;
    if shifted {
        flags |= CGEventFlags::MaskShift.0;
    }
    tap(code, CGEventFlags(flags))
}

fn tap(code: CGKeyCode, flags: CGEventFlags) -> Result<(), String> {
    for down in [true, false] {
        let event = CGEvent::new_keyboard_event(None, code, down)
            .ok_or("CGEventCreateKeyboardEvent failed")?;
        CGEvent::set_flags(Some(&event), flags);
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
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
    }
}
