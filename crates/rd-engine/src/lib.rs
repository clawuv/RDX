//! 会话引擎（ADR-0004）：纯逻辑、无平台依赖、headless 可测（ADR-0006）。
//!
//! 每个会话是一个 actor：tokio task + 事件收件箱，状态由显式枚举与转移表驱动，
//! 不允许出现 RustDesk `connection.rs` 式的隐式状态。

pub mod state;

use std::collections::BTreeMap;

pub use state::{IllegalTransition, SessionEvent, SessionState};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ControlEventError {
    #[error("control message has no recognized kind")]
    Unknown,
    #[error("keyframe request stream id {0} is outside the frame protocol range")]
    InvalidStreamId(u32),
}

impl TryFrom<rd_protocol::control::ControlMessage> for SessionEvent {
    type Error = ControlEventError;

    fn try_from(message: rd_protocol::control::ControlMessage) -> Result<Self, Self::Error> {
        match message.kind {
            Some(rd_protocol::control::control_message::Kind::KeyframeRequest(request)) => {
                let stream_id = u8::try_from(request.stream_id)
                    .map_err(|_| ControlEventError::InvalidStreamId(request.stream_id))?;
                Ok(Self::KeyframeRequested {
                    stream_id,
                    after_frame_id: request.after_frame_id,
                })
            }
            _ => Err(ControlEventError::Unknown),
        }
    }
}

/// 会话 actor 主循环：消费事件、推进状态，进入 Closed 即退出。
/// 返回最终状态，供测试与上层汇报。
pub async fn run(mut events: tokio::sync::mpsc::Receiver<SessionEvent>) -> SessionState {
    run_inner(&mut events, None).await
}

/// Run a session actor and forward active-session keyframe requests to the
/// application/media pipeline. The caller translates these to encoder actions.
pub async fn run_with_keyframe_requests(
    mut events: tokio::sync::mpsc::Receiver<SessionEvent>,
    requests: tokio::sync::mpsc::Sender<rd_protocol::control::KeyframeRequest>,
) -> SessionState {
    run_inner(&mut events, Some(requests)).await
}

