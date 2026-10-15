# P2 分片与重组组件

P1 剩余验收按用户要求暂缓，不表示通过。本记录现包括实验性封装及其 str0m 本机接入；不宣布传输规格冻结或 P2 完成。

## 实验性封装

每个编码访问单元对应一个完整 Frame。分片前校验声明载荷长度与实际长度一致，保留 stream、flags、seq 和载荷。完整 Frame 编码后按 12KiB 切块，每块附 21 字节大端头：

| 字段 | 长度 |
|---|---|
| 魔数 RDF1 | 4 |
| 连接内访问单元 ID | 8 |
| stream ID | 1 |
| 完整 Frame 字节数 | 4 |
| 分片索引（从 0 开始） | 2 |
| 分片总数 | 2 |

ID 每个连接内递增，不随 Frame 的 u16 seq 回绕；u64 耗尽须建立新连接，不回绕。新连接创建全新重组器，旧连接消息不得路由进新实例。当前固定块大小是实验参数，正式接入还需能力协商。

## 限制与丢弃规则

- 单 Frame 载荷最多 16MiB，最多同时重组 4 帧，声明的完整帧字节总预留不超过 32MiB；拒绝超额新帧，不驱逐已有帧。
- 首片起 500ms 过期；重复片不续期，同内容重复片忽略，冲突片丢弃整帧。应用必须用单调时钟定时调用 expire，即使没有收到消息。
- 支持乱序片；保存最近 64 个 ID 范围的已完成/已丢弃记录，拒绝重放或更旧的 ID。高 ID 跳跃会使旧 ID 失效，此版本不做跨连接重放认证。
- expire 返回过期访问单元及其 stream ID；传输层负责触发恢复请求。
- buffered_bytes/peak_bytes 表示完整帧字节预留，不是进程 RSS。拼装期间额外分配一个完整 Frame（最多 16MiB+8 字节），加上分片元数据与分配器开销，不能把 32MiB 宣称为总内存峰值。调用方持有的输出帧不计入重组缓存。

## str0m 与关键帧恢复接入（2026-10-15）

- rdx-control 是有序可靠 DataChannel；rdx-video 是无序、零重传 DataChannel。控制 API 限 16KiB；视频使用 12KiB 分片。发送命令、外发及接收队列按控制/视频分开，各最多 256 条；worker 不阻塞推送，满队列时关闭连接。控制每轮优先发送最多 16 条，再发送最多 16 条媒体分片。SDP answer 走独立的一次性同步通道，不占控制消息额度。
- `rdx-control` 增加 protobuf `KeyframeRequest`，携带 stream ID 和独立于帧内 u16 序号的 u64 访问单元 ID。过期重组会发出本地 `KeyframeNeeded`，并通过可靠控制通道发给对端；对端解析后发出 `KeyframeRequested`。
- 丢片是可恢复媒体损失：`Transport::recv` 清理过期帧并发出关键帧请求后继续等待，不以错误结束会话。真实本机 ICE/DTLS/SCTP 测试验证请求抵达对端，随后同一接收调用成功交付新的关键帧。
- `rd-engine` 已能把 protobuf 请求映射为 `SessionEvent::KeyframeRequested`；活动会话 actor 会转交媒体队列，队列拥塞时按流保留最新请求，并在离开 Active 时清除待处理请求。`rd-app` 的编码管道提供有界关键帧请求入口，按 stream ID 路由并在下一次 H.264 编码时调用 `request_keyframe()`；实测编码输出 IDR。
- `send_control`/`recv_control` 与 `Transport::send`/`recv` 仍是分开的 API。`rd-app` 已提供 `webrtc` feature 下的 `remote::attach_transport`：独立消费传输事件并驱动 engine actor，媒体 sender 可使用 `pipeline::spawn_with_keyframe_requests` 的返回值。连接附着只启动 Handshaking，不自动通过认证或能力协商。单流 H.264 网络收发适配已实现，见 [媒体适配记录](p2-remote-media-2026-10-15.md)；远程 UI 模式与持续捕获/显示任务仍待接入。
- 队列条目长度有界，但 str0m 内部、Fragment 编码的副本和完整帧拼装也占内存。队列有界不等于进程 RSS 已验收。

