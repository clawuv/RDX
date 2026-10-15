# 协议模糊测试

独立 cargo-fuzz 工程，不改变主工作区的默认构建。`protocol` 检查裸帧、protobuf 控制消息和有状态 RDF1 重组器：错误输入不消费帧缓冲、合法帧往返一致、缓存不超过 32MiB、过期后释放缓存。

分片操作编码：`[时间增量:u8][包长:u16 大端][包内容]`，时间单位 4ms。每次操作先过期再接收包；输入结束推进 TTL 并验证缓存归零。种子包括合法帧、关键帧请求、完成/重放/未完成分片及超时。单输入内不模拟真实网络或编解码器。

稳定版运行种子、逐字节变异与截断回归：

```powershell
cargo test --manifest-path fuzz/Cargo.toml --lib --locked
```

Linux + nightly（cargo-fuzz/AddressSanitizer 环境）：

```sh
cargo install cargo-fuzz --version 0.13.1 --locked
cargo +nightly fuzz run protocol -- -max_total_time=30 -max_len=65536 -rss_limit_mb=512 -timeout=5
```

CI 每次 push/PR 执行 30 秒有界冒烟并保留失败样本；这不等同长期模糊测试或安全审计。可增加时间开展持续测试。复现使用 `cargo +nightly fuzz run protocol fuzz/artifacts/protocol/<样本文件>`。信令和 broker IPC schema 尚未实现，相关目标待后续添加。
