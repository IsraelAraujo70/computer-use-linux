//! The agent's visible cursor on macOS. Background mode never moves the
//! user's pointer, so the user sees where the agent works through a second,
//! colored arrow drawn in a click-through overlay window: it glides to each
//! point the agent acts on, rings on clicks and fades out when idle.
//!
//! `computer-use-linux cursor-overlay` runs the overlay (a LaunchAgent next
//! to the MCP socket); the MCP server sends it one datagram per action
//! (`show`). Without a running overlay, `show` is a no-op.

use anyhow::{Context, Result};
use std::{
    os::unix::net::UnixDatagram,
    path::PathBuf,
    time::{Duration, Instant},
};

/// What the agent did at a point.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Move,
    Click,
}

fn socket_path() -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/state/computer-use-mac/cursor.sock")
}

/// Shows the agent's cursor at a global point (top-left origin, points).
/// Best effort: never blocks and never fails the action.
pub(crate) fn show(x: f64, y: f64, kind: Kind) {
    let Ok(socket) = UnixDatagram::unbound() else {
        return;
    };
    let _ = socket.set_nonblocking(true);
    let tag = if kind == Kind::Click { "c" } else { "m" };
    let _ = socket.send_to(format!("{x} {y} {tag}").as_bytes(), socket_path());
}

/// Image size in points; the arrow tip sits at `TIP` inside it, leaving
/// room for the click ring around the tip.
const SIZE: f64 = 44.0;
const TIP: f64 = 14.0;
const SCALE: u32 = 2;
const GLIDE: Duration = Duration::from_millis(180);
const RING: Duration = Duration::from_millis(260);
const IDLE: Duration = Duration::from_secs(3);
const COLOR: [u8; 3] = [124, 58, 237];

/// Runs the overlay until killed. Must run on the main thread.
pub(crate) fn run_overlay() -> Result<()> {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSEventMask,
        NSImage, NSImageView, NSWindow, NSWindowCollectionBehavior, NSWindowSharingType,
        NSWindowStyleMask,
    };
    use objc2_foundation::{NSData, NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize};

    let mtm = MainThreadMarker::new().context("the cursor overlay must run on the main thread")?;
    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(&path);
    let socket = UnixDatagram::bind(&path).context("could not bind the cursor socket")?;
    socket.set_nonblocking(true)?;

    let app = NSApplication::sharedApplication(mtm);
    // No Dock icon, no menu bar, never activated: it must not take focus.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.finishLaunching();

    let image = |click: bool| {
        let png = arrow_png(click);
        let data = NSData::with_bytes(&png);
        let image = NSImage::initWithData(NSImage::alloc(), &data).expect("cursor PNG decodes");
        image.setSize(NSSize::new(SIZE, SIZE));
        image
    };
    let (arrow, ring) = (image(false), image(true));
    let view = NSImageView::imageViewWithImage(&arrow, mtm);
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(SIZE, SIZE));
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setOpaque(false);
    window.setHasShadow(false);
    window.setIgnoresMouseEvents(true);
    // Keep the agent's own cursor out of its screenshots.
    window.setSharingType(NSWindowSharingType::None);
    // Above everything, on every Space and over full-screen apps.
    window.setLevel(1000);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    window.setContentView(Some(&view));

    let mut at: Option<(f64, f64)> = None;
    let mut from = (0.0, 0.0);
    let mut to = (0.0, 0.0);
    let mut moved = Instant::now();
    let mut ring_until: Option<Instant> = None;
    let mut visible = false;
    let mut buffer = [0u8; 128];
    loop {
        // Pump AppKit for one frame.
        let until = NSDate::dateWithTimeIntervalSinceNow(1.0 / 60.0);
        while let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&until),
            unsafe { NSDefaultRunLoopMode },
            true,
        ) {
            app.sendEvent(&event);
        }
        while let Ok(len) = socket.recv(&mut buffer) {
            let message = String::from_utf8_lossy(&buffer[..len]);
            let mut parts = message.split_whitespace();
            let (Some(x), Some(y)) = (
                parts.next().and_then(|v| v.parse::<f64>().ok()),
                parts.next().and_then(|v| v.parse::<f64>().ok()),
            ) else {
                continue;
            };
            from = at.unwrap_or((x, y));
            to = (x, y);
            moved = Instant::now();
            if parts.next() == Some("c") {
                ring_until = Some(moved + GLIDE + RING);
            }
        }
        let elapsed = moved.elapsed();
        if elapsed > IDLE + GLIDE {
            if visible {
                window.orderOut(None);
                visible = false;
                at = None;
            }
            continue;
        }
        // Ease-out glide from the last point to the new one.
        let t = (elapsed.as_secs_f64() / GLIDE.as_secs_f64()).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        let point = (
            from.0 + (to.0 - from.0) * eased,
            from.1 + (to.1 - from.1) * eased,
        );
        at = Some(point);
        let ringing = ring_until.is_some_and(|until| t >= 1.0 && Instant::now() < until);
        view.setImage(Some(if ringing { &ring } else { &arrow }));
        // AppKit's origin is the bottom-left of the primary display.
        let primary = objc2_core_graphics::CGDisplayBounds(objc2_core_graphics::CGMainDisplayID());
        window.setFrameOrigin(NSPoint::new(
            point.0 - TIP,
            primary.size.height - (point.1 - TIP) - SIZE,
        ));
        if !visible {
            window.orderFrontRegardless();
            visible = true;
        }
    }
}

