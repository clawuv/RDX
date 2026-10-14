# RDX 实施计划

- 状态：执行中（P1 第一步完成）
- P0：工程项本地完成（构建绿 / 测试全过 / clippy 无警告 / 核心 crate wasm 编译通过）；能力矩阵与威胁模型为草案待评审；Linux+Windows CI 待首推远端确认。
- P1 进度（2026-10-14）：**合成源回环垂直切片已跑通**——`cargo run -p rd-app --bin rd-loop`：合成捕获 → openh264 软编/软解 → egui/wgpu YUV 渲染。实测（M 系列 Mac，debug 构建，720p30）：编码 p50/p95 = 2.6/3.0ms，解码 = 2.1/2.4ms，端到端（捕获→上屏）= **6.1/6.8ms**（前台窗口），稳定 30fps——远优于 50ms 门槛，两大风险点（egui/wgpu 视频路径、openh264 集成）验证通过。
- P1 进度（2026-10-14，二）：**ScreenCaptureKit 捕获后端已接入**（`rd-loop --screen`）——`MacScreenCapturer`（420v/NV12 → 捕获边界去交织为 I420，SC 回调线程→定长通道→拉取端取最新），选型 `screencapturekit` v11（MIT，swift-bridge，macOS 构建需 Xcode/CLT；Swift 运行时 rpath 由 `.cargo/config.toml` 统一注入）。无 Screen Recording 权限时表现为显示器列表为空，已实现带指引的报错与合成源回退（已实测）。**剩余**：授权后真机帧数据验证（需用户在系统设置授予屏幕录制权限）、硬编路径与 GPU 帧互操作验证、release 构建复测；另记录一个待查项——后台/失焦窗口下 e2e 呈现 ~60ms（约 2 帧间隔的稳定相位偏移，编解码耗时不变，疑为呈现节流/拾取相位，需在前台窗口 + 真实捕获下复测）。
- 许可证：已定为 **MIT**（`LICENSE`）；为此移除了 AGPL 来源的 UI 壳（备份于仓库外 `../RDX-agpl-ui-backup/`），AGPL/GPL 依赖由 `deny.toml` 白名单机制禁止。git 历史中仍残留该 UI 代码（公开仓库），处理选项待定：接受历史保留（HEAD 已干净）或重写历史后强推。
- 制定日期：2026-10-13
- 架构决策记录：[docs/adr/](docs/adr/)
- 参考对象：RustDesk（经验与教训来源，见 ADR 各篇 Context）

**计划前提**：按 2 名有经验的 Rust 工程师估算工期；单人开发约乘 1.8；首发只做桌面三平台（Windows / macOS / Linux），Web 端与移动端靠后；每个里程碑以"可演示"为退出标准。

---

## 一、范围定义

**MVP（最小可用产品）= 第 4 阶段结束时的产物**：公网可用、带权限控制的远程桌面——捕获、编码、传输、显示、输入注入、经审阅的身份认证、端到端加密。

**明确不做进 v1**（防止对齐 RustDesk 全量功能的 scope 失控）：多会话并行、文件传输、终端、RDP 桥接、白板、地址簿同步、移动端。全部排在 MVP 之后，且每项独立评估是否值得做。

## 二、工作区布局

```
rd/  （实际目录名 RDX）
├── Cargo.toml              # workspace
├── crates/
│   ├── rd-protocol/        # protobuf 控制消息 + 裸帧格式 + 版本/能力协商
│   ├── rd-transport/       # Transport trait；str0m 实现；信令客户端
│   ├── rd-codec/           # 编解码 trait；openh264 软编 + 各平台硬编；dav1d/硬解
│   ├── rd-capture/         # 捕获 trait；dxgi / x11+damage / pipewire / screencapturekit
│   ├── rd-input/           # 注入 trait；SendInput / CGEvent / uinput+XTest
│   ├── rd-engine/          # 会话 actor、状态机、QoS、权限（纯逻辑，headless 可测）
│   ├── rd-crypto/          # opaque-ke 认证 + snow(Noise) 会话加密
│   ├── rd-broker/          # 特权能力代理：daemon + IPC + 安装器
│   ├── rd-audio/           # cpal 采集/播放 + opus
│   ├── rd-app/             # egui 应用（唯一二进制）
│   └── rd-server/          # 信令服务（axum + WebSocket + sqlx/postgres）
└── fuzz/                   # cargo-fuzz 目标：帧解析器、信令协议（P2 起进入 CI）
```

