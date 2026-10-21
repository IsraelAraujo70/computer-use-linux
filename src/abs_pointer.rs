//! Direct uinput **absolute** pointer.
//!
//! ydotool's virtual device is relative-only (`EV=7`: SYN|KEY|REL), so its
//! `--absolute` is faked as "pin-to-corner + relative move", which the
//! compositor then distorts with pointer acceleration and fractional display
//! scaling — clicks land in the wrong place on multi-monitor / HiDPI setups.
//!
//! Here we create our own uinput device that exposes a true `ABS_X`/`ABS_Y`
//! axis whose range matches the full screenshot's pixel dimensions. The
//! compositor maps that range across its logical output layout. Callers keep
//! using capture-pixel coordinates, which can differ from logical coordinates
//! on scaled outputs. Returned landings describe the emitted coordinates;
//! they are not a readback of the compositor's actual pointer position.

use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result};
use evdev::{
    uinput::VirtualDevice, AbsInfo, AbsoluteAxisCode, AttributeSet, EventType, InputEvent, KeyCode,
    PropType, UinputAbsSetup,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "pointer input may be clamped; inspect requested and emitted coordinates"]
pub(crate) struct PointerLanding {
    pub(crate) requested: (i32, i32),
    pub(crate) emitted: (i32, i32),
}

#[derive(Clone, Copy)]
struct AbsPointerGeometry {
    max_x: i32,
    max_y: i32,
}

impl AbsPointerGeometry {
    fn from_dimensions(width: i32, height: i32) -> Self {
        Self {
            max_x: width.max(1).saturating_sub(1),
            max_y: height.max(1).saturating_sub(1),
        }
    }

    fn axis_maxima(self) -> (i32, i32) {
        (self.max_x, self.max_y)
    }

    fn clamp_coordinates(self, x: i32, y: i32) -> (i32, i32) {
        (x.clamp(0, self.max_x), y.clamp(0, self.max_y))
    }

    fn landing_for(self, x: i32, y: i32) -> PointerLanding {
        PointerLanding {
            requested: (x, y),
            emitted: self.clamp_coordinates(x, y),
        }
    }

    /// The kernel input core drops an ABS event whose value equals the axis's
    /// current value. Re-emitting the device's last point therefore produces
    /// no motion, and if another device (ydotool, a streaming client, a real
    /// mouse) moved the cursor since, the press lands at that stale position.
    /// When `target` repeats `last`, return an adjacent in-range point to emit
    /// first so the target frame carries a change. A single repeated axis
    /// needs nothing: libinput keeps its stored value for the missing axis.
    fn priming_point(self, last: (i32, i32), target: (i32, i32)) -> Option<(i32, i32)> {
        if last != target {
            return None;
        }
        let step = |value: i32, max: i32| if value < max { value + 1 } else { value - 1 };
        if self.max_x > 0 {
            Some((step(target.0, self.max_x), target.1))
        } else if self.max_y > 0 {
            Some((target.0, step(target.1, self.max_y)))
        } else {
            None
        }
    }
}

pub struct AbsPointer {
    device: VirtualDevice,
    geometry: AbsPointerGeometry,
    /// Last point written to the device; the kernel's current ABS values.
    last_emitted: (i32, i32),
}

