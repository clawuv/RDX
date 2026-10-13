# RDX

纯 Rust 远程桌面：egui/wgpu UI · str0m + coturn 传输 · 用户态会话引擎 + 薄特权代理 · 一套代码编译原生与 wasm。

- 实施计划：[PLAN.md](PLAN.md)
- 架构决策记录：[docs/adr/](docs/adr/)

## 状态

P0（骨架）。当前包含 workspace、核心 trait、裸帧格式（含测试）、会话状态机（含测试）、最小 egui 应用。

## 构建

```sh
cargo build --workspace
cargo test --workspace
cargo run -p rd-app
```

## Crate 一览

| crate | 职责 |
|---|---|
| `rd-protocol` | 数据面裸帧格式 + 控制面消息（protobuf，P2 起） |
| `rd-transport` | `Transport` trait；str0m（原生）/ 浏览器 WebRTC（wasm）；内存回环实现供测试 |
| `rd-capture` | 屏幕捕获 trait；平台实现 P1 起接入 |
| `rd-codec` | 编解码 trait；openh264 软编基线 + 硬编，P1 起接入 |
| `rd-input` | 输入注入 trait；平台实现 P4 起接入 |
| `rd-engine` | 会话状态机与 actor（纯逻辑，headless 可测） |
| `rd-crypto` | OPAQUE 认证 + Noise 会话加密（P4/P6 落地） |
| `rd-broker` | 特权能力代理（P4 落地） |
| `rd-audio` | 音频采集/播放（P5 落地） |
| `rd-app` | egui 应用（唯一二进制） |
| `rd-server` | 信令服务（P3 落地） |

## 许可证

待定（闭源商用前必须在此定案，并收紧 `deny.toml` 的依赖许可白名单）。