**关键约束（ADR-0006，从第一天生效）**：`rd-engine` 的纯会话逻辑以及 `rd-protocol`、`rd-crypto` 不依赖平台 crate；运行时适配与媒体/GPU 互操作单独验证 wasm 可行性。平台能力通过 trait 或明确的平台适配层注入，不假定所有能力都由 broker 提供。

### P0 必须补齐的验证门槛

- 增加 wasm32 CI 检查：先检查 `rd-protocol`、`rd-crypto`，并验证 `rd-engine` 的纯状态机部分可编译；若 Tokio actor 不兼容，则把状态机核心与运行时适配器拆开，不以放宽 wasm 约束解决。
- 完成并持续维护 [平台能力矩阵](docs/platform-capabilities.md)：逐项标出捕获、输入、音频、锁屏/UAC、所需权限、是否需要 broker、v1 不支持情形。仅把确需特权的操作放入 broker；用户会话 API、桌面会话限制和 portal 授权不可由“提权代理”假定绕过。
- 写出 v1 威胁模型与认证决策：明确服务器是否可信、设备身份如何绑定、口令猜测/重放/降级如何防护、Noise 身份如何认证。未经审阅的“口令派生密钥 + Noise”不得作为已满足认证要求的实现规格。
- 在 P2 冻结数据面格式前，定义编码访问单元、传输分片与接收端重组的边界，并用最大尺寸、过期、乱序、丢片、重连等场景做资源上限测试。

## 三、里程碑总览

| 阶段 | 内容 | 工期 | 累计 | 状态 |
|---|---|---|---|---|
| P0 | 骨架 + CI + ADR | 2 周 | 2 周 | ☐ |
| P1 | 单机视频垂直切片 | 6 周 | 8 周 | ☐ |
| P2 | 双端拆分 + 传输层（局域网） | 6 周 | 14 周 | ☐ |
| P3 | 信令服务器 + 打洞 + TURN（公网） | 4 周 | 18 周 | ☐ |
| P4 | 输入 + 权限 + 会话管理 → **MVP** | 4 周 | ~5 个月 | ☐ |
| P5 | 音频 / 剪贴板 / 文件传输 | 6 周 | 7 个月 | ☐ |
| P6 | 安全硬化（OPAQUE、审计、透明日志） | 4 周 | 8 个月 | ☐ |
| P7 | 打包 / 签名 / 自动更新 / i18n | 4 周+ | 9 个月 | ☐ |
| P8 | wasm Web 端（可与 P5 起并行） | 6-8 周 | ~10 个月 | ☐ |

## 四、各阶段详情

### P0：骨架（2 周）

- 建 workspace、crate 空壳、trait 定义（`Transport` / `Capturer` / `Encoder` / `InputInjector`）。
- CI：三平台构建 + clippy（-D warnings）+ `cargo test`；fuzz 目录占位，fuzz 目标自 P2 起进入 CI。
- deny.toml（依赖许可与版本审查，从第一天挡住"fork 依赖"老路）。
- 写 ADR-0001 ~ 0006，把已定决策固定成文档。
- **退出标准**：三平台 `cargo build` 绿；wasm 核心检查进入 CI；平台能力矩阵与 v1 威胁模型有明确结论；关键 ADR 与实现边界一致。

### P1：单机视频垂直切片（6 周）——全项目最高风险段，最先做

同机回环：捕获 → 编码 → 解码 → egui 显示，30fps@1080p。验证两件最没把握的事：**egui 视频路径性能**和**捕获/编码 trait 的接口形状**。

- 捕获：先做 Windows DXGI（脏矩形信息最全）+ macOS ScreenCaptureKit；Linux 先 X11（xcb damage），Wayland 排 P2。
- 编码：openh264 软编起步（基线永远可用），硬编按 MF / VideoToolbox / VAAPI 顺序接入；解码软解起步。
- egui 渲染（本项目最关键的技术点）：解码输出保持 NV12/I420，用 `egui_wgpu` 的 `CallbackTrait` 挂自定义 wgpu pass，YUV→RGB 在 shader 里做，CPU 侧零转换；纹理用 `Queue::write_texture` 上传；解码线程到 UI 用帧到达事件驱动 `ctx.request_repaint()`。
- **退出标准**：1080p30 端到端延迟（捕获到上屏）< 50ms，CPU 占用可接受；回环 demo 录屏。
- 延迟需用同一参考设备、固定分辨率/帧率和可复现素材测量；分别记录捕获、编码、上传、解码、呈现耗时及 p50/p95，不能只凭录屏观感验收。测试需至少覆盖软编基线和实际 GPU 上传路径。
- 该阶段先验证 GPU 帧互操作与真实纹理上传；“解码输出 NV12/I420”不等于硬件解码帧可零拷贝上传。若必须下载到 CPU，记录拷贝次数、带宽与 p95 延迟，并据此重估后续跨平台工作量。
- **风险**：egui/wgpu 版本 API 变动（钉版本）；若 `CallbackTrait` 路径受阻，降级方案是 `TextureHandle` + CPU 转 RGB（慢但能跑），不阻塞后续阶段。

