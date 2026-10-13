# ADR-0001: 纯 Rust 单语言栈，UI 选 egui/wgpu

- 状态：已接受
- 日期：2026-10-13

## 背景

RustDesk 的结构性代价：flutter_rust_bridge 钉死旧版、codegen 无法本地运行、300+ FFI 函数手写维护；Web 端更是用 TS 整体重写了一遍客户端逻辑，同一套会话逻辑存在两份实现。远程桌面应用的主体是一块视频画布 + 少量控件，对复杂桌面控件生态的需求是次要的。

## 决策

引擎和 UI 都用 Rust：桌面/移动编译为原生程序，Web 编译为 wasm。UI 层用 egui + wgpu，视频帧作为 GPU 纹理经 `egui_wgpu` 的 `CallbackTrait` 自定义 pass 直接合成，YUV→RGB 在 shader 中完成。

## 后果

- 消灭 FFI 桥与 Web 端重写，功能只改一处。
- egui 原生支持 wasm，桌面与 Web 共用同一份引擎和 UI 代码。
- 代价：egui 复杂控件与 i18n 成熟度不如 Flutter，UI 打磨速度慢；此为换取结构性收益的自觉取舍。
- 已知风险：egui/wgpu API 变动较快，钉版本；视频渲染性能必须在 P1 最先验证（有 TextureHandle + CPU 转换的降级路径）。
