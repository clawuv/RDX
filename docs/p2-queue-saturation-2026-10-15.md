# P2 视频接收队列饱和验证（2026-10-15）

新增真实本机 WebRTC 回归 saturated_video_receiver_is_bounded_and_worker_exits。双方等待视频通道打开后，经 UDP/ICE/DTLS/SCTP 持续发送合法单分片帧，接收侧不读取视频。每片数据为 12KiB 帧加 21 字节 RDF1 信封，发送间隔 1ms，最多尝试 4096 片，worker 退出即停止，整体等待上限 15 秒。

断言接收队列积累到固定 256 项后 worker 退出，而非无限缓冲或阻塞；控制发送返回 Closed；独立生命周期事件流在保留 transport 句柄时仍结束；随后排空 256 个完整帧并核对内容，再次接收返回 Closed，重组缓存归零。用共享 video_ready 测试辅助函数等待双方真实 ChannelOpen，避免握手前零重传首帧丢失。

单项首次运行通过，耗时 4.23 秒。该场景验证既有溢出即关闭策略，没有修改为丢最旧帧或自动降码率。溢出时已排队数据仍可读出，至多 256 项；对象丢弃时释放队列。单视频接收队列的协议数据上限为 256×(12288+21)=3,151,104 字节，约 3.01MiB，不含 Vec、队列元数据、SCTP/DTLS 内部缓存、其他外发/控制队列及编解码器；不能据此推断整个会话 RSS。

验证：启用 rd-app/webrtc 的四库定向测试 66 项通过（app 28、engine 11、protocol 13、transport 14）；transport/app all-targets 严格 clippy、全工作区格式与 diff 检查通过。测试不是带宽整形、真实网络拥塞、跨机长稳或进程 RSS 验收；P2 这些缺项仍保留，P1 继续暂缓。
