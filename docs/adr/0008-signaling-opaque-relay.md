# ADR-0008: 信令服务只做注册/中转/在线，载荷不透明

- 状态：已接受（协议边界与实现；安全评审边界沿用 threat-model-v1）
- 日期：2026-10-15

## 背景

P3 需要公网建连：两端找到彼此（ID 寻址）、交换 SDP 文本与 ICE 候选、感知
对端在线状态。信令服务器按威胁模型属"不可信载荷"方，只可见元数据。此前
SDP/候选交换只存在于进程内测试设施 `connect_pair`。同时核查确认 str0m
0.24.1 不自带 TURN 客户端与候选收集（README「NIC enumeration and TURN」），
srflx/relay 收集成为本阶段新增工作量。

## 决策

- 消息面：`rd-protocol::signaling` prost 消息族（Register/RegisterResult/
  Signal/SignalError/Presence/Heartbeat），WebSocket 二进制帧承载；控制面
  protobuf 路线（ADR-0003）不动，媒体帧绝不进入信令。
- 服务器（`rd-server`，axum）只做三件事：ID 注册/寻址、Signal 中转、
  在线状态。**不解析 Signal 体**：SDP 以文本、候选以结构化字段原样转发，
  仅施加大小上限（64KiB）与每连接限速（100 条/5s）；日志不记录载荷。
- 在线状态：WS 连接即在线，30s 静默即清理；对某 ID 发过 Signal 即隐式
  订阅其上下线事件（Presence）。
- ID：`[a-z0-9]{4,32}`，客户端生成；在线期间重复注册拒绝（IdInUse），
  离线后可被抢占——防抢占最小机制（公钥自证）为后续工作，登记在
  P3 计划 §6 决策 2，不宣称任何认证语义。
- TURN 凭证：注册应答附带 coturn REST API 时限凭证（HMAC-SHA1 共享密钥
  代签，ADR-0002）；凭证与中继地址一起下发，客户端无需单独配置 TURN。
- 候选收集在客户端（`rd-transport`）：srflx 用自带最小 STUN Binding
  客户端；relay 用自带最小 TURN 客户端（allocate/CreatePermission/Refresh/
  Send/Data indication，长期凭证 401 交换；不做 channel binding、TCP/IPv6
  中继）。收集在构造函数内同步完成，候选随信令显式交换；`relay_only`
  全程走中继，`relay_fallback` 在直连超时后整体切换并上报
  `Str0mEvent::RelayFallback`。
- 存储：v1-dev 进程内存（devices + known_ids + presence），单实例；Postgres
  与按 ID 一致性哈希扩展留部署硬化阶段，协议已按无粘性 presence 设计。

## 后果

- 服务器实现面小且可测：mock TURN 与进程内真实 rd-server 的集成测试覆盖
  注册冲突、中转、在线/离线、限额、心跳与强制 relay 端到端。
- str0m 的"候选收集自理"意味着 STUN/TURN 客户端成为长期维护面；用最小
  子集（点对点单 allocation 单对端）控制风险，coturn 真机差异待部署联调
  证实——mock 的权限语义按 coturn 行为建模（无权限即丢弃）。
- 直连→中继回退是"整体切换"而非逐对择优：混合模式下 ICE 最终若选中直连
  对，路由仍可能走中继（回退一旦触发即粘滞）。v1 接受该简化，优化属 P3
  后续。对称 NAT 场景按 `relay_only`/回退路径验收。
- SDP 文本作信令载荷意味着服务器可见会话描述（媒体行、ICE ufrag），但
  这些本就不含密钥材料；端到端机密性由 DTLS/Noise 层保证，且 P4 认证
  未落地前公网连通不构成安全验收（threat-model-v1 §6）。
