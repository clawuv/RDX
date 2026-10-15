//! 信令服务（P3，ADR-0008）：axum + WebSocket。
//!
//! 只做三件事：ID 注册/寻址、SDP/ICE 中转（载荷不透明）、在线状态。
//! TURN 凭证由本服务代签（HMAC 时限 REST API），中继本体是 coturn，不自建。
//!
//! v1-dev：注册与在线状态为进程内存态，单实例；Postgres 设备库与水平
//! 扩展留待部署硬化（见 docs/p3-plan-2026-10-15.md §6）。服务器不可信
//! 载荷：不解析 Signal 体、不记录任何载荷日志（threat-model-v1 §2）。

pub mod app;
pub mod turn;

pub use app::{app, ServerConfig};

use std::time::Duration;

/// Minimal deployment defaults; production values come from the environment.
pub fn config_from_env() -> ServerConfig {
    fn env(name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    }
    ServerConfig {
        turn_secret: env("RDX_TURN_SECRET")
            .unwrap_or_else(|| "dev-insecure-turn-secret".to_string())
            .into_bytes(),
        turn_addr: env("RDX_TURN_ADDR").unwrap_or_else(|| "127.0.0.1:3478".to_string()),
        stun_addr: env("RDX_STUN_ADDR").unwrap_or_else(|| "127.0.0.1:3478".to_string()),
        credential_ttl: Duration::from_secs(600),
        heartbeat_timeout: Duration::from_secs(30),
        max_signal_payload: 64 * 1024,
        signal_rate: 100,
        signal_rate_window: Duration::from_secs(5),
    }
}

/// 绑定并服务直到 listener 出错；测试与二进制共用。
pub async fn serve(listener: tokio::net::TcpListener, config: ServerConfig) -> std::io::Result<()> {
    axum::serve(listener, app(config)).await
}
