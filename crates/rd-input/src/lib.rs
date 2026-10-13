//! 输入注入 trait（ADR-0006）。P4 接入平台实现：
//! Windows SendInput、macOS CGEvent（需辅助功能权限）、Linux uinput（Wayland）/ XTest（X11）。

/// 平台无关键码：USB HID usage (keyboard page)。映射到各平台扫描码由注入端完成。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub hid_usage: u16,
    pub pressed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    Key(KeyEvent),
    /// 绝对坐标，0.0..=1.0 归一化到被控屏幕（跨分辨率正确性）。
    MouseMove {
        x: f32,
        y: f32,
    },
    MousePress {
        button: MouseButton,
    },
    MouseRelease {
        button: MouseButton,
    },
    /// 滚轮增量，单位：格（正方向为向上/向左）。
    Wheel {
        dx: f32,
        dy: f32,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("input injection not permitted (missing OS permission or user denied)")]
    NotPermitted,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub trait InputInjector: Send {
    fn inject(&mut self, event: &InputEvent) -> Result<(), InputError>;
}
