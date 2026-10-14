//! Windows chrome helpers for the undecorated window.
//!
//! winit gives an undecorated window no resize frame on Windows (the whole
//! window is client area), so a window-proc subclass answers WM_NCHITTEST
//! with the standard edge/corner codes for the outer few pixels — which are
//! plain background in every screen layout. Everything else is forwarded to
//! winit's own proc untouched.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, GetWindowLongPtrW, GetWindowRect, IsZoomed, SetWindowLongPtrW, SetWindowPos,
    GWLP_WNDPROC, GWL_STYLE, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP,
    HTTOPLEFT, HTTOPRIGHT, NCCALCSIZE_PARAMS, SM_CXPADDEDBORDER, SM_CXSIZEFRAME, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_THICKFRAME,
};

const WM_NCCALCSIZE: u32 = 0x0083;
const WM_NCHITTEST: u32 = 0x0084;

static OLD_PROC: AtomicIsize = AtomicIsize::new(0);
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Installs the subclass on the app window. Called every frame; runs once.
pub fn install_resize_frame(frame: &eframe::Frame) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if INSTALLED.load(Ordering::Relaxed) {
        return;
    }
    let Ok(handle) = frame.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    // SAFETY: on the UI thread with a live window; the previous proc is kept
    // in OLD_PROC and every message we do not handle goes back to it.
    unsafe {
        let old = SetWindowLongPtrW(
            h.hwnd.get() as HWND,
            GWLP_WNDPROC,
            wnd_proc as *const () as isize,
        );
        if old == 0 {
            return;
        }
        OLD_PROC.store(old, Ordering::Relaxed);
        INSTALLED.store(true, Ordering::Relaxed);
        // the sizing border makes the system resize loop (and Aero snap)
        // work; WM_NCCALCSIZE below keeps it invisible
        let style = GetWindowLongPtrW(h.hwnd.get() as HWND, GWL_STYLE) as u32;
        SetWindowLongPtrW(
            h.hwnd.get() as HWND,
            GWL_STYLE,
            (style | WS_THICKFRAME) as isize,
        );
        SetWindowPos(
            h.hwnd.get() as HWND,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// The width of the resize frame, in points, the shell keeps clear of
/// controls (kept in sync with the hit zones below).
pub fn resize_border() -> f32 {
    8.0
}

// SAFETY: forwards to the winit proc saved at install time.
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // keep the sizing border invisible: the client area is the whole window,
    // and the resize zones are the outer pixels claimed in WM_NCHITTEST.
    // Windows still sizes a maximized borderless window to the work area
    // plus the frame (ptMaxSize is ignored), so the maximized client is
    // clamped to the work area here — otherwise 8px of content sit off
    // screen on every side.
    if msg == WM_NCCALCSIZE && wp != 0 {
        if IsZoomed(hwnd) != 0 {
            if let Some(work) = work_area(hwnd) {
                (*(lp as *mut NCCALCSIZE_PARAMS)).rgrc[0] = work;
            }
        }
        return 0;
    }
    if msg == WM_NCHITTEST {
        if let Some(hit) = resize_hit(hwnd, lp) {
            return hit;
        }
    }
    let old = OLD_PROC.load(Ordering::Relaxed);
    if old == 0 {
        return 0;
    }
    // SAFETY: the stored value is winit's WNDPROC, put there right before
    // ours took over; both are pointer-sized.
    let proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT =
        std::mem::transmute(old);
    CallWindowProcW(Some(proc), hwnd, msg, wp, lp)
}

/// The work area (without the taskbar) of the monitor the window is on.
// SAFETY: plain Win32 queries on our own window.
unsafe fn work_area(hwnd: HWND) -> Option<RECT> {
    let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = std::mem::zeroed();
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if mon.is_null() || GetMonitorInfoW(mon, &mut info) == 0 {
        return None;
    }
    Some(info.rcWork)
}

/// Standard resize hit codes for the window's outer pixels, or None to let
/// winit answer (client area / maximized windows do not resize).
// SAFETY: plain Win32 queries on our own window.
unsafe fn resize_hit(hwnd: HWND, lp: LPARAM) -> Option<LRESULT> {
    if IsZoomed(hwnd) != 0 {
        return None;
    }
    // screen coordinates, both words signed (multi-monitor setups go negative)
    let x = (lp as u32 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lp as u32 >> 16) & 0xFFFF) as u16 as i16 as i32;
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if GetWindowRect(hwnd, &mut r) == 0 {
        return None;
    }
    let dpi = GetDpiForWindow(hwnd);
    let thin = GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi).max(2) as i32;
    let fat = thin + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi).max(0);
    // the top edge stays thin so the toolbar buttons keep their hit area
    let on_l = x < r.left + fat;
    let on_r = x >= r.right - fat;
    let on_t = y < r.top + thin;
    let on_b = y >= r.bottom - fat;
    if !on_l && !on_r && !on_t && !on_b {
        return None;
    }
    let hit = if on_t && on_l {
        HTTOPLEFT
    } else if on_t && on_r {
        HTTOPRIGHT
    } else if on_b && on_l {
        HTBOTTOMLEFT
    } else if on_b && on_r {
        HTBOTTOMRIGHT
    } else if on_l {
        HTLEFT
    } else if on_r {
        HTRIGHT
    } else if on_t {
        HTTOP
    } else {
        HTBOTTOM
    };
    Some(hit as LRESULT)
}

