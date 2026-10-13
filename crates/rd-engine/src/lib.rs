//! 会话引擎（ADR-0004）：纯逻辑、无平台依赖、headless 可测（ADR-0006）。
//!
//! 每个会话是一个 actor：tokio task + 事件收件箱，状态由显式枚举与转移表驱动，
//! 不允许出现 RustDesk `connection.rs` 式的隐式状态。

pub mod state;

pub use state::{IllegalTransition, SessionEvent, SessionState};

/// 会话 actor 主循环：消费事件、推进状态，进入 Closed 即退出。
/// 返回最终状态，供测试与上层汇报。
pub async fn run(mut events: tokio::sync::mpsc::Receiver<SessionEvent>) -> SessionState {
    let mut state = SessionState::Idle;
    while let Some(event) = events.recv().await {
        match state.advance(event) {
            Ok(next) => state = next,
            Err(err) => tracing::warn!(?err, "ignoring illegal session event"),
        }
        if state == SessionState::Closed {
            break;
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn run_walks_happy_path_to_closed() {
        let (tx, rx) = mpsc::channel(16);
        let handle = tokio::spawn(run(rx));
        for event in [
            SessionEvent::Connect,
            SessionEvent::HandshakeDone,
            SessionEvent::AuthOk,
            SessionEvent::CapabilitiesAgreed,
            SessionEvent::Drain,
            SessionEvent::DrainDone,
        ] {
            tx.send(event).await.unwrap();
        }
        assert_eq!(handle.await.unwrap(), SessionState::Closed);
    }

    #[tokio::test]
    async fn run_stops_on_auth_failure() {
        let (tx, rx) = mpsc::channel(16);
        let handle = tokio::spawn(run(rx));
        tx.send(SessionEvent::Connect).await.unwrap();
        tx.send(SessionEvent::HandshakeDone).await.unwrap();
        tx.send(SessionEvent::AuthFailed).await.unwrap();
        assert_eq!(handle.await.unwrap(), SessionState::Closed);
    }

    #[tokio::test]
    async fn run_ignores_illegal_event_and_continues() {
        let (tx, rx) = mpsc::channel(16);
        let handle = tokio::spawn(run(rx));
        tx.send(SessionEvent::Connect).await.unwrap();
        // Active 之前的 AuthOk 是非法事件，应被忽略而不是终止会话。
        tx.send(SessionEvent::AuthOk).await.unwrap();
        tx.send(SessionEvent::HandshakeDone).await.unwrap();
        tx.send(SessionEvent::Disconnected).await.unwrap();
        assert_eq!(handle.await.unwrap(), SessionState::Closed);
    }
}
