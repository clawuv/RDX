# ADR-0002: 传输只做两条路——str0m WebRTC + coturn TURN

- 状态：已接受
- 日期：2026-10-13

## 背景

RustDesk 并行维护四条传输（TCP/KCP/WebSocket/WebRTC），并为打洞、拥塞控制各自造轮子，还长期维护 20 个官方 fork 的依赖（含打过补丁的 webrtc-rs）。每条通道都要单独测打洞、拥塞、断线恢复。

## 决策

- 主路 WebRTC，实现选 str0m（同步用户态、无 libwebrtc 重量）；备路中继用 coturn（TURN），不自建中继。ICE/STUN/打洞/拥塞控制全部复用标准生态。
- Web 端（P8）在同一 `Transport` trait 下换浏览器原生 WebRTC 实现。
- str0m 是同步的：跑在专用线程上按定时器轮询，与 tokio 之间只走 channel；P2 第一周做 spike 验证此桥接。
- 数据通道策略：控制/输入走有序可靠 DataChannel；视频帧走无序 + 零重传 DataChannel，迟到帧直接丢弃等待下一关键帧。

## 后果

- 不再需要 KCP/自研打洞/自建中继，维护面大幅收窄。
- 依赖上游 str0m 的成熟度；若不可用，退路是 libwebrtc 绑定，`Transport` trait 已隔离爆炸半径。
- TURN 部署与凭证签发（HMAC 时限 REST API，信令服务代签）成为部署侧的新增工作。
