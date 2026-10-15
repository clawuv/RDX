//! Application-level bridge between WebRTC transport events and the session
//! actor. Media keyframe actions are delivered through the pipeline's bounded
//! request queue.

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use rd_engine::{SessionEvent, SessionState};
use rd_protocol::control::KeyframeRequest;
use rd_transport::{Str0mEvent, Str0mEventReceiver, Str0mTransport};

/// Exchange fixed-profile capabilities after authentication, before media starts.
/// Caller exclusively owns ordinary control reception during this exchange.
/// Failure requires closing this connection; never reuse a timed-out exchange.
pub async fn exchange_capabilities(
    transport: &mut Str0mTransport,
) -> crate::remote_media::MediaResult<rd_protocol::control::Capabilities> {
    use rd_protocol::control::{control_message::Kind, Capabilities, ControlMessage};
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        transport
            .send_control(
                &ControlMessage {
                    kind: Some(Kind::Capabilities(Capabilities::local())),
                }
                .encode(),
            )
            .await?;
        let message = ControlMessage::decode_wire(&transport.recv_control().await?)?;
        let Some(Kind::Capabilities(peer)) = message.kind else {
            return Err("expected initial capabilities message".into());
        };
        peer.validate_peer()?;
        transport.enable_heartbeat();
        Ok(peer)
    })
    .await?
}

pub struct SessionBridge {
    events: mpsc::Sender<SessionEvent>,
    actor: JoinHandle<SessionState>,
    forwarder: JoinHandle<()>,
}

impl SessionBridge {
    /// Feed connection/authentication/capability events into the session actor.
    pub async fn send_event(&self, event: SessionEvent) -> Result<(), SessionEvent> {
        self.events.send(event).await.map_err(|error| error.0)
    }

    /// Wait for Disconnected, authentication failure, or DrainDone.
    /// Transport events remain live while waiting.
    pub async fn join(mut self) -> Result<SessionState, tokio::task::JoinError> {
        (&mut self.actor).await
    }

    /// Close the session and wait for the actor to terminate.
    pub async fn shutdown(self) -> Result<SessionState, tokio::task::JoinError> {
        let _ = self.send_event(SessionEvent::Disconnected).await;
        self.join().await
    }
}

impl Drop for SessionBridge {
    fn drop(&mut self) {
        self.forwarder.abort();
        self.actor.abort();
    }
}

/// Attach the transport's independently owned event stream to a session actor.
/// The media sender is normally returned by `pipeline::spawn_with_keyframe_requests`.
/// Returns None if another consumer already took the transport event stream.
pub fn attach_transport(
    transport: &mut Str0mTransport,
    media_requests: mpsc::Sender<KeyframeRequest>,
) -> Option<SessionBridge> {
    let receiver = transport.take_events()?;
    let (events, event_rx) = mpsc::channel(256);
    // Attaching an existing transport begins the connection, not authentication.
    events
        .try_send(SessionEvent::Connect)
        .expect("empty session queue");
    let actor = tokio::spawn(rd_engine::run_with_keyframe_requests(
        event_rx,
        media_requests,
    ));
    let forwarder = tokio::spawn(forward_transport_events(receiver, events.clone()));
    Some(SessionBridge {
        events,
        actor,
        forwarder,
    })
}

