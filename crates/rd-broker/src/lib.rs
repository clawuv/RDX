//! 特权能力代理（ADR-0004，P4 落地）。
//!
//! 约束（安全审查清单，落地时逐条验证）：
//! - 能力面只有 capture / input / audio 三个接口；
//! - 不解析会话消息、不联网；
//! - 只接受本机特定 UID / 管道 ACL 的连接；
//! - IPC schema 版本化（interprocess crate）。
