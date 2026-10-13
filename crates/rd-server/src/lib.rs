//! 信令服务（P3 落地）：axum + WebSocket。
//!
//! 只做三件事：ID 注册/寻址、SDP/ICE 中转、在线状态。存储 Postgres（sqlx）。
//! TURN 凭证由本服务代签（HMAC 时限 REST API），中继本体是 coturn，不自建。