async fn forward_transport_events(
    mut receiver: Str0mEventReceiver,
    events: mpsc::Sender<SessionEvent>,
) {
    while let Some(event) = receiver.recv().await {
        let session_event = match event {
            Str0mEvent::KeyframeRequested {
                stream_id,
                after_frame_id,
            } => Some(SessionEvent::KeyframeRequested {
                stream_id,
                after_frame_id,
            }),
            Str0mEvent::KeyframeNeeded { .. } => None,
            Str0mEvent::ChannelOpen(_, _)
            | Str0mEvent::IceConnected
            | Str0mEvent::RelayFallback => None,
        };
        if let Some(event) = session_event {
            if events.send(event).await.is_err() {
                return;
            }
        }
    }
    let _ = events.send(SessionEvent::Disconnected).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use rd_protocol::control::ControlMessage;
    use rd_transport::connect_pair;

    #[tokio::test]
    async fn peers_exchange_real_webrtc_capabilities_and_reject_incompatible_peer() {
        let (mut a, mut b) = connect_pair().unwrap();
        let (a_result, b_result) =
            tokio::join!(exchange_capabilities(&mut a), exchange_capabilities(&mut b));
        assert_eq!(
            a_result.unwrap(),
            rd_protocol::control::Capabilities::local()
        );
        assert!(b_result.is_ok());
        let (mut a, mut b) = connect_pair().unwrap();
        let mut incompatible = rd_protocol::control::Capabilities::local();
        incompatible.max_video_message = 1;
        b.send_control(
            &ControlMessage {
                kind: Some(rd_protocol::control::control_message::Kind::Capabilities(
                    incompatible,
                )),
            }
            .encode(),
        )
        .await
        .unwrap();
        assert!(exchange_capabilities(&mut a).await.is_err());
    }

    #[tokio::test]
    async fn transport_request_forces_pipeline_encoder_idr() {
        let (mut peer, mut receiver) = connect_pair().unwrap();
        let (media_tx, mut media_rx) = mpsc::channel(1);
        let bridge = attach_transport(&mut receiver, media_tx).unwrap();
        for event in [
            SessionEvent::HandshakeDone,
            SessionEvent::AuthOk,
            SessionEvent::CapabilitiesAgreed,
        ] {
            bridge.send_event(event).await.unwrap();
        }

        use rd_codec::{CodecId, Encoder as _, EncoderConfig, OpenH264Encoder};
        let mut encoder = OpenH264Encoder::new(EncoderConfig {
            codec: CodecId::H264,
            bitrate_kbps: 800,
            framerate: 30,
            keyframe_interval: 1000,
        })
        .unwrap();
        let mut frame = rd_capture::CapturedFrame {
            width: 128,
            height: 96,
            format: rd_capture::PixelFormat::I420,
            data: bytes::Bytes::from(vec![128; 128 * 96 * 3 / 2]),
            damage: Vec::new(),
            timestamp_us: 0,
        };
        assert!(encoder.encode(&frame).await.unwrap()[0].keyframe);
        frame.timestamp_us = 33_333;
        assert!(!encoder.encode(&frame).await.unwrap()[0].keyframe);

        let request = ControlMessage::keyframe_request(3, 65_536).encode();
        peer.send_control(&request).await.unwrap();
        let recovered_after = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(id) =
                    crate::pipeline::apply_keyframe_requests(&mut encoder, 3, &mut media_rx)
                {
                    break id;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(recovered_after, 65_536);
        frame.timestamp_us = 66_666;
        assert!(encoder.encode(&frame).await.unwrap()[0].keyframe);

        bridge.send_event(SessionEvent::Disconnected).await.unwrap();
        assert_eq!(bridge.join().await.unwrap(), SessionState::Closed);
    }

    #[tokio::test]
    async fn transport_close_finishes_waiting_bridge() {
        let (_peer, mut receiver) = connect_pair().unwrap();
        let (media_tx, _media_rx) = mpsc::channel(1);
        let bridge = attach_transport(&mut receiver, media_tx.clone()).unwrap();
        assert!(attach_transport(&mut receiver, media_tx).is_none());
        drop(receiver);
        let state = tokio::time::timeout(Duration::from_secs(3), bridge.join())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state, SessionState::Closed);
    }

    #[tokio::test]
    async fn peer_silence_closes_session_within_ten_seconds() {
        let (mut peer, mut receiver) = connect_pair().unwrap();
        let (a, b) = tokio::join!(
            exchange_capabilities(&mut peer),
            exchange_capabilities(&mut receiver)
        );
        a.unwrap();
        b.unwrap();
        let (media_tx, _media_rx) = mpsc::channel(1);
        let bridge = attach_transport(&mut receiver, media_tx).unwrap();
        // No fake authentication: loss must also terminate pre-auth sessions.
        let start = std::time::Instant::now();
        drop(peer);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), bridge.join())
                .await
                .unwrap()
                .unwrap(),
            SessionState::Closed
        );
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(receiver.recv_control().await.is_err());
    }

    #[tokio::test]
    async fn shutdown_and_drop_release_media_sender() {
        let (_peer, mut receiver) = connect_pair().unwrap();
        let (media_tx, mut media_rx) = mpsc::channel(1);
        let bridge = attach_transport(&mut receiver, media_tx).unwrap();
        assert_eq!(bridge.shutdown().await.unwrap(), SessionState::Closed);
        assert!(media_rx.recv().await.is_none());

        let (_peer, mut receiver) = connect_pair().unwrap();
        let (media_tx, mut media_rx) = mpsc::channel(1);
        let bridge = attach_transport(&mut receiver, media_tx).unwrap();
        drop(bridge);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), media_rx.recv())
                .await
                .unwrap()
                .is_none()
        );
    }
}
