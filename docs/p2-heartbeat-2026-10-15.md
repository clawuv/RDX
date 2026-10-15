# P2 独立心跳与断线检测（2026-10-15）

控制 protobuf 增加 Heartbeat（oneof tag 3），携带 u64 nonce 与 response 标记。Capabilities 增加 heartbeat（tag 7），当前实验配置要求对端显式支持，旧版本缺省 false 会被拒绝。线上格式仍未冻结。

`remote::exchange_capabilities` 成功验证对端后调用 `Str0mTransport::enable_heartbeat`。底层开发接口调用者也可显式启用，但必须先确认对端支持。心跳发送、回应和超时检测全部在 transport 专用线程运行，不依赖视频、应用读取普通控制消息或 UI 刷新；心跳不会占用应用的普通控制接收队列。

控制通道打开后立即发起一个探测，同一时刻仅保留一个待确认 nonce。匹配且未超时的响应到达后，等待 1 秒再发下一个；从探测入外发队列起 8 秒未收到匹配回应则关闭本地 worker。错误 nonce、重复旧响应与超时后的响应不能延长截止时间。心跳与回应优先进入有界控制外发队列；队列已满则关闭，避免静默积压。SCTP 仍为有序可靠传输，拥塞也可能导致超时，尚未实现自适应阈值。

关闭 worker 释放控制/视频接收通道，独立事件接收器结束；既有 SessionBridge 转发 Disconnected，actor 进入 Closed。连续媒体管线也会结束并关闭 UI 输出。静态桌面无视频时不触发视频接收超时，仍通过心跳判断连接存活。未启用心跳的开发 transport 保持原行为。

测试覆盖精确超时边界、错误/过期/重放 nonce、每次确认后的间隔，以及真实本机 ICE/DTLS/SCTP：应用不读取控制消息的空闲双端保持连接 10 秒；随后停止对端 worker 但保留 UDP 绑定，使其不回应也不产生立即的端口不可达捷径，本端须在 10 秒内结束接收。应用集成另验证对端退出后 SessionBridge 进入 Closed。

这不是实际拔网线或局域网双机验收。开发回环已补充[有界重连](p2-reconnect-2026-10-15.md)；生产重连、信令、认证与主 UI 总装仍待实现；P1 继续延期。恢复必须创建新 transport 和新媒体适配器，不复用已关闭对象。

同时修正已有缺片测试的时序：恢复帧改为在真实收到 KeyframeRequested 后发送，不再把握手开始前的固定 650ms 等待当作重组器已过期。

验收结果：`cargo test -p rd-protocol -p rd-engine -p rd-transport -p rd-app --features rd-app/webrtc --lib --locked` 全部通过，共 60 项（app 25、engine 11、protocol 13、transport 11）。相同包与 feature 的 `cargo clippy --all-targets --locked -- -D warnings`、`cargo fmt --all -- --check`、`git diff --check` 均通过。这是定向库回归，不代表完整工作区或真实断网验收。