impl AbsPointer {
    /// Create the absolute pointer sized to the capture's `width`×`height`.
    /// Waits 500 ms for libinput to enumerate the device before its first event.
    pub fn create(width: i32, height: i32) -> Result<Self> {
        let geometry = AbsPointerGeometry::from_dimensions(width, height);
        let (max_x, max_y) = geometry.axis_maxima();
        // value, min, max, fuzz, flat, resolution. resolution=1 unit/px.
        let abs_x =
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, AbsInfo::new(0, 0, max_x, 0, 0, 1));
        let abs_y =
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, AbsInfo::new(0, 0, max_y, 0, 0, 1));
        let keys =
            AttributeSet::from_iter([KeyCode::BTN_LEFT, KeyCode::BTN_RIGHT, KeyCode::BTN_MIDDLE]);
        // INPUT_PROP_DIRECT marks the device as a direct (absolute) pointer so
        // libinput maps its axes to screen coordinates rather than treating it
        // as a relative touchpad.
        let props = AttributeSet::from_iter([PropType::DIRECT]);

        let device = VirtualDevice::builder()
            .context("uinput builder (is /dev/uinput writable?)")?
            .name("computer-use-linux absolute pointer")
            .with_properties(&props)?
            .with_absolute_axis(&abs_x)?
            .with_absolute_axis(&abs_y)?
            .with_keys(&keys)?
            .build()
            .context("failed to create uinput absolute pointer device")?;

        // Give udev/libinput time to enumerate the new device.
        sleep(Duration::from_millis(500));

        Ok(Self {
            device,
            geometry,
            // Both axes were created with value 0.
            last_emitted: (0, 0),
        })
    }

    /// Move the pointer to capture-pixel coordinates `(x, y)` and report
    /// both the requested point and the values emitted after edge clamping.
    pub fn move_to(&mut self, x: i32, y: i32) -> Result<PointerLanding> {
        let landing = self.geometry.landing_for(x, y);
        if let Some(priming) = self
            .geometry
            .priming_point(self.last_emitted, landing.emitted)
        {
            self.emit_point(priming)?;
        }
        self.emit_point(landing.emitted)?;
        Ok(landing)
    }

    fn emit_point(&mut self, (x, y): (i32, i32)) -> Result<()> {
        self.device
            .emit(&[
                InputEvent::new_now(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, x),
                InputEvent::new_now(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, y),
            ])
            .context("failed to emit absolute motion")?;
        self.last_emitted = (x, y);
        Ok(())
    }

    /// Move to `(x, y)` then press+release `button` `count` times.
    pub fn click(
        &mut self,
        x: i32,
        y: i32,
        button: PointerButton,
        count: u32,
    ) -> Result<PointerLanding> {
        let landing = self.move_to(x, y)?;
        sleep(Duration::from_millis(30));
        let code = button.key_code();
        for _ in 0..count.max(1) {
            self.device
                .emit(&[InputEvent::new_now(EventType::KEY.0, code, 1)])?;
            sleep(Duration::from_millis(30));
            self.device
                .emit(&[InputEvent::new_now(EventType::KEY.0, code, 0)])?;
            sleep(Duration::from_millis(40));
        }
        Ok(landing)
    }

    /// Press at `(start)`, move to `(end)`, release — a drag with `button`.
    pub fn drag(
        &mut self,
        start: (i32, i32),
        end: (i32, i32),
        button: PointerButton,
    ) -> Result<()> {
        let code = button.key_code();
        // Drag currently reports backend success only; retain the landing
        // values explicitly so their intentional omission stays visible.
        let _start_landing = self.move_to(start.0, start.1)?;
        sleep(Duration::from_millis(30));
        self.device
            .emit(&[InputEvent::new_now(EventType::KEY.0, code, 1)])?;
        sleep(Duration::from_millis(40));
        let end_landing = self.move_to(end.0, end.1);
        sleep(Duration::from_millis(40));
        // Always try to release after a press, even if endpoint motion failed.
        let released = self
            .device
            .emit(&[InputEvent::new_now(EventType::KEY.0, code, 0)]);
        let _end_landing = end_landing?;
        released?;
        Ok(())
    }
}

/// Pointer buttons we can synthesize.
#[derive(Clone, Copy, Debug)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
}

impl PointerButton {
    pub fn from_name(name: Option<&str>) -> Option<Self> {
        match name.unwrap_or("left").to_ascii_lowercase().as_str() {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "middle" => Some(Self::Middle),
            _ => None,
        }
    }

