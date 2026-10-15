# P2 应用媒体网络适配（2026-10-15）

`rd-app::remote_media`（显式 `webrtc` feature）提供 `MediaSender` 与 `MediaReceiver`，把 OpenH264 的编码访问单元发送至 Transport，再在另一端解码成现有显示层使用的 `DecodedFrame`。调用方必须先完成会话认证及能力协商；这两个适配器本身不执行认证，也不自动进入 Active。

## 实验载荷

Frame 外层及分片结构保持原状，媒体载荷使用 `H26\x01`（4 字节）+ 访问单元编号（u64 大端）+ 采集时间戳（u64 大端）+ H.264 数据。编号每个发送器从零递增，低 16 位写入 Frame seq；编号耗尽必须重连，不能回绕。20 字节媒体头计入 16MiB 载荷上限，空访问单元、声明长度不符、版本不符和序号不符均拒绝。

这是实验格式，仍需纳入版本化能力协商，不能直接与此前裸 H.264 载荷混用。时间戳属于发送端时钟；跨机器未做时钟同步，不能直接与接收端时间相减作为端到端延迟。

## 收发与恢复

- 一个 MediaSender / MediaReceiver 对应一个 stream 和一个连接；重连需重新创建实例。
- send 每次接受一个编码访问单元，不增加应用层缓存。传输层继续使用已有有界队列和分片。发送出错应结束会话，不重试部分已发送的帧。
- receive 每次消费一个完整网络帧，返回 Some(decoded) 或 None（其他流、旧帧、重复帧、无法解码的依赖帧被丢弃）。调用方应持续调用，使 transport 执行分片过期检查。
- 首帧和丢帧后必须等待关键帧；关键帧恢复时重建解码器。解码错误会返回错误并进入等待关键帧状态。迟到旧帧不会回退接收状态。
- 新增 WebRTC 入口 `receive_webrtc`：缺口、首帧为增量帧或解码失败进入等待 IDR 时，通过可靠控制通道请求关键帧；在持续收到帧时最多每 250ms 重试，成功解码后清除限频状态。通用 `receive` 保留仅解码行为。缺片超时仍沿传输层请求 IDR；无后续帧到来时不触发定时重试。拥塞恢复与乱序容忍窗口后续补齐。
- 本轮未接入主 UI 的远程模式、后台捕获任务或退出联动。调用方负责采集/编码循环、请求队列消费、显示帧发布、关闭和超时。

## 可运行验证

```powershell
$env:CARGO_TARGET_DIR='target/p2-validation'
$env:CARGO_INCREMENTAL='0'
cargo test -p rd-app --features webrtc --lib remote_media::tests --locked
```

测试覆盖真实本机 ICE/DTLS/SCTP 上传输连续 H.264 帧并解码，保留分辨率与时间戳，跨越 u16 序号回绕；另以确定性丢整帧验证增量帧丢弃、重复帧拒绝、后续 IDR 恢复和畸形载荷拒绝。认证和跨机性能不属于该测试的通过范围。

上一增量验证：应用库 19 项测试、全部 targets 严格 clippy、格式及差异检查通过。本轮新增整帧丢失 → 可靠控制请求 → 活动会话媒体队列 → 强制 IDR → 接收恢复的真实本机 WebRTC 组合测试。测试认证事件仍为夹具。

本轮验证：`cargo test -p rd-app --features webrtc --lib --locked` 20 项通过；`cargo clippy -p rd-app --features webrtc --all-targets --locked -- -D warnings`、格式与差异检查通过。