async fn run_inner(
    events: &mut tokio::sync::mpsc::Receiver<SessionEvent>,
    requests: Option<tokio::sync::mpsc::Sender<rd_protocol::control::KeyframeRequest>>,
) -> SessionState {
    let mut state = SessionState::Idle;
    // At most one pending request per stream; a newer incomplete access unit
    // supersedes an older one while the bounded media queue is full.
    let mut pending = BTreeMap::<u8, u64>::new();
    loop {
        let event = if let (Some(requests), false) = (&requests, pending.is_empty()) {
            tokio::select! {
                event = events.recv() => event,
                permit = requests.reserve() => {
                    match permit {
                        Ok(permit) => {
                            let (stream_id, after_frame_id) = pending.pop_first().unwrap();
                            permit.send(rd_protocol::control::KeyframeRequest {
                                stream_id: u32::from(stream_id),
                                after_frame_id,
                            });
                        }
                        Err(_) => {
                            tracing::warn!("media handler closed while keyframe recovery was pending");
                            state = SessionState::Closed;
                            break;
                        }
                    }
                    continue;
                }
            }
        } else {
            events.recv().await
        };
        let Some(event) = event else { break };
        if let SessionEvent::KeyframeRequested {
            stream_id,
            after_frame_id,
        } = event
        {
            if state == SessionState::Active {
                if requests.is_some() {
                    pending
                        .entry(stream_id)
                        .and_modify(|id| *id = (*id).max(after_frame_id))
                        .or_insert(after_frame_id);
                } else {
                    tracing::warn!(
                        stream_id,
                        after_frame_id,
                        "keyframe request has no media handler"
                    );
                }
            } else {
                tracing::warn!(
                    ?state,
                    stream_id,
                    after_frame_id,
                    "ignoring keyframe request outside active session"
                );
            }
            continue;
        }
        match state.advance(event) {
            Ok(next) => {
                state = next;
                if state != SessionState::Active {
                    pending.clear();
                }
            }
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
    async fn active_keyframe_request_reaches_media_handler_without_state_change() {
        let (tx, rx) = mpsc::channel(16);
        let (request_tx, mut request_rx) = mpsc::channel(1);
        let handle = tokio::spawn(run_with_keyframe_requests(rx, request_tx));
        for event in [
            SessionEvent::Connect,
            SessionEvent::HandshakeDone,
            SessionEvent::AuthOk,
            SessionEvent::CapabilitiesAgreed,
            SessionEvent::KeyframeRequested {
                stream_id: 9,
                after_frame_id: 65_536,
            },
        ] {
            tx.send(event).await.unwrap();
        }
        let request = request_rx.recv().await.unwrap();
        assert_eq!(request.stream_id, 9);
        assert_eq!(request.after_frame_id, 65_536);
        tx.send(SessionEvent::Disconnected).await.unwrap();
        assert_eq!(handle.await.unwrap(), SessionState::Closed);
    }

    #[tokio::test]
    async fn full_media_queue_preserves_latest_request_per_stream() {
        let (tx, rx) = mpsc::channel(16);
        let (request_tx, mut request_rx) = mpsc::channel(1);
        request_tx
            .try_send(rd_protocol::control::KeyframeRequest {
                stream_id: 99,
                after_frame_id: 0,
            })
            .unwrap();
        let handle = tokio::spawn(run_with_keyframe_requests(rx, request_tx));
        for event in [
            SessionEvent::Connect,
            SessionEvent::HandshakeDone,
            SessionEvent::AuthOk,
            SessionEvent::CapabilitiesAgreed,
            SessionEvent::KeyframeRequested {
                stream_id: 9,
                after_frame_id: 10,
            },
            SessionEvent::KeyframeRequested {
                stream_id: 9,
                after_frame_id: 11,
            },
        ] {
            tx.send(event).await.unwrap();
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while tx.capacity() != 16 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(request_rx.recv().await.unwrap().stream_id, 99);
        let request = tokio::time::timeout(std::time::Duration::from_secs(1), request_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request.stream_id, 9);
        assert_eq!(request.after_frame_id, 11);
        tx.send(SessionEvent::Disconnected).await.unwrap();
        assert_eq!(handle.await.unwrap(), SessionState::Closed);
    }

    #[tokio::test]
    async fn draining_session_discards_pending_keyframe_requests() {
        let (tx, rx) = mpsc::channel(16);
        let (request_tx, mut request_rx) = mpsc::channel(1);
        request_tx
            .try_send(rd_protocol::control::KeyframeRequest {
                stream_id: 99,
                after_frame_id: 0,
            })
            .unwrap();
        let handle = tokio::spawn(run_with_keyframe_requests(rx, request_tx));
        for event in [
            SessionEvent::Connect,
            SessionEvent::HandshakeDone,
            SessionEvent::AuthOk,
            SessionEvent::CapabilitiesAgreed,
            SessionEvent::KeyframeRequested {
                stream_id: 9,
                after_frame_id: 10,
            },
            SessionEvent::Drain,
        ] {
            tx.send(event).await.unwrap();
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while tx.capacity() != 16 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(request_rx.recv().await.unwrap().stream_id, 99);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), request_rx.recv())
                .await
                .is_err()
        );
        tx.send(SessionEvent::DrainDone).await.unwrap();
        assert_eq!(handle.await.unwrap(), SessionState::Closed);
    }

    #[test]
    fn protobuf_keyframe_request_maps_to_session_event() {
        let message = rd_protocol::control::ControlMessage::keyframe_request(5, 65_536);
        let event = SessionEvent::try_from(message).unwrap();
        assert_eq!(
            event,
            SessionEvent::KeyframeRequested {
                stream_id: 5,
                after_frame_id: 65_536
            }
        );
        let invalid = rd_protocol::control::ControlMessage {
            kind: Some(
                rd_protocol::control::control_message::Kind::KeyframeRequest(
                    rd_protocol::control::KeyframeRequest {
                        stream_id: 256,
                        after_frame_id: 1,
                    },
                ),
            ),
        };
        assert_eq!(
            SessionEvent::try_from(invalid),
            Err(ControlEventError::InvalidStreamId(256))
        );
    }

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
