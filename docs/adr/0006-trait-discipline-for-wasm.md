# ADR-0006: trait 纪律——核心 crate 不许依赖平台

- 状态：已接受
- 日期：2026-10-13

## 背景

Web 端免重写的前提是引擎与协议代码可以原样编译到 wasm。一旦核心 crate 长出对平台 crate 的依赖（windows / core-foundation / x11rb…），wasm 目标即被污染，最终退化成 RustDesk 那样"Web 端另写一份"。

## 决策

- `rd-protocol`、`rd-crypto` 与 `rd-engine` 的纯状态机/会话逻辑禁止依赖平台 crate 与 UI crate；P0 CI 对 wasm32-unknown-unknown 做编译检查。Tokio actor、计时器和 channel 属于运行时适配层，需验证 wasm 兼容性；若不兼容，应把核心逻辑与运行时适配器分离，而不是让核心状态机绑定不可用的运行时。
- 平台能力（捕获、注入、编解码硬件路径、传输）一律以 trait 表达：`Capturer` / `Encoder` / `Decoder` / `InputInjector` / `Transport`，由 `rd-app`（原生）或 wasm 侧注入实现。
- 能力 trait 的数据类型（帧、事件）必须是与平台无关的纯数据；但该约束不等同于零拷贝互操作。P1 需验证硬件帧/GPU 纹理的跨 API 交接以及浏览器 WebCodecs 输出是否能适配同一抽象。

## 后果

- 目标是让协议和会话规则复用；渲染、媒体缓冲和运行时仍可能需要平台适配，不能预先承诺 P8 只是“换实现”。
- 约束需要纪律维护：每次核心 crate 想引入"只是个小工具"的平台 crate 时，改为在 trait 上加能力。
