# P3 公网部署手册（v1-dev，单 VPS）

对应 docs/p3-plan-2026-10-15.md 的 W2 部署产物与 docs/adr/0008。当前为
**未经真机验证的部署清单**：本仓库 CI/本机测试覆盖到 mock TURN 与进程内
真实 rd-server 为止；coturn 真机联调、公网 NAT 矩阵与双家庭网络验收仍开放。

## 拓扑

一台 Linux VPS（公网 UDP 3478、TCP 80/443、UDP 49160-49200 放行）：

- **caddy**：443 上把 `wss://<域名>/ws` 反代给 rd-server，自动签发证书。
- **rd-server**：信令（`/ws`）、健康检查（`/healthz`）、TURN 凭证代签。
- **coturn**（host 网络）：3478/udp 同时提供 STUN 与 TURN；中继端口段
  49160-49200/udp；`use-auth-secret` + 共享密钥，不自建中继（ADR-0002）。
- **postgres**：v1-dev 尚未接线，为部署硬化阶段（P3 计划 §6 决策 5）预留。

## 首次部署

```bash
cd deploy
cp .env.example .env       # 改掉全部值；RDX_TURN_SECRET 用 openssl rand -hex 32
docker compose up -d --build
curl -fsS https://<域名>/healthz        # 期望输出 ok
```

自检清单：

1. `docker compose logs caddy` 无证书签发错误（DNS 未生效会失败）。
2. `docker compose logs coturn` 出现 `Listening port: 3478` 类就绪日志。
3. 客户端 `SignalingClient::connect("wss://<域名>/ws")` 注册成功且
   `RegisterResult.turn` 里的 `turn_addr`/`stun_addr` 是 `<域名>:3478`。
4. 强制 relay 自测：两端 `IceConfig { turn: Some(...), relay_only: true }`
   连接并传帧——等价于仓库内 mock 的
   `two_nodes_exchange_frames_over_forced_turn_relay`，但走真 coturn。

## TURN 密钥轮换

凭证格式 `username = "<unix 到期>:<设备 ID>"`、
`password = base64(HMAC-SHA1(secret, username))`，默认有效期 600s：

1. 生成新 secret，写 `.env` 的 `RDX_TURN_SECRET`。
2. `docker compose up -d`（rd-server 与 coturn 同时换新 secret）。
3. 旧凭证最多 600s 内仍被部分接受（两端各自重新注册即拿到新凭证）。
   窗口期内新旧混用会导致分配失败，客户端按重连处理。

避免单侧先换造成长时间不一致；需要无缝轮换时，给 coturn 配置
`static-auth-secret` 多值列表（本清单未启用）。

## 运维边界（P3 计划 §1/W4 的"不做"在这里同样生效）

- 服务器日志不记录 SDP/候选/信令载荷；排查靠设备 ID 与事件计数。
- 限速默认值在 `rd_server::ServerConfig`（100 signal/5s、64KiB 载荷上限、
  30s 心跳超时），调整后需重启 rd-server。
- v1 单实例、无水平扩展；presence 在 rd-server 内存中，重启后客户端
  重新注册即可恢复。
- TURN 中继带宽：coturn 配了 user-quota=12 / total-quota=1200（kBps），
  按 VPS 带宽自行调整；超出即分配失败，属容量问题不是故障。

## 已知开放项

- coturn 真机联调与 NAT 矩阵（P3 计划 W3）未执行。
- 双家庭网络真实验收（P3 计划 W4）未执行。
- Postgres 设备库与 ID 防抢占最小机制（P3 计划 §6 决策 2）未实做：
  当前 ID 离线即可被抢占，属已知限制。
- `wss` 证书依赖 caddy；客户端当前连接串须带 `wss://`（TLS 由 caddy
  终结，rd-server 自身只监听明文 8080，仅在 compose 内网可达）。
