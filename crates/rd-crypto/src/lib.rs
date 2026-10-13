//! 认证与会话加密（ADR-0005）：OPAQUE（opaque-ke）认证 + Noise（snow）会话加密。
//!
//! 落地节奏：P4 先上对称口令 + Noise 会话密钥（v1 可用）；P6 换 OPAQUE 与透明日志。
//! 本 crate 在落地前保持空置，接口届时随威胁模型文档一起定稿。