/// The arrow (and with `click`, a ring around its tip) as a PNG at 2x.
fn arrow_png(click: bool) -> Vec<u8> {
    // Classic pointer outline, in points from the tip.
    const ARROW: [(f64, f64); 7] = [
        (0.0, 0.0),
        (0.0, 18.0),
        (4.5, 13.8),
        (7.6, 20.8),
        (10.4, 19.6),
        (7.4, 12.8),
        (13.0, 12.8),
    ];
    let side = (SIZE as u32) * SCALE;
    let mut image = image::RgbaImage::new(side, side);
    let samples = 4;
    for py in 0..side {
        for px in 0..side {
            let (mut fill, mut edge, mut ring) = (0, 0, 0);
            for sy in 0..samples {
                for sx in 0..samples {
                    let x = (f64::from(px) + (f64::from(sx) + 0.5) / f64::from(samples))
                        / f64::from(SCALE)
                        - TIP;
                    let y = (f64::from(py) + (f64::from(sy) + 0.5) / f64::from(samples))
                        / f64::from(SCALE)
                        - TIP;
                    let inside = inside(&ARROW, x, y);
                    let near = distance_to_outline(&ARROW, x, y) < 1.4;
                    if inside && !near {
                        fill += 1;
                    } else if inside || near {
                        edge += 1;
                    }
                    let radius = (x * x + y * y).sqrt();
                    if click && (9.0..12.0).contains(&radius) {
                        ring += 1;
                    }
                }
            }
            let total = f64::from(samples * samples);
            let (fill, edge, ring) = (
                f64::from(fill) / total,
                f64::from(edge) / total,
                f64::from(ring) / total,
            );
            // Arrow over ring; white outline, colored body.
            let arrow_alpha = fill + edge;
            let mut rgb = [0.0; 3];
            let mut alpha = 0.0;
            if ring > 0.0 {
                let a = ring * 0.85;
                for (c, value) in rgb.iter_mut().zip(COLOR) {
                    *c = f64::from(value) * a;
                }
                alpha = a;
            }
            if arrow_alpha > 0.0 {
                for (c, value) in rgb.iter_mut().zip(COLOR) {
                    let body = (f64::from(value) * fill + 255.0 * edge) / arrow_alpha;
                    *c = body * arrow_alpha + *c * (1.0 - arrow_alpha);
                }
                alpha = arrow_alpha + alpha * (1.0 - arrow_alpha);
            }
            if alpha > 0.0 {
                let un = |c: f64| (c / alpha).round().clamp(0.0, 255.0) as u8;
                image.put_pixel(
                    px,
                    py,
                    image::Rgba([un(rgb[0]), un(rgb[1]), un(rgb[2]), (alpha * 255.0) as u8]),
                );
            }
        }
    }
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("PNG encodes");
    out
}

fn inside(polygon: &[(f64, f64)], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let ((xi, yi), (xj, yj)) = (polygon[i], polygon[j]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn distance_to_outline(polygon: &[(f64, f64)], x: f64, y: f64) -> f64 {
    let mut best = f64::MAX;
    for i in 0..polygon.len() {
        let (ax, ay) = polygon[i];
        let (bx, by) = polygon[(i + 1) % polygon.len()];
        let (dx, dy) = (bx - ax, by - ay);
        let t = (((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        let (cx, cy) = (ax + t * dx, ay + t * dy);
        best = best.min(((x - cx).powi(2) + (y - cy).powi(2)).sqrt());
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arrow_tip_is_opaque_and_the_corner_clear() {
        let png = arrow_png(false);
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        let tip = (TIP as u32 + 1) * SCALE;
        assert!(image.get_pixel(tip, tip + 4 * SCALE)[3] > 200, "body drawn");
        assert_eq!(image.get_pixel(0, 0)[3], 0, "corner transparent");
        let ring = arrow_png(true);
        let ring = image::load_from_memory(&ring).unwrap().to_rgba8();
        // A point on the ring above the tip, outside the arrow.
        let on_ring = (TIP as u32 * SCALE, ((TIP - 10.5) * f64::from(SCALE)) as u32);
        assert!(ring.get_pixel(on_ring.0, on_ring.1)[3] > 100, "ring drawn");
        assert_eq!(
            image.get_pixel(on_ring.0, on_ring.1)[3],
            0,
            "no ring without click"
        );
    }
}
