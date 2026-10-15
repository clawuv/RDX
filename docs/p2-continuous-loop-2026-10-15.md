# P2 连续 WebRTC 回环（2026-10-15）

`remote_pipeline::spawn_loopback` 将连续采集、H.264 编码、真实本机 UDP/ICE/DTLS/SCTP 传输、重组解码和最新帧显示槽连接起来；`rd-loop` 可选择该路径，复用现有 YUV GPU 渲染。默认使用合成源，Windows 可通过 `RDX_CAPTURE_DISPLAY` 选择 DXGI 真屏来源。

PowerShell 运行方式：

```powershell
$env:RDX_LOOP_WEBRTC='1'
$env:RDX_LOOP_SECONDS='5'
cargo run -p rd-app --features webrtc --bin rd-loop --locked
```

Windows 真屏模式在上述命令前增加 `$env:RDX_CAPTURE_DISPLAY='0'`（索引按枚举顺序）；恢复时按原显示器名称匹配，不跳到其他显示器。非法索引或不可用显示器会关闭输出并报错，不回退合成源。其他平台暂不支持此网络入口的真屏模式。不带 `RDX_LOOP_WEBRTC=1` 时保持原有单机编解码回环；未启用 feature 时开发入口报错退出。

双方先完成固定配置能力交换再发媒体。生产端按设定帧率运行、跳过错过的节拍；恢复事件经容量 32 的请求队列反馈编码器。解码端通过单槽 watch 交给 UI，慢 UI 不积累解码帧。所有 UI 接收器释放时取消连接建立和运行中的异步任务，传输对象随之释放；发送连续等待超过 5 秒则结束管线。接收不按视频静默时间判定断线，以允许静态桌面和采集恢复；连接存活由 transport 独立心跳检测。网络关闭或超时先执行最多 3 次有界重连，期间清空画面；不可重试错误或重连耗尽关闭输出，已有 UI 停止提示继续生效。详见[开发回环重连](p2-reconnect-2026-10-15.md)，生产会话重连仍未实现。

采集 Timeout 表示暂时无新帧，继续采集；DisplayChanged 清空当前 UI 帧并按 250/500/1000/2000/4000ms 退避重试最多 5 次。恢复后重新验证偶数非零尺寸，重建编码器发出 SPS/PPS 和 IDR，媒体编号保持单调。恢复耗尽或其他捕获错误结束管线。该逻辑复用已有采集恢复函数，不改动 P1 的验收状态。

时间戳与 UI 共用进程单调起点，现有 capture→upload 指标可用于本机开发诊断；不是跨机时钟同步或真实呈现延迟。输出 LoopFrame 的 encode_ms 为 0（未传输编码耗时），decode_ms 包含接收等待，不能作为纯解码性能指标。

后续增量已补齐独立心跳：能力交换后自动启用，静态桌面仍保持连接，探测 8 秒无匹配响应则关闭 transport 和媒体任务。详见 [心跳与断线检测](p2-heartbeat-2026-10-15.md)。

此模式双方在同一进程，使用本机回环 UDP。它直接转发开发链路的关键帧事件，不伪造 AuthOk，也不代表已完成生产认证、Active 门控、远程主 UI 或局域网双机验收。P1 剩余验收继续延期。

最新验收：应用库 24 项测试通过（含真实 WebRTC 连续传输、显示端暂停 350ms 后只读取最新帧、关闭接收端后管线退出、非法配置拒绝，以及静止 5.2 秒后模拟显示配置变化，从 128×96 恢复至 160×120 并完成网络解码）；`cargo clippy -p rd-app --features webrtc --all-targets --locked -- -D warnings` 通过。模拟配置变化不是实际拔插显示器验收。

Windows debug 带窗口实跑：`RDX_LOOP_WEBRTC=1 RDX_LOOP_SECONDS=5`，退出码 0，5.01 秒内上传 87 帧、独立绘制 87 帧（约 17.4 帧/秒），`pipeline_stopped=false`。capture→upload submit p50/p95=189.94/243.48ms；capture→GPU done=200.84/271.69ms。这仅确认窗口渲染调用与网络管线接通，未进行图像目视验收，也不满足性能退出标准；不能据此宣称 1080p30、P1 或 P2 已通过。

Windows 真屏 debug 实跑：增加 `RDX_CAPTURE_DISPLAY=0`、限时 8 秒，退出码 0，8.03 秒内上传和独立绘制均为 116 帧，`pipeline_stopped=false`。capture→upload submit p50/p95=216.86/311.14ms；capture→GPU done=252.72/391.29ms。当前桌面与回环窗口同屏，无受控动态素材及目视画质验收，仅证明 DXGI→WebRTC→GPU 路径可运行，不能作为帧率或延迟达标依据。