### P2：双端 + 传输层（6 周）

拆出受控端/控制端两个角色（同一二进制的两种模式），引入 `rd-engine` 的会话 actor。

- 消息面：protobuf（prost），帧面候选格式 `[stream:u8][flags:u8][seq:u16][len:u32]`，**帧绝不经 protobuf**。在下列分片规格验收前不得宣称格式定稿；当前头部没有足以标识无序分片索引的信息。
- str0m 集成：str0m 是同步的用户态 WebRTC，跑在专用线程上按定时器轮询，与 tokio 之间只走 channel——先做一个 spike 确认这个桥接模式（本阶段第一周出结论，它是最大不确定点）。
- 数据通道策略：控制/输入走有序可靠 DataChannel；视频帧走**无序 + 零重传**的 DataChannel（迟到帧直接丢弃，等下一关键帧），这是低延迟的关键取舍。
- 在 Transport 内将编码访问单元切成符合协商消息上限的块；每块需携带可关联帧序号与分片索引/总数（或等价的末片可验证方案）。重组器必须有每会话和每帧字节上限、并发帧上限和明确过期时间；丢任一分片即丢整帧，过期后释放缓存并请求关键帧。控制与媒体分通道还需验证控制消息不会被大媒体写入造成的队列积压拖延。
- 测试乱序、重复、丢片、超大长度、超时、序号回绕及断线重连；记录重组缓存峰值和丢帧恢复延迟。只有这些语义确定后再冻结 ADR-0003 的线上布局。
- 会话状态机：显式枚举状态（`Idle → Handshaking → Authenticating → CapabilityExchange → Active → Draining → Closed`）+ 转移函数；每会话一个 tokio task + 收件箱。
- 局域网内两台机器 WebRTC 直连跑通。
- **退出标准**：局域网 1080p30，端到端延迟 < 80ms，拔网线 10 秒内触发状态机转移到断线态并可重连。

### P3：公网可用（4 周）

- `rd-server` 信令服务：axum + WebSocket，只做三件事——ID 注册/寻址、SDP/ICE 中转、在线状态。存储 Postgres（sqlx）。按 ID 一致性哈希预留水平扩展，v1 单实例。
- coturn 部署：TURN 凭证用 HMAC 时限 REST API 签发（信令服务代签），不自建中继。
- NAT 打洞验证矩阵：至少覆盖锥形 NAT × 对称 NAT × TURN 兜底三类组合（云上多区域节点造环境）。
- **退出标准**：两个不同家庭网络的机器成功连接，对称 NAT 场景 100% 走 TURN 且可用。

### P4：输入 + 权限 + 会话管理 → MVP（4 周）

- `rd-input`：Windows SendInput（UAC/锁屏限制按能力矩阵验收）、macOS CGEvent（辅助功能权限引导 UI 必须做好）、Linux X11 使用 XTest；Wayland 优先验证 RemoteDesktop portal，只有明确需要且桌面环境支持时才评估 uinput，不把 uinput 当作通用 Wayland 注入方案。
- `rd-broker` 是否需要按平台安装服务（Windows 服务 / systemd / launchd）由 P0 能力矩阵决定；IPC schema 版本化（interprocess crate）。只暴露该平台确需的最小能力，不解析会话消息、不联网、只接受本机特定 UID/管道 ACL 的连接。
- 权限模型：每会话可回收的键盘/鼠标/剪贴板权限，控制端发起、受控端确认（首次连接弹确认对话框）。
- 认证与加密按 P0 威胁模型决策落地。禁止自行拼接“口令哈希/派生密钥 + Noise”并将其视为已认证；如 v1 必须支持口令，采用经过审阅的 PAKE 组合并定义限速/重试策略。若该方案无法在 P4 前审阅完成，先缩小 MVP 的远程身份/配对承诺，不降低认证强度。
- egui 主界面成型：ID 展示、连接输入、会话窗口、权限开关、断线提示。
- **退出标准（MVP）**：公网 + 中继场景下完整走通"连接 → 认证 → 看画面 → 鼠标键盘操控 → 挂断"，第三方朋友可试用。

### P5：音频 / 剪贴板 / 文件传输（6 周，可两人并行拆 3 条线）