    fn key_code(self) -> u16 {
        match self {
            Self::Left => KeyCode::BTN_LEFT.0,
            Self::Right => KeyCode::BTN_RIGHT.0,
            Self::Middle => KeyCode::BTN_MIDDLE.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AbsPointerGeometry, PointerButton};

    #[test]
    fn axis_range_ends_at_last_desktop_pixel() {
        let geometry = AbsPointerGeometry::from_dimensions(1920, 1080);

        assert_eq!(geometry.axis_maxima(), (1919, 1079));
    }

    #[test]
    fn pointer_landing_preserves_the_request_and_emitted_coordinates() {
        let geometry = AbsPointerGeometry::from_dimensions(1920, 1080);

        for (requested, emitted) in [
            ((640, 480), (640, 480)),
            ((1920, 1080), (1919, 1079)),
            ((-1, -1), (0, 0)),
            ((i32::MAX, i32::MAX), (1919, 1079)),
        ] {
            let landing = geometry.landing_for(requested.0, requested.1);
            assert_eq!(landing.requested, requested);
            assert_eq!(landing.emitted, emitted);
        }
    }

    #[test]
    fn repeated_target_is_primed_through_an_adjacent_in_range_point() {
        let geometry = AbsPointerGeometry::from_dimensions(1920, 1080);

        assert_eq!(
            geometry.priming_point((712, 298), (712, 298)),
            Some((713, 298))
        );
        assert_eq!(
            geometry.priming_point((1919, 1079), (1919, 1079)),
            Some((1918, 1079))
        );
        // A fresh device starts at (0, 0), so its first move there also repeats.
        assert_eq!(geometry.priming_point((0, 0), (0, 0)), Some((1, 0)));
        assert_eq!(
            AbsPointerGeometry::from_dimensions(1, 1080).priming_point((0, 5), (0, 5)),
            Some((0, 6))
        );
    }

    #[test]
    fn changed_targets_are_not_primed() {
        let geometry = AbsPointerGeometry::from_dimensions(1920, 1080);

        assert_eq!(geometry.priming_point((712, 298), (713, 298)), None);
        assert_eq!(geometry.priming_point((712, 298), (712, 299)), None);
        assert_eq!(geometry.priming_point((0, 0), (640, 480)), None);
    }

    /// Live check against the running Hyprland session: a second device moves
    /// the cursor away, then the same point is requested again.
    #[test]
    #[ignore = "needs a single 1920x1080 scale-1 Hyprland layout and writable /dev/uinput; moves the pointer"]
    fn repeated_move_lands_after_another_device_moved_the_cursor() {
        fn cursorpos() -> (i32, i32) {
            let output = std::process::Command::new("hyprctl")
                .arg("cursorpos")
                .output()
                .expect("hyprctl cursorpos");
            let text = String::from_utf8(output.stdout).unwrap();
            let (x, y) = text.trim().split_once(", ").expect("x, y");
            (x.parse().unwrap(), y.parse().unwrap())
        }
        let settle = || std::thread::sleep(std::time::Duration::from_millis(250));
        let (width, height) = (1920, 1080);
        let mut pointer = super::AbsPointer::create(width, height).unwrap();
        let mut other = super::AbsPointer::create(width, height).unwrap();

        let _ = pointer.move_to(712, 298).unwrap();
        settle();
        assert_eq!(cursorpos(), (712, 298));
        let _ = other.move_to(1424, 596).unwrap();
        settle();
        assert_eq!(cursorpos(), (1424, 596));
        let _ = pointer.move_to(712, 298).unwrap();
        settle();
        assert_eq!(cursorpos(), (712, 298));
    }

    #[test]
    fn unsupported_buttons_fall_through_to_other_backends() {
        assert!(matches!(
            PointerButton::from_name(None),
            Some(PointerButton::Left)
        ));
        assert!(matches!(
            PointerButton::from_name(Some("right")),
            Some(PointerButton::Right)
        ));
        assert!(matches!(
            PointerButton::from_name(Some("middle")),
            Some(PointerButton::Middle)
        ));

        for button in ["side", "extra", "forward", "back"] {
            assert!(
                PointerButton::from_name(Some(button)).is_none(),
                "{button} must fall through instead of becoming a left click"
            );
        }
    }
}
