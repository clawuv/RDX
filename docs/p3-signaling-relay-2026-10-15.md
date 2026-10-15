# P3 开发进展：信令服务、TURN 客户端与部署产物（2026-10-15）

按 [P3 执行计划](p3-plan-2026-10-15.md) 推进的 v1-dev 增量。**不宣布 P3
验收通过**：公网真机联调（coturn、NAT 矩阵、双家庭网络）与部署硬化仍开放。
ADR-0008 固化了本增量的协议与服务边界决策。

## rd-protocol：信令消息（17 项测试通过，含原控制面）

`signaling.rs`：prost 手写 derive（无 protoc 依赖，wasm 边界不受影响）。
Register/RegisterResult/Signal（SDP 文本与候选）/SignalError（类型化错误码）/
Presence/Heartbeat + TurnCredential。SDP 以 `str0m` 的 `to_sdp_string` 文本
作载荷，服务器不解析。

## rd-server：信令服务（10 项测试，严格 clippy 通过）

axum + WS，`/ws` 与 `/healthz`；内存 devices/known_ids/presence，v1 单实例。
实测覆盖：注册冲突/非法 ID、不透明中转、未知/离线对端类型化错误、
watcher 上下线事件、64KiB 超限不断连、限速、静默清理、心跳保活。
TURN 凭证按 coturn REST API 规范签发（HMAC-SHA1，独立向量测试）。

## rd-transport：候选收集与 TURN 客户端（34 项测试，严格 clippy 通过）

- `stun.rs`：最小 RFC 5389 Binding 客户端（XOR-MAPPED-ADDRESS 优先、重传
  预算、mock STUN 服务器实测）。
- `turn.rs` + `turn/mock.rs`：最小 TURN 客户端（allocate 401→长期凭证、
  CreatePermission、Refresh、Send/Data indication）与进程内 mock TURN
  服务器（权限强制、双 allocation 内部中继）。HMAC/长度调整用独立向量
  测试；错误码/地址编解码全覆盖。
- `str0m_impl.rs`：`IceConfig`（stun/turn/relay_only/relay_fallback）+
  `LocalCandidates`；`Plane` 持有直连与 TURN 双 socket、按策略路由、
  中继到达包以**中继候选地址**为 destination 喂入（`is` 代理会丢弃
  目的地不匹配 Host/Relayed 候选的请求——调试实测确认）；`add_remote_candidate`
  命令通道；`Str0mEvent::RelayFallback` 生命周期事件。
- 关键端到端测试：`two_nodes_exchange_frames_over_forced_turn_relay`
  （双端 relay_only 经真实 ICE/DTLS/SCTP 全程 mock TURN 中继双向传帧——
  **计划首周 spike 的目标验证**）；`relay_fallback_engages_when_direct_never_connects`
  （直连不通 300ms 后整体切中继并传帧）。排查该测试时确认：事件接收端
  被丢弃会使 worker 在下一次事件上报时按设计退出（观测者消失即关闭），
  测试须持续排水事件。
- `signaling.rs`：`SignalingClient`（connect/register/signal/heartbeat/recv）
  与 `dial`/`answer` 编排（offer/answer/候选交换、150ms 候选排水窗）；
  对**进程内真实 rd-server** 集成测试：双端经 wss 信令建连并双向传帧、
  未知对端类型化错误、重复注册拒绝。

## deploy/（清单就绪，未经真机验证）

docker-compose（caddy WSS + rd-server + coturn host 网络 + postgres 预留）、
rd-server.Dockerfile、.env.example、部署手册（含 TURN 密钥轮换窗口说明与
自检清单）。开放项在手册中逐条列出。

## 与计划对照

| 计划项 | 状态 |
|---|---|
| W1 双 spike（TURN 路线 + 信令协议） | ✅ 本机验证（mock TURN；真 coturn 待部署） |
| W1 rd-server 骨架 + 集成测试 | ✅ |
| W2 VPS 生产化部署 | ☐ 产物就绪，未上真机 |
| W2 客户端完整候选注入 + ICE restart | 部分（候选注入 ✅；ICE restart 重连未做，仍属 P2 联动缺项） |
| W3 NAT 矩阵 + netem | ☐ 需云上环境 |
| W4 硬化与真实验收 | 部分（服务器侧限额/心跳 ✅；TURN 凭证过期刷新、rd-app UI 状态未做） |

## 遗留与风险

- coturn 真机行为与 mock 的差异（nonce 轮换、438 stale-nonce、凭证到期
  拒绝）未经实测；TURN 客户端未处理 438 重试。
- `relay_fallback` 粘滞切换的简化（ADR-0008 已记录）；混合模式择优未做。
- ID 离线抢占、Postgres 设备库、TURN/TCP 443：均为计划 §6 已登记的后续项。
- P2 收口项（跨机、重连、拥塞/丢包矩阵）仍开放；本增量的"跨机"仍是本机
  双实例 + mock/真实进程内服务器。
- 观察到 `rd-app` 既有真实 ICE 测试 `real_webrtc_video_decodes_and_
  preserves_timestamps_across_sequence_wrap` 在并行满载下偶发 10s 超时
  （隔离 3/3、串行 28/28 通过），属 P2 WIP 测试的时序余量问题，非本增量
  回归；后续应放宽该类真实网络测试的时限或串行化执行。