## 验证与后续

协议层 12 项测试通过，覆盖最大帧完整重组、乱序、重复、重放、缺片过期、重连实例隔离、容量拒绝、畸形长度、冲突片、控制消息跨 u16 回绕及 stream ID 保真。严格 clippy 通过。

app 关键帧管道 2 项测试通过，覆盖多 stream 请求隔离和真实 H.264 IDR 输出。engine 侧 11 项测试通过，覆盖 protobuf 解码、非法 stream ID 拒绝、活动会话转发媒体队列、媒体队列满时按流保留最新请求、离开活动态时清理待处理请求及原状态机路径。str0m 侧启用 `webrtc` 的 9 项测试通过，覆盖真实本机 ICE/DTLS/SCTP 上的双向传帧、跨片元数据、控制/视频独立通道、视频队列繁忙时控制消息投递、过期后请求关键帧并继续接收以及队列满关闭；默认 feature 下 transport 的 2 项测试也通过。此前完整 workspace 测试 66 项通过；三个 crate 的严格 clippy、`rd-app` 严格 clippy 与 `rd-protocol`/`rd-engine`/`rd-crypto` wasm 构建通过；格式检查和 `git diff --check` 通过。

下一步：总装持续捕获/显示任务与远程 UI 模式，补重连测试和分片消息上限能力协商，再做跨机器局域网联调并测量拥塞、队列峰值与 RSS。完整 P2 验收仍开放。

## 应用层桥接增量（2026-10-15）

- `take_events()` 移交独立事件接收器，使事件监听不再独占媒体传输句柄；worker 退出后接收器排空已缓存事件并结束。
- `SessionBridge::join()` 等待期间继续消费传输事件；`shutdown()` 显式结束会话，Drop 取消 actor 与转发任务。连接释放后事件流结束并发送 Disconnected。
- 单流媒体管线仅合并本流最新请求，丢弃其他 stream 的请求；测试复用生产实现。
- 组合测试使用真实本机 ICE/DTLS/SCTP，将 protobuf 请求经应用桥接、活动会话 actor 和生产管线请求处理函数送入 OpenH264，检查增量帧后的下一帧为 IDR。认证/能力事件是测试夹具，不能视为真实身份认证已完成。
- 可运行入口：`cargo test -p rd-app --features webrtc --lib remote::tests --locked`。当前 UDP 仍绑定 127.0.0.1，仅为本机开发验证。

应用集成时，在 tokio runtime 内使用：

```rust,ignore
let (frames, media_requests) = pipeline::spawn_with_keyframe_requests(config, repaint);
let bridge = remote::attach_transport(&mut transport, media_requests)
    .expect("transport events already owned");
// 真实握手、认证、能力协商完成时，由对应实现发送会话事件。
// frames 交给现有显示管线；transport 仍由调用方负责媒体收发。
// 结束会话：
bridge.shutdown().await?;
drop(frames);
drop(transport);
```

桥接对象不拥有传输句柄和显示接收器，因此 `shutdown()` 关闭的是 actor；调用方还需释放媒体管线和 transport。当前未实现自动重连。

本轮应用库验证：`cargo test -p rd-app --features webrtc --lib --locked` 17 项通过，包含 3 项应用桥接组合/生命周期测试。Windows 增量缓存曾报告访问拒绝，关闭增量编译（`CARGO_INCREMENTAL=0`）后编译和测试成功。

本轮最终验证：`cargo test --workspace --features rd-app/webrtc --locked` 共 76 项测试通过；`cargo clippy -p rd-app -p rd-transport --features rd-app/webrtc --all-targets --locked -- -D warnings`、`cargo fmt --all -- --check` 与 `git diff --check` 均通过。
