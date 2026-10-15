//! rd-app 库面：P1 回环演示模块（pipeline / video / stats）。
//! 桌面 UI 入口在 `src/main.rs`（仪表板壳），演示入口在 `src/bin/rd-loop.rs`。

pub mod pattern;
pub mod pipeline;
#[cfg(feature = "webrtc")]
pub mod remote;
#[cfg(feature = "webrtc")]
pub mod remote_media;
#[cfg(feature = "webrtc")]
pub mod remote_pipeline;
pub mod stats;
pub mod video;

/// Shared renderer setup for the desktop shell and the video loopback.
/// Avoid initializing older Intel Vulkan drivers on Windows: they can crash
/// inside native driver code before wgpu can return an adapter error.
pub fn wgpu_configuration() -> egui_wgpu::WgpuConfiguration {
    #[allow(unused_mut)]
    let mut config = egui_wgpu::WgpuConfiguration::default();
    #[cfg(windows)]
    if let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup {
        setup.instance_descriptor.backends =
            wgpu::Backends::from_env().unwrap_or(wgpu::Backends::DX12);
    }
    config
}
