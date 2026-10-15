//! 会话状态机：显式状态 + 转移表（ADR-0004）。
//!
//! ```text
//! Idle ──Connect──▶ Handshaking ──HandshakeDone──▶ Authenticating ──AuthOk──▶ CapabilityExchange
//!                     │                                │                        │
//!              Disconnected│                 AuthFailed│Disconnected    CapabilitiesAgreed│
//!                     ▼            ▼                       ▼                        ▼
//!                    Closed      Closed                  Closed                  Active
//!                                                                        Drain│      │Disconnected
//!                                                                             ▼      ▼
//!                                                                         Draining  Closed
//!                                                                    DrainDone│Disconnected
//!                                                                             ▼
//!                                                                           Closed
//! ```

/// 非法转移：事件在当前状态下无对应转移。actor 侧记录后忽略，不终止会话。
#[derive(Debug, thiserror::Error)]
#[error("illegal transition: {from:?} on {event:?}")]
pub struct IllegalTransition {
    pub from: SessionState,
    pub event: SessionEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Handshaking,
    Authenticating,
    CapabilityExchange,
    Active,
    Draining,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEvent {
    Connect,
    HandshakeDone,
    AuthOk,
    AuthFailed,
    CapabilitiesAgreed,
    Drain,
    DrainDone,
    Disconnected,
    /// A peer requested a fresh keyframe after losing a video access unit.
    KeyframeRequested {
        stream_id: u8,
        after_frame_id: u64,
    },
}

impl SessionState {
    /// 唯一的转移入口。新增状态或事件必须同步修改本表与单元测试。
    pub fn advance(self, event: SessionEvent) -> Result<Self, IllegalTransition> {
        use SessionEvent as E;
        use SessionState as S;
        let next = match (self, event) {
            (S::Idle, E::Connect) => S::Handshaking,
            (S::Handshaking, E::HandshakeDone) => S::Authenticating,
            (S::Handshaking, E::Disconnected) => S::Closed,
            (S::Authenticating, E::AuthOk) => S::CapabilityExchange,
            (S::Authenticating, E::AuthFailed) | (S::Authenticating, E::Disconnected) => S::Closed,
            (S::CapabilityExchange, E::CapabilitiesAgreed) => S::Active,
            (S::CapabilityExchange, E::Disconnected) => S::Closed,
            (S::Active, E::Drain) => S::Draining,
            (S::Active, E::KeyframeRequested { .. }) => S::Active,
            (S::Active, E::Disconnected) => S::Closed,
            (S::Draining, E::DrainDone) | (S::Draining, E::Disconnected) => S::Closed,
            (from, event) => {
                return Err(IllegalTransition { from, event });
            }
        };
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path() {
        let mut s = SessionState::Idle;
        for (event, expect) in [
            (SessionEvent::Connect, SessionState::Handshaking),
            (SessionEvent::HandshakeDone, SessionState::Authenticating),
            (SessionEvent::AuthOk, SessionState::CapabilityExchange),
            (SessionEvent::CapabilitiesAgreed, SessionState::Active),
            (SessionEvent::Drain, SessionState::Draining),
            (SessionEvent::DrainDone, SessionState::Closed),
        ] {
            s = s.advance(event).unwrap();
            assert_eq!(s, expect);
        }
    }

    #[test]
    fn auth_failed_closes() {
        let s = SessionState::Idle
            .advance(SessionEvent::Connect)
            .unwrap()
            .advance(SessionEvent::HandshakeDone)
            .unwrap()
            .advance(SessionEvent::AuthFailed)
            .unwrap();
        assert_eq!(s, SessionState::Closed);
    }

    #[test]
    fn disconnect_from_any_live_state_closes() {
        for state in [
            SessionState::Idle, // Idle 无连接，Disconnected 非法，见下一测试
            SessionState::Handshaking,
            SessionState::Authenticating,
            SessionState::CapabilityExchange,
            SessionState::Active,
            SessionState::Draining,
        ] {
            if state == SessionState::Idle {
                continue;
            }
            assert_eq!(
                state.advance(SessionEvent::Disconnected).unwrap(),
                SessionState::Closed
            );
        }
    }

    #[test]
    fn illegal_transitions_rejected() {
        // Idle 上除 Connect 外全部非法。
        for event in [
            SessionEvent::HandshakeDone,
            SessionEvent::AuthOk,
            SessionEvent::AuthFailed,
            SessionEvent::CapabilitiesAgreed,
            SessionEvent::Drain,
            SessionEvent::DrainDone,
            SessionEvent::Disconnected,
        ] {
            assert!(SessionState::Idle.advance(event).is_err());
        }
        // Active 上不能重新握手。
        assert!(SessionState::Active.advance(SessionEvent::AuthOk).is_err());
        // Closed 是终态。
        for event in [
            SessionEvent::Connect,
            SessionEvent::Drain,
            SessionEvent::Disconnected,
        ] {
            assert!(SessionState::Closed.advance(event).is_err());
        }
    }
}