use crate::theme::{lerp_color, with_alpha};
use crate::TOOLBAR_H;
use eframe::egui::{self, Color32, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};
pub fn caption_buttons(ui: &mut Ui, screen: Rect, theme: &crate::theme::Theme) {
    let right = screen.right();
    let maxed = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    let h = TOOLBAR_H;
    // keep the invisible resize frame at the window's right edge clear
    let edge = resize_border();
    let mut rect = Rect::from_min_size(
        Pos2::new(right - edge - 46.0, screen.top()),
        Vec2::new(46.0, h),
    );
    if caption_button(ui, rect, Id::new("cap-close"), Cap::Close, theme).clicked() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }
    rect = rect.translate(Vec2::new(-46.0, 0.0));
    if caption_button(
        ui,
        rect,
        Id::new("cap-max"),
        if maxed { Cap::Restore } else { Cap::Max },
        theme,
    )
    .clicked()
    {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!maxed));
    }
    rect = rect.translate(Vec2::new(-46.0, 0.0));
    if caption_button(ui, rect, Id::new("cap-min"), Cap::Min, theme).clicked() {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
}
/// The caption glyphs, drawn by hand like every other icon.
#[cfg(windows)]
#[derive(Clone, Copy, PartialEq)]
enum Cap {
    Min,
    Max,
    Restore,
    Close,
}

#[cfg(windows)]
fn caption_button(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    glyph: Cap,
    theme: &super::theme::Theme,
) -> egui::Response {
    let resp = ui.interact(rect, id, Sense::click());
    let h = ui
        .ctx()
        .animate_bool_with_time(id.with("h"), resp.hovered(), 0.08);
    let p = ui.painter();
    let danger = glyph == Cap::Close;
    if h > 0.0 {
        let bg = if danger {
            lerp_color(Color32::TRANSPARENT, theme.danger, h)
        } else {
            with_alpha(theme.text, 0.08 * h)
        };
        p.rect_filled(rect, 0.0, bg);
    }
    let col = if danger && h > 0.35 {
        Color32::WHITE
    } else {
        lerp_color(theme.text_dim, theme.text, h)
    };
    let st = Stroke::new(1.6, col);
    let c = rect.center();
    let s = 10.0;
    match glyph {
        Cap::Min => {
            p.line_segment(
                [
                    Pos2::new(c.x - s / 2.0, c.y + 1.0),
                    Pos2::new(c.x + s / 2.0, c.y + 1.0),
                ],
                st,
            );
        }
        Cap::Max => {
            p.rect_stroke(
                Rect::from_center_size(c, Vec2::splat(s)),
                1.0,
                st,
                egui::StrokeKind::Inside,
            );
        }
        Cap::Restore => {
            let d = 3.5;
            p.rect_stroke(
                Rect::from_center_size(c + Vec2::new(d, -d), Vec2::splat(s - 1.0)),
                1.0,
                st,
                egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                Rect::from_center_size(c - Vec2::new(d, -d) / 2.0, Vec2::splat(s - 1.0)),
                1.0,
                st,
                egui::StrokeKind::Inside,
            );
        }
        Cap::Close => {
            let d = 0.42 * s;
            p.line_segment([c + Vec2::new(-d, -d), c + Vec2::new(d, d)], st);
            p.line_segment([c + Vec2::new(d, -d), c + Vec2::new(-d, d)], st);
        }
    }
    resp
}
