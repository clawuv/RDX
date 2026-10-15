# P2 固定配置能力交换（2026-10-15）

控制 protobuf 增加 Capabilities（oneof tag 2），字段包括实验配置版本、视频消息接收上限、控制消息接收上限、Frame 载荷接收上限、H.264 时间戳格式 v1 和关键帧恢复支持。

后续增量：Capabilities tag 7 增加必需的 heartbeat 支持；缺失该能力的旧实验版本会被拒绝。交换成功后启用 transport 独立心跳，详见 [心跳记录](p2-heartbeat-2026-10-15.md)。以下 45 项为最初能力交换增量的历史验收，最新回归以心跳记录为准。

v1 对应 RDF1 分片、12KiB 块加 21 字节信封、16KiB 控制消息、16MiB Frame 载荷、H26\x01 媒体封装及现有 protobuf 关键帧恢复。接收上限大于本地要求可以接受，小于要求拒绝；版本、媒体格式和恢复能力必须兼容。不会根据较小的对端上限动态改变分片。

`remote::exchange_capabilities(&mut transport)` 在可靠控制通道先发送本地能力，再读取对端能力，5 秒内未完成则失败。交换期间调用方独占普通控制消息接收；首条非能力消息、未知消息、解码失败或不兼容均失败。失败后必须关闭连接，不能重用超时的交换。

调用方需在真实认证完成后、发送媒体前执行交换，成功后再给会话 actor 发送 CapabilitiesAgreed。该入口不执行认证，也未在主 UI 自动调用；当前底层 MediaSender 仍是可直接调用的开发接口，不具备不可绕过的 Active 门控。交换不是身份认证或额外确认握手。

测试包括 protobuf 往返、逐字段不兼容拒绝、更大接收容量兼容，以及真实本机 ICE/DTLS/SCTP 上的双方交换和不兼容对端拒绝。动态消息大小协商、会话总装、重连与跨机验收仍开放，线上格式尚未冻结。

验收记录：`cargo test -p rd-protocol -p rd-engine -p rd-app --features rd-app/webrtc --lib --locked` 通过，共 45 项（app 21、engine 11、protocol 13）。相同包与 feature 的 `cargo clippy --all-targets --locked -- -D warnings` 通过；`cargo fmt --all -- --check` 与 `git diff --check` 通过。这是定向库回归，不代表完整工作区或跨机验收。P1 剩余验收继续按用户要求延期。