- 音频：cpal 采集 + Opus（20ms 帧），播放端 jitter buffer（先简单的 60ms 固定缓冲，实测再调）。
- 剪贴板：arboard，文本先行，文件剪贴板排后。
- 文件传输：复用可靠 DataChannel，分块 + SHA-256 校验 + 断点续传；egui 传输管理界面。
- Wayland 捕获补课：ashpd 走 xdg-desktop-portal ScreenCast + pipewire-rs，接受"每次授权弹窗"的 v1 体验。

### P6：安全硬化（4 周）

- 按 P0 威胁模型完成选定 PAKE/设备认证方案的安全硬化；若选 OPAQUE（opaque-ke），保护服务端注册记录并完成注册、恢复和设备绑定流程。设备公钥目录/透明日志需有客户端一致性验证和回滚处理，不以公布 merkle head 单独作为完成标准。
- fuzz 补全：帧解析器、信令消息、IPC schema 各一个 fuzz 目标进 CI。
- 外部安全审计准备：威胁模型文档 + broker 权限最小化复核。
- 双栈确认：全链路 DTLS/Noise，抓包验证媒体面无明文。

### P7：发布工程（4 周，与 P6 部分并行）

- cargo-dist 出三平台安装包；Windows EV 签名 + macOS 公证（预留 1-2 周磨合）；Linux AppImage + deb/rpm。
- 自动更新：ed25519 签名的 JSON manifest，v1 全量更新即可。
- i18n：i18n-embed + fluent，中英先行。
- 崩溃上报：opt-in，默认关闭。

### P8：wasm Web 端（6-8 周，可自 P5 起安排第二人并行）

- 依赖 P0-P4 验证的 wasm 核心边界；协议、加密和可移植会话逻辑编 wasm，运行时、传输、解码与渲染按浏览器能力适配。
- 能力层换实现：Transport 用浏览器原生 WebRTC（wasm-bindgen，不是 str0m）；解码用 WebCodecs（VideoDecoder 喂 AV1/H.264）；捕获/输入在 Web 端不存在（纯控制端）。
- UI 复用 `rd-app` 的 egui（egui 原生支持 wasm），信令走 WebSocket。
- **退出标准**：浏览器里免安装控制一台原生受控端，1080p 可用。

## 五、贯穿性工作（不属于任何阶段）

- **测试金字塔**：engine 层 headless 单测（状态机转移表全覆盖）；回放测试（录制会话消息序列 → 确定性重放进引擎断言状态）；每里程碑一次真实网络演练。不追求 UI 自动化。
- **性能预算表**：每阶段 demo 量一次（延迟、fps、CPU、内存），超预算即在本阶段内解决，不带病前进。
- **ADR 流程**：所有偏离本计划的决策补 ADR，防止架构被"临时方案"侵蚀。

## 六、风险登记表（按杀伤力排序）

| 风险 | 概率 | 影响 | 对策 |
|---|---|---|---|
| egui 视频路径性能不达标 | 中 | 高 | P1 最先验证；有 TextureHandle 降级路径 |
| str0m 与 tokio 桥接/成熟度问题 | 中 | 高 | P2 第一周 spike；最坏情况换 libwebrtc 绑定，Transport trait 隔离了爆炸半径 |
| Wayland 捕获/注入体验差 | 高 | 中 | 接受 portal 授权弹窗的 v1 体验，文档说明；不追求绕过 |
| macOS 权限引导劝退用户 | 高 | 中 | P4 专做引导 UI + 检测状态 |
| 实时 AV1 软编不可用 | 高 | 低 | AV1 只做硬编，软编基线定为 H.264，计划内已有 |
| 签名/公证流程拖延发布 | 中 | 中 | P7 提前买证书、提前走通一轮 |
| 两人并行度不足 | 中 | 中 | P5 起 engine/平台两条线可拆；P8 可插第二人 |

## 七、汇总

- **MVP（公网可用的远程桌面）：当前约 5 个月 / 2 人，仅作目标而非承诺。** 该估算依赖 P1 GPU 路径、P2 分片/重连、P0 平台权限与认证决策验证通过；阶段估时需在这些 spike 后重估，并单列跨平台验收、发布准备与风险缓冲。
- **对齐 RustDesk 基础功能集（含音频、文件传输、剪贴板）：约 8 个月。**
- **含 Web 端、发布工程在内的一代完整版本：约 10 个月 / 2 人**；单人约 15-18 个月到 MVP+基础功能。

计划里刻意保留了两个"尽早证伪"的设计：P1 的 egui 视频垂直切片和 P2 首周的 str0m spike——这两个是全项目唯一没有先例背书的技术点，其余都是在抄成熟的作业。
