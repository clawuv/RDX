//! macOS window chrome helpers.

/// (Vertical centre from the top, right edge, left edge) of the traffic-light
/// buttons, read from AppKit so the custom top bar lines up on every macOS
/// version.
pub fn traffic_lights(frame: &eframe::Frame) -> Option<(f32, f32, f32)> {
    use objc2_app_kit::{NSView, NSWindowButton};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = frame.window_handle().ok()?;
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return None;
    };
    // SAFETY: winit hands out a valid NSView for the lifetime of the window,
    // and eframe calls us on the main thread.
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    let window = view.window()?;
    let content = window.contentView()?;
    let height = content.frame().size.height;
    let close = window.standardWindowButton(NSWindowButton::CloseButton)?;
    let zoom = window.standardWindowButton(NSWindowButton::ZoomButton)?;
    let c = close.convertRect_toView(close.bounds(), None);
    let z = zoom.convertRect_toView(zoom.bounds(), None);
    let center_from_top = height - (c.origin.y + c.size.height / 2.0);
    Some((
        center_from_top as f32,
        (z.origin.x + z.size.width) as f32,
        c.origin.x as f32,
    ))
}

/// Moves the three traffic-light buttons by (dx, dy) points (dy positive =
/// down). Returns false when the window isn't ready yet; try again next frame.
pub fn shift_traffic_lights(frame: &eframe::Frame, dx: f64, dy: f64) -> bool {
    use objc2_app_kit::{NSView, NSWindowButton};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = frame.window_handle() else {
        return false;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return false;
    };
    // SAFETY: winit hands out a valid NSView for the lifetime of the window,
    // and eframe calls us on the main thread.
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    let Some(window) = view.window() else {
        return false;
    };
    let mut ok = true;
    for t in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        let Some(btn) = window.standardWindowButton(t) else {
            ok = false;
            continue;
        };
        let mut f = btn.frame();
        f.origin.x += dx;
        f.origin.y -= dy; // AppKit's y axis points up
        btn.setFrameOrigin(f.origin);
    }
    ok
}
