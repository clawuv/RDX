# 阶段复核：2026-10-14（Windows）

## 结论

P0 未完全验收，P1 进行中，P2 暂不启动。设置页、主题和窗口外观属于 UI 壳，不能替代真实捕获、联网、输入与认证能力。

## 本轮核对

- 11 个 workspace crate、6 篇 ADR、三平台与 wasm CI 定义存在。
- 当前实现：合成 I420 捕获、openh264 编解码、wgpu 上传/绘制、内存通道、会话状态机。
- 未实现：真实 DXGI / ScreenCaptureKit / X11 捕获、硬编互操作、str0m 网络、信令服务、输入注入和完整认证。
- 平台能力矩阵与威胁模型仍为草案；三平台远端 CI 未在本次确认。

## 本轮开发与验证

1. 基线 22 项单元测试通过；严格 clippy 暴露两处 UI 属性/测试写法问题，已修复。
2. 捕获原来使用自身起点，上传侧使用独立起点；现共享一个 Instant，时间戳提前至合成帧生成开始。新增回归测试验证非零起点。
3. 将无界 mpsc 改为 watch 最新帧槽；消费者停顿时覆盖旧帧，关闭消费者后生产者退出。新增慢消费者/断开测试。
4. 解码计时改为每个 packet 单独起算。原 e2e 标签更正为 capture_to_upload_submit：Queue::write_texture 返回不代表 GPU 完成或屏幕呈现，不能用于 <50ms P1 验收。
5. 回环默认改为 1920×1080、30fps、8Mbps；Windows 默认 DX12。
6. 修改后 24 项单元测试通过，严格 clippy 通过。测试过程中出现过一次 Windows 增量缓存目录拒绝访问警告，测试成功，后续 clippy 成功。

## 环境与复现

- Windows / Rust-Cargo 1.95 / i5-9600K / Intel UHD 630，驱动 26.20.100.7584；另有 Oray 虚拟显示驱动。
- cargo test --workspace --locked
- cargo clippy --workspace --all-targets --locked -- -D warnings
- cargo build --workspace --release --locked
- cargo run -p rd-app --bin rd-loop --release
- cargo build -p rd-protocol -p rd-engine -p rd-crypto --target wasm32-unknown-unknown --locked

## 后续顺序

1. Windows DXGI 真屏捕获与显示器枚举、失效恢复，接入现有回环。
2. 分别测量捕获、编码、解码、CPU 上传提交、GPU/呈现；记录丢帧、CPU/内存、拷贝次数，验证 1080p30 并录屏。
3. 硬编/GPU 互操作及 macOS/Linux 验证；完成 P0 评审与 CI 门槛。
4. P1 达标后再推进 P2 str0m spike 与分片重组。

## 本机 release 复测结果

全 workspace release 构建成功。运行 rd-loop 约 22 秒，窗口响应正常，正常关闭退出码 0。原始日志保存在 target/p1-1080p-release.log（构建目录，未纳入版本管理）。

- 三个管道窗口 fps：30.0 / 35.3 / 30.1。35.3 暗示固定节拍追赶，不能据此断言稳定实时输出。
- 编码 p50：16.54–16.76ms，p95：18.14–21.70ms。
- 解码 p50：2.37–2.38ms，p95：3.43–4.76ms。
- 上传提交延迟末次 p50/p95：24.9/42.8ms；各窗口 p95：41.3–55.0ms。
- UI 上传 fps：0.2–30.0，波动明显；窗口遮挡、前后台状态和重绘调度尚未受控，不宣称稳定 1080p30 或上屏延迟达标。
- 采样结束工作集 415,178,752 bytes，进程累计 CPU 11.34 秒；不是完整内存峰值或归一化 CPU 使用率。
- wasm 目标初始未安装；本次 rustup 下载长时间无进展，已终止本次安装进程，wasm 本机复验标记为未完成（不是编译失败）。

下一轮第一优先级：在固定前台/无遮挡条件下复测，分离逻辑更新、上传、绘制与呈现计数，定位 UI 低帧率及合成源追赶行为；随后接入 DXGI。P1 仍未通过，不进入 P2。
