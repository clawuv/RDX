# RDX

纯 Rust 远程桌面：egui/wgpu UI · str0m + coturn 传输 · 用户态会话引擎 + 薄特权代理 · 一套代码编译原生与 wasm。

- 实施计划：[PLAN.md](PLAN.md)
- 架构决策记录：[docs/adr/](docs/adr/)

## 状态

P1 进行中：已实现合成视频回环（H.264 软编软解 + GPU YUV 渲染）与桌面 UI，Windows DXGI 和 macOS ScreenCaptureKit 已接入可选真屏回环；真屏连续采样、CPU 编解码基准、限时回环统计与 release 对照已实测，受控性能验收仍未完成。Linux 捕获、传输和认证仍未完成；P0 跨平台与安全评审门槛尚未全部验收。详见 PLAN.md、docs/validation-2026-10-14.md 和 [macOS 捕获接入记录](docs/p1-macos-2026-10-14.md)。

## 构建

工具链与 DCX 一致，固定为 Rust/Cargo 1.95；通过 rustup 安装 Rust 后，
在项目目录运行普通 `cargo` 命令即可自动选择，无需额外指定 `+1.95`。
Windows 使用 MSVC 工具链，需要安装 Visual Studio C++ Build Tools 和 Windows SDK。

```sh
cargo build --workspace
cargo test --workspace
cargo run -p rd-app
```

正式构建与 DCX 使用相同优化：`opt-level = 3`、fat LTO、单个 codegen unit、strip；
开发构建的第三方依赖使用 `opt-level = 2`。

```sh
cargo build --workspace --release
cargo run -p rd-app --release
```

Windows GUI 产物为 `target/release/rd-app.exe`，正式版本不弹出控制台窗口。
`rd-loop` 调试工具仍保留控制台，可用 `cargo run -p rd-app --bin rd-loop` 运行；
设置 `RDX_LOOP_SECONDS=<1..120>` 可限时运行并在结束时输出上传/绘制汇总，
设置 `RDX_CAPTURE_DISPLAY=<索引>` 可选择真屏来源（需系统授予屏幕录制/捕获权限）。
CI 同样固定使用 1.95；wasm 目标由对应 CI 作业安装。

P2 本机连续 WebRTC 开发模式：设置 `RDX_LOOP_WEBRTC=1`，运行
`cargo run -p rd-app --features webrtc --bin rd-loop`。该模式默认用合成源，
经真实本机 WebRTC 传输后显示；Windows 可设置 `RDX_CAPTURE_DISPLAY=0` 选择真屏来源，
可搭配 `RDX_LOOP_SECONDS` 限时运行；网络关闭或超时最多自动重连 3 次，保留首次选择的显示器。详见 [验收记录](docs/p2-continuous-loop-2026-10-15.md)。

Windows 的主界面和 `rd-loop` 默认使用 wgpu/DX12，避免部分旧版 Intel Vulkan
驱动在启动时发生访问冲突。无需设置环境变量；调试时仍可通过 `WGPU_BACKEND`
显式选择其他后端。

## Crate 一览

| crate | 职责 |
|---|---|
| `rd-protocol` | 数据面裸帧格式 + 控制面消息（protobuf，P2 起） |
| `rd-transport` | `Transport` trait；str0m（原生）/ 浏览器 WebRTC（wasm）；内存回环实现供测试 |
| `rd-capture` | 屏幕捕获 trait；合成源、Windows DXGI、macOS ScreenCaptureKit 已接入，Linux 未接入 |
| `rd-codec` | 编解码 trait；openh264 软编软解基线已实现，硬编未接入 |
| `rd-input` | 输入注入 trait；平台实现 P4 起接入 |
| `rd-engine` | 会话状态机与 actor（纯逻辑，headless 可测） |
| `rd-crypto` | OPAQUE 认证 + Noise 会话加密（P4/P6 落地） |
| `rd-broker` | 特权能力代理（P4 落地） |
| `rd-audio` | 音频采集/播放（P5 落地） |
| `rd-app` | egui 应用（唯一二进制） |
| `rd-server` | 信令服务（P3 落地） |

## 许可证

AGPL-3.0-only（见 [LICENSE](LICENSE)）。`rd-app` 部分参考 UI 与字体源自 DCX/SquirrelDisk（AGPL-3.0-only），来源声明见 `crates/rd-app/assets/REFERENCE-NOTICE.md`；字体对应版本许可仍待核对，详见 [docs/licensing-inventory.md](docs/licensing-inventory.md)。依赖侧约束见 `deny.toml`。
