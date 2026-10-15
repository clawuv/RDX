//! str0m 桥接（ADR-0002）：`Rtc` 与全部 UDP socket 归一个专用线程，
//! tokio 侧只通过 channel 交互（命令进、事件出），线程内以
//! poll_output/Transmit/Timeout 驱动。crypto provider 进程内安装一次。
//!
//! P3：`IceConfig` 注入 STUN/TURN——srflx 收集与 TURN 中继由本 crate 自带
//! 的最小客户端完成（str0m 不做候选收集）；SDP 与候选经信令通道交换。
//! relay_only 全程走中继；relay_fallback 在直连迟迟不通时整体切到中继。
//!
//! 已接入实验性分片并保留 Frame 元数据；控制与视频使用独立通道。
//! 缺片过期会通知本地并通过控制通道请求对端关键帧；重连仍待实现。

use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bytes::Bytes;
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::channel::{ChannelConfig, ChannelId, Reliability};
use str0m::crypto;
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::Mutex;

use rd_protocol::control::{control_message, ControlMessage};
use rd_protocol::fragment::{fragment, FragmentError, Reassembler, CHUNK, ENVELOPE};
use rd_protocol::signaling::{Candidate as SignalCandidate, CandidateKind};
use rd_protocol::Frame;

use crate::turn::TurnClient;
use crate::{Transport, TransportError};

const CONTROL_LABEL: &str = "rdx-control";
const VIDEO_LABEL: &str = "rdx-video";
pub const MAX_CONTROL_MESSAGE: usize = 16 * 1024;
/// Separate queues hold at most 256 messages/fragments each. Full receive
/// queues close the transport rather than blocking its worker.
const EVENT_BUFFER: usize = 256;
const WRITE_BATCH: usize = 16;

/// ICE servers and relay policy for one transport endpoint. The default is
/// the historical loopback behavior: one host candidate, no STUN/TURN.
#[derive(Debug, Clone, Default)]
pub struct IceConfig {
    /// STUN endpoint used to gather a server-reflexive candidate on the
    /// media socket. Gathering failure is non-fatal and just skips srflx.
    pub stun: Option<SocketAddr>,
    pub turn: Option<TurnConfig>,
    /// Only gather and use the TURN relayed candidate; host candidates are
    /// not added. Use to validate the relay path end to end.
    pub relay_only: bool,
    /// Switch all routing to the relay when ICE has not connected within
    /// this duration after the first remote candidate (symmetric-NAT
    /// fallback). `None` never falls back.
    pub relay_fallback: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct TurnConfig {
    pub addr: SocketAddr,
    /// TURN REST API credential issued by the signaling server.
    pub username: String,
    pub password: String,
}

/// Local candidates ready to be advertised over signaling.
#[derive(Debug, Clone, Default)]
pub struct LocalCandidates {
    pub host: Vec<SocketAddr>,
    pub srflx: Option<SocketAddr>,
    pub relayed: Option<SocketAddr>,
}

impl LocalCandidates {
    pub fn to_signaling(&self) -> Vec<SignalCandidate> {
        let mut advertised = Vec::with_capacity(self.host.len() + 2);
        for addr in &self.host {
            advertised.push(SignalCandidate::udp(
                CandidateKind::Host,
                addr.to_string(),
                None,
            ));
        }
        if let Some(mapped) = self.srflx {
            advertised.push(SignalCandidate::udp(
                CandidateKind::Srflx,
                mapped.to_string(),
                None,
            ));
        }
        if let Some(relayed) = self.relayed {
            advertised.push(SignalCandidate::udp(
                CandidateKind::Relayed,
                relayed.to_string(),
                None,
            ));
        }
        advertised
    }
}

fn decode_signal_candidate(info: &SignalCandidate) -> Option<Candidate> {
    let addr: SocketAddr = info.addr.parse().ok()?;
    let related: Option<SocketAddr> = info.related.parse().ok();
    let base = related.unwrap_or(addr);
    match info.candidate_kind()? {
        CandidateKind::Host => Candidate::host(addr, "udp").ok(),
        CandidateKind::Srflx => Candidate::server_reflexive(addr, base, "udp").ok(),
        CandidateKind::Relayed => Candidate::relayed(addr, base, "udp").ok(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ChannelKind {
    Control,
    Video,
}

enum Cmd {
    Send(Bytes),
    AddRemote(SignalCandidate),
}

struct EventSenders {
    lifecycle: mpsc::Sender<Str0mEvent>,
    control: mpsc::Sender<Vec<u8>>,
    video: mpsc::Sender<Vec<u8>>,
}

struct SignalWorkerExit(Arc<AtomicBool>);

struct WorkerControl {
    stop: Arc<AtomicBool>,
    heartbeat_enabled: Arc<AtomicBool>,
}

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(8);

struct HeartbeatState {
    pending: Option<(u64, Instant)>,
    next_probe: Instant,
    next_nonce: u64,
}

impl HeartbeatState {
    fn new(now: Instant) -> Self {
        Self {
            pending: None,
            next_probe: now,
            next_nonce: 1,
        }
    }

    fn poll(&mut self, now: Instant) -> Result<Option<rd_protocol::control::Heartbeat>, ()> {
        if let Some((_, sent)) = self.pending {
            return if now.duration_since(sent) >= HEARTBEAT_TIMEOUT {
                Err(())
            } else {
                Ok(None)
            };
        }
        if now < self.next_probe {
            return Ok(None);
        }
        let nonce = self.next_nonce;
        self.next_nonce = nonce.checked_add(1).ok_or(())?;
        self.pending = Some((nonce, now));
        Ok(Some(rd_protocol::control::Heartbeat {
            nonce,
            response: false,
        }))
    }

    fn acknowledge(&mut self, nonce: u64, now: Instant) {
        if self.pending.is_some_and(|(expected, sent)| {
            expected == nonce && now.duration_since(sent) < HEARTBEAT_TIMEOUT
        }) {
            self.pending = None;
            self.next_probe = now + HEARTBEAT_INTERVAL;
        }
    }
}

fn heartbeat_bytes(heartbeat: rd_protocol::control::Heartbeat) -> Bytes {
    ControlMessage {
        kind: Some(control_message::Kind::Heartbeat(heartbeat)),
    }
    .encode()
    .into()
}

impl Drop for SignalWorkerExit {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// 专用线程向 tokio 侧推送的事件；非 Frame 事件用于观测连接生命周期。
#[derive(Debug)]
pub enum Str0mEvent {
    ChannelOpen(ChannelId, String),
    IceConnected,
    /// Direct routing never connected and all media switched to the TURN
    /// relay (see `IceConfig::relay_fallback`).
    RelayFallback,
    KeyframeNeeded {
        stream_id: u8,
        after_frame_id: u64,
    },
    KeyframeRequested {
        stream_id: u8,
        after_frame_id: u64,
    },
}

/// Owns every UDP socket of one endpoint: the direct media socket plus an
/// optional TURN allocation socket, and the routing decision between them.
struct Plane {
    direct: UdpSocket,
    direct_local: SocketAddr,
    turn: Option<TurnClient>,
    turn_local: SocketAddr,
    relay_only: bool,
    relay_fallback: Option<Duration>,
    /// Sticky once set: either relay_only from the start or a fallback hit.
    relay_deadline: Option<Instant>,
    using_relay: bool,
    pending_switch_notice: bool,
}

impl Plane {
    fn new(direct: UdpSocket, direct_local: SocketAddr) -> Self {
        Self {
            direct,
            direct_local,
            turn: None,
            turn_local: direct_local,
            relay_only: false,
            relay_fallback: None,
            relay_deadline: None,
            using_relay: false,
            pending_switch_notice: false,
        }
    }

    fn direct_socket(&self) -> &UdpSocket {
        &self.direct
    }

    fn install_turn(&mut self, client: TurnClient, config: &IceConfig) {
        self.turn_local = client.socket_local_addr();
        self.relay_only = config.relay_only;
        self.relay_fallback = config.relay_fallback;
        self.using_relay = config.relay_only;
        self.turn = Some(client);
    }

    /// First remote candidate arms the fallback timer, if configured.
    fn note_remote(&mut self) {
        if self.relay_deadline.is_none() && !self.relay_only {
            if let Some(fallback) = self.relay_fallback {
                self.relay_deadline = Some(Instant::now() + fallback);
            }
        }
    }

    fn mark_connected(&mut self) {
        self.relay_deadline = None;
    }

    fn route_relay(&mut self, now: Instant) -> bool {
        if self.using_relay {
            return true;
        }
        if let Some(deadline) = self.relay_deadline {
            if now >= deadline {
                self.using_relay = true;
                self.pending_switch_notice = true;
                return true;
            }
        }
        false
    }

    /// One-shot notice that routing fell back to the relay.
    fn take_switch_notice(&mut self) -> bool {
        std::mem::take(&mut self.pending_switch_notice)
    }

    /// Inbound relayed packets are addressed to the relayed candidate, not to
    /// the TURN control socket; the ICE agent drops anything else.
    fn relay_destination(&self) -> Option<SocketAddr> {
        self.turn.as_ref().map(|client| client.relayed_address())
    }

    fn send(&mut self, destination: SocketAddr, contents: &[u8]) {
        let relay = self.route_relay(Instant::now());
        if std::env::var("RDX_TRACE").is_ok() {
            eprintln!("send -> {destination} relay={relay} len={}", contents.len());
        }
        let server = self.turn.as_ref().map(|client| client.server_address());
        if relay && server != Some(destination) {
            if let Some(client) = self.turn.as_mut() {
                if client.ensure_permission(destination).is_ok()
                    && client.send_data(destination, contents).is_ok()
                {
                    return;
                }
            }
        }
        let _ = self.direct.send_to(contents, destination);
    }

    /// One non-blocking direct-socket read; `None` when nothing arrived.
    fn read_direct(&self, buffer: &mut [u8]) -> Option<(SocketAddr, usize)> {
        match self.direct.recv_from(buffer) {
            Ok((length, source)) => Some((source, length)),
            Err(_) => None,
        }
    }
}

/// Separately owned event stream so an application can send/receive media
/// while it observes control and lifecycle events from the same transport.
pub struct Str0mEventReceiver {
    events: mpsc::Receiver<Str0mEvent>,
    worker_done: Arc<AtomicBool>,
}

impl Str0mEventReceiver {
    pub async fn recv(&mut self) -> Option<Str0mEvent> {
        loop {
            if self.worker_done.load(Ordering::Acquire) {
                return self.events.try_recv().ok();
            }
            match tokio::time::timeout(Duration::from_millis(50), self.events.recv()).await {
                Ok(event) => return event,
                Err(_) => continue,
            }
        }
    }
}

fn install_crypto() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| crypto::from_feature_flags().install_process_default());
}

/// Build the endpoint's RTC, socket plane, and gathered local candidates.
/// srflx gathering failure is non-fatal; TURN allocation failure is fatal
/// because an explicitly configured relay is a hard requirement.
fn build_rtc(config: &IceConfig) -> Result<(Rtc, Plane, LocalCandidates), anyhow::Error> {
    install_crypto();
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let addr = socket.local_addr()?;
    let mut rtc = Rtc::builder().build(Instant::now());
    let mut plane = Plane::new(socket, addr);
    let mut candidates = LocalCandidates::default();
    if !config.relay_only {
        rtc.add_local_candidate(Candidate::host(addr, "udp").expect("host candidate"))
            .expect("add host candidate");
        candidates.host.push(addr);
        if let Some(stun) = config.stun {
            if let Ok(mapped) = crate::stun::request_mapped_address(plane.direct_socket(), stun) {
                if mapped != addr {
                    rtc.add_local_candidate(
                        Candidate::server_reflexive(mapped, addr, "udp").expect("srflx candidate"),
                    )
                    .expect("add srflx candidate");
                    candidates.srflx = Some(mapped);
                }
            }
        }
    }
    if let Some(turn) = &config.turn {
        let turn_socket = UdpSocket::bind("127.0.0.1:0")?;
        let client = TurnClient::allocate(turn_socket, turn.addr, &turn.username, &turn.password)?;
        let relayed = client.relayed_address();
        rtc.add_local_candidate(
            Candidate::relayed(relayed, addr, "udp").expect("relayed candidate"),
        )
        .expect("add relayed candidate");
        candidates.relayed = Some(relayed);
        plane.install_turn(client, config);
    }
    Ok((rtc, plane, candidates))
}

/// str0m 传输节点：tokio 侧句柄。Drop 发送 Shutdown 并等待线程退出。
pub struct Str0mTransport {
    cmd: mpsc::Sender<Cmd>,
    video_cmd: mpsc::Sender<Cmd>,
    answer_tx: Option<std::sync::mpsc::Sender<(SdpPendingOffer, SdpAnswer)>>,
    events: Option<Str0mEventReceiver>,
    event_tx: mpsc::Sender<Str0mEvent>,
    control_events: Mutex<mpsc::Receiver<Vec<u8>>>,
    video_events: Mutex<mpsc::Receiver<Vec<u8>>>,
    join: Option<JoinHandle<()>>,
    next_id: u64,
    reassembler: Reassembler,
    origin: Instant,
    stop: Arc<AtomicBool>,
    heartbeat_enabled: Arc<AtomicBool>,
}

impl Str0mTransport {
    /// 以 SDP offer 发起端创建可靠控制通道和无序零重传视频通道。
    /// 返回待传递给应答端的 offer 与待保留至 accept_answer 的 pending。
    pub fn new_offerer() -> Result<(Self, SdpOffer, SdpPendingOffer), anyhow::Error> {
        let (transport, offer, pending, _) = Self::new_offerer_ice(&IceConfig::default())?;
        Ok((transport, offer, pending))
    }

    /// 同 `new_offerer`，但按 `IceConfig` 收集候选并返回待经信令通告的
    /// 本端候选列表。
    pub fn new_offerer_ice(
        config: &IceConfig,
    ) -> Result<(Self, SdpOffer, SdpPendingOffer, LocalCandidates), anyhow::Error> {
        let (mut rtc, plane, local) = build_rtc(config)?;
        let mut api = rtc.sdp_api();
        api.add_channel_with_config(ChannelConfig {
            label: CONTROL_LABEL.to_string(),
            ordered: true,
            reliability: Reliability::Reliable,
            ..Default::default()
        });
        api.add_channel_with_config(ChannelConfig {
            label: VIDEO_LABEL.to_string(),
            ordered: false,
            reliability: Reliability::MaxRetransmits { retransmits: 0 },
            ..Default::default()
        });
        let (offer, pending) = api
            .apply()
            .ok_or_else(|| anyhow::anyhow!("sdp apply produced no offer"))?;
        Ok((Self::spawn(rtc, plane), offer, pending, local))
    }

    /// 以 SDP answer 应答端身份创建。
    pub fn new_answerer(offer: SdpOffer) -> Result<(Self, SdpAnswer), anyhow::Error> {
        let (transport, answer, _) = Self::new_answerer_ice(offer, &IceConfig::default())?;
        Ok((transport, answer))
    }

    /// 同 `new_answerer`，但按 `IceConfig` 收集候选并返回本端候选列表。
    pub fn new_answerer_ice(
        offer: SdpOffer,
        config: &IceConfig,
    ) -> Result<(Self, SdpAnswer, LocalCandidates), anyhow::Error> {
        let (mut rtc, plane, local) = build_rtc(config)?;
        let answer = rtc.sdp_api().accept_offer(offer)?;
        Ok((Self::spawn(rtc, plane), answer, local))
    }

    /// offer 端收到 answer 后完成握手。try_send 不阻塞运行时线程；
    /// 队列满时返回错误，由上层重试或视为信令失败。
    pub fn accept_answer(
        &mut self,
        pending: SdpPendingOffer,
        answer: SdpAnswer,
    ) -> Result<(), TransportError> {
        self.answer_tx
            .take()
            .ok_or(TransportError::Closed)?
            .send((pending, answer))
            .map_err(|_| TransportError::Closed)
    }

    fn spawn(rtc: Rtc, plane: Plane) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(EVENT_BUFFER);
        let (video_tx, video_rx) = mpsc::channel(EVENT_BUFFER);
        let (ev_tx, ev_rx) = mpsc::channel(EVENT_BUFFER);
        let (control_tx, control_rx) = mpsc::channel(EVENT_BUFFER);
        let (video_event_tx, video_event_rx) = mpsc::channel(EVENT_BUFFER);
        let (answer_tx, answer_rx) = std::sync::mpsc::channel();
        let event_tx = ev_tx.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let worker_done = Arc::new(AtomicBool::new(false));
        let thread_done = worker_done.clone();
        let heartbeat_enabled = Arc::new(AtomicBool::new(false));
        let thread_heartbeat = heartbeat_enabled.clone();
        let join = std::thread::Builder::new()
            .name("rdx-str0m".into())
            .spawn(move || {
                let _signal_exit = SignalWorkerExit(thread_done);
                run_loop(
                    rtc,
                    plane,
                    cmd_rx,
                    video_rx,
                    answer_rx,
                    EventSenders {
                        lifecycle: ev_tx,
                        control: control_tx,
                        video: video_event_tx,
                    },
                    WorkerControl {
                        stop: thread_stop,
                        heartbeat_enabled: thread_heartbeat,
                    },
                )
            })
            .expect("spawn str0m thread");
        Self {
            cmd: cmd_tx,
            video_cmd: video_tx,
            answer_tx: Some(answer_tx),
            events: Some(Str0mEventReceiver {
                events: ev_rx,
                worker_done,
            }),
            event_tx,
            control_events: Mutex::new(control_rx),
            video_events: Mutex::new(video_event_rx),
            join: Some(join),
            next_id: 0,
            reassembler: Reassembler::default(),
            origin: Instant::now(),
            stop,
            heartbeat_enabled,
        }
    }

    /// Move the event stream out for concurrent application-level processing.
    /// After this call, `next_event` on the transport returns `None`.
    pub fn take_events(&mut self) -> Option<Str0mEventReceiver> {
        self.events.take()
    }

    /// Add one remotely advertised candidate (signaling form). Unknown or
    /// malformed candidates are ignored; duplicates are rejected by the agent.
    pub async fn add_remote_candidate(
        &mut self,
        candidate: SignalCandidate,
    ) -> Result<(), TransportError> {
        self.cmd
            .send(Cmd::AddRemote(candidate))
            .await
            .map_err(|_| TransportError::Closed)
    }

    /// Enable only after the peer has agreed to heartbeat support. No video or
    /// application control polling is needed for the worker to answer probes.
    pub fn enable_heartbeat(&self) {
        self.heartbeat_enabled.store(true, Ordering::Release);
    }

    /// Lifecycle and recovery notifications; media and control use separate queues.
    pub async fn next_event(&mut self) -> Option<Str0mEvent> {
        self.events.as_mut()?.recv().await
    }

    /// Receive one reliable, ordered control message.
    pub async fn recv_control(&mut self) -> Result<Vec<u8>, TransportError> {
        self.control_events
            .lock()
            .await
            .recv()
            .await
            .ok_or(TransportError::Closed)
    }

    /// Queue a small control message. The channel is ordered and reliable.
    pub async fn send_control(&mut self, data: &[u8]) -> Result<(), TransportError> {
        if data.len() > MAX_CONTROL_MESSAGE {
            return Err(
                anyhow::anyhow!("control message exceeds {MAX_CONTROL_MESSAGE} bytes").into(),
            );
        }
        self.cmd
            .send(Cmd::Send(Bytes::copy_from_slice(data)))
            .await
            .map_err(|_| TransportError::Closed)
    }
}

impl Drop for Str0mTransport {
    fn drop(&mut self) {
        // Independent cancellation works even with both command queues full.
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[allow(async_fn_in_trait)]
impl Transport for Str0mTransport {
    async fn send(&mut self, frame: Frame) -> Result<(), TransportError> {
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or(TransportError::Closed)?;
        let fragments = fragment(&frame, id).map_err(anyhow::Error::from)?;
        for packet in fragments {
            self.video_cmd
                .send(Cmd::Send(packet))
                .await
                .map_err(|_| TransportError::Closed)?;
        }
        Ok(())
    }

    async fn recv(&mut self) -> Result<Frame, TransportError> {
        let result = self.recv_frame().await;
        if result.is_err() {
            // A failed receive requires a fresh connection. Release partial
            // frames even when the caller retains this closed handle.
            self.reassembler = Reassembler::default();
        }
        result
    }
}

impl Str0mTransport {
    async fn recv_frame(&mut self) -> Result<Frame, TransportError> {
        loop {
            let now = self.origin.elapsed().as_millis() as u64;
            let expired = self.reassembler.expire_requests(now);
            if !expired.is_empty() {
                let mut latest = HashMap::new();
                for (stream_id, frame_id) in expired {
                    latest
                        .entry(stream_id)
                        .and_modify(|id: &mut u64| *id = (*id).max(frame_id))
                        .or_insert(frame_id);
                }
                for (stream_id, after_frame_id) in latest {
                    let _ = self.event_tx.try_send(Str0mEvent::KeyframeNeeded {
                        stream_id,
                        after_frame_id,
                    });
                    let request =
                        ControlMessage::keyframe_request(stream_id, after_frame_id).encode();
                    self.send_control(&request).await?;
                }
                // A lost media frame is recoverable; keep waiting for a fresh
                // keyframe instead of making callers mistake packet loss for
                // a disconnected transport.
                continue;
            }
            let mut video_events = self.video_events.lock().await;
            let packet =
                match tokio::time::timeout(Duration::from_millis(50), video_events.recv()).await {
                    Ok(packet) => packet.ok_or(TransportError::Closed)?,
                    Err(_) => continue,
                };
            drop(video_events);
            let now = self.origin.elapsed().as_millis() as u64;
            match self.reassembler.push(&packet, now) {
                Ok(Some(frame)) => return Ok(frame),
                Ok(None) | Err(FragmentError::Stale) => continue,
                Err(error) => return Err(anyhow::Error::from(error).into()),
            }
        }
    }
}

/// 创建一对已完成 SDP 握手、经真实 ICE/DTLS（127.0.0.1 UDP）互通的传输端。
pub fn connect_pair() -> Result<(Str0mTransport, Str0mTransport), anyhow::Error> {
    let (mut a, offer, pending) = Str0mTransport::new_offerer()?;
    let (b, answer) = Str0mTransport::new_answerer(offer)?;
    a.accept_answer(pending, answer)?;
    Ok((a, b))
}

fn run_loop(
    mut rtc: Rtc,
    mut plane: Plane,
    mut cmd_rx: mpsc::Receiver<Cmd>,
    mut video_rx: mpsc::Receiver<Cmd>,
    answer_rx: std::sync::mpsc::Receiver<(SdpPendingOffer, SdpAnswer)>,
    senders: EventSenders,
    control: WorkerControl,
) {
    let WorkerControl {
        stop,
        heartbeat_enabled,
    } = control;
    let mut buf = vec![0u8; 2000];
    let mut control_outbox: VecDeque<Bytes> = VecDeque::new();
    let mut video_outbox: VecDeque<Bytes> = VecDeque::new();
    let mut channels: HashMap<ChannelKind, ChannelId> = HashMap::new();
    let mut channel_kind: HashMap<ChannelId, ChannelKind> = HashMap::new();
    // pending SDP answer 由命令线程带入（offer 端在握手完成前即可收发命令）。
    let mut pending_answer: Option<(SdpPendingOffer, SdpAnswer)> = None;
    let mut heartbeat: Option<HeartbeatState> = None;

    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        // 1. 命令先行：握手包也是命令；命令端全部断开即结束线程。
        while control_outbox.len() < EVENT_BUFFER {
            match cmd_rx.try_recv() {
                Ok(Cmd::Send(payload)) => control_outbox.push_back(payload),
                Ok(Cmd::AddRemote(info)) => {
                    if let Some(candidate) = decode_signal_candidate(&info) {
                        rtc.add_remote_candidate(candidate);
                    }
                    plane.note_remote();
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        while video_outbox.len() < EVENT_BUFFER {
            match video_rx.try_recv() {
                Ok(Cmd::Send(payload)) => video_outbox.push_back(payload),
                // Remote candidates arrive on the control queue only.
                Ok(Cmd::AddRemote(_)) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if let Ok((pending, answer)) = answer_rx.try_recv() {
            pending_answer = Some((pending, answer));
        }
        if let Some((pending, answer)) = pending_answer.take() {
            if rtc.sdp_api().accept_answer(pending, answer).is_err() {
                return; // 握手失败：结束线程，事件通道关闭对上层表现为 Closed。
            }
        }
        if !rtc.is_alive() {
            return;
        }

        // 2. 驱动 str0m 输出直到下一次超时。
        let timeout;
        loop {
            match rtc.poll_output() {
                Ok(Output::Timeout(next)) => {
                    timeout = next;
                    break;
                }
                Ok(Output::Transmit(tx)) => {
                    plane.send(tx.destination, &tx.contents);
                }
                Ok(Output::Event(event)) => match event {
                    Event::ChannelOpen(id, label) => {
                        let kind = match label.as_str() {
                            CONTROL_LABEL => ChannelKind::Control,
                            VIDEO_LABEL => ChannelKind::Video,
                            _ => return,
                        };
                        channels.insert(kind, id);
                        channel_kind.insert(id, kind);
                        if senders
                            .lifecycle
                            .try_send(Str0mEvent::ChannelOpen(id, label))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Event::ChannelData(data) if !data.binary => return,
                    Event::ChannelData(data) => match channel_kind.get(&data.id) {
                        Some(ChannelKind::Control) if data.data.len() <= MAX_CONTROL_MESSAGE => {
                            match ControlMessage::decode_wire(&data.data) {
                                Ok(ControlMessage {
                                    kind: Some(control_message::Kind::Heartbeat(probe)),
                                }) => {
                                    if probe.response {
                                        if let Some(state) = heartbeat.as_mut() {
                                            state.acknowledge(probe.nonce, Instant::now());
                                        }
                                    } else {
                                        if control_outbox.len() >= EVENT_BUFFER {
                                            return;
                                        }
                                        control_outbox.push_front(heartbeat_bytes(
                                            rd_protocol::control::Heartbeat {
                                                nonce: probe.nonce,
                                                response: true,
                                            },
                                        ));
                                    }
                                }
                                Ok(ControlMessage {
                                    kind: Some(control_message::Kind::KeyframeRequest(request)),
                                }) => {
                                    let Ok(stream_id) = u8::try_from(request.stream_id) else {
                                        return;
                                    };
                                    if senders
                                        .lifecycle
                                        .try_send(Str0mEvent::KeyframeRequested {
                                            stream_id,
                                            after_frame_id: request.after_frame_id,
                                        })
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                                _ => {
                                    if senders.control.try_send(data.data).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        Some(ChannelKind::Video) if data.data.len() <= CHUNK + ENVELOPE => {
                            if senders.video.try_send(data.data).is_err() {
                                return;
                            }
                        }
                        _ => return,
                    },
                    Event::IceConnectionStateChange(str0m::IceConnectionState::Connected) => {
                        plane.mark_connected();
                        match senders.lifecycle.try_send(Str0mEvent::IceConnected) {
                            Ok(()) => {}
                            Err(_) => return,
                        }
                    }
                    _ => {}
                },
                Err(_) => return,
            }
        }

        if heartbeat_enabled.load(Ordering::Acquire) && channels.contains_key(&ChannelKind::Control)
        {
            let now = Instant::now();
            match heartbeat
                .get_or_insert_with(|| HeartbeatState::new(now))
                .poll(now)
            {
                Err(()) => return,
                Ok(Some(probe)) => {
                    if control_outbox.len() >= EVENT_BUFFER {
                        return;
                    }
                    control_outbox.push_front(heartbeat_bytes(probe));
                }
                Ok(None) => {}
            }
        }

        // 3. 通道打开后按 SCTP 可用窗口冲刷外发队列；未被接受则下轮重试。
        let mut sent = 0;
        while sent < WRITE_BATCH {
            let (Some(id), Some(front)) = (
                channels.get(&ChannelKind::Control).copied(),
                control_outbox.front(),
            ) else {
                break;
            };
            let accepted = rtc
                .channel(id)
                .and_then(|mut ch| ch.write(true, front).ok())
                .unwrap_or(false);
            if !accepted {
                break;
            }
            control_outbox.pop_front();
            sent += 1;
        }
        let mut sent = 0;
        while sent < WRITE_BATCH {
            let (Some(id), Some(front)) = (
                channels.get(&ChannelKind::Video).copied(),
                video_outbox.front(),
            ) else {
                break;
            };
            let accepted = rtc
                .channel(id)
                .and_then(|mut ch| ch.write(true, front).ok())
                .unwrap_or(false);
            if !accepted {
                break;
            }
            video_outbox.pop_front();
            sent += 1;
        }

        if plane.take_switch_notice()
            && senders
                .lifecycle
                .try_send(Str0mEvent::RelayFallback)
                .is_err()
        {
            return;
        }

        // 4. 在最近的截止时刻内等待 UDP 输入。
        let wait = timeout
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1))
            .min(Duration::from_millis(100));
        let direct_local = plane.direct_local;
        let feed = |rtc: &mut Rtc, source: SocketAddr, local: SocketAddr, data: &[u8]| -> bool {
            if let Ok(input) = Receive::new(Protocol::Udp, source, local, data) {
                rtc.handle_input(Input::Receive(Instant::now(), input))
                    .is_ok()
            } else {
                true
            }
        };
        if plane.turn.is_none() {
            plane
                .direct
                .set_read_timeout(Some(wait))
                .expect("set read timeout");
            if let Some((source, n)) = plane.read_direct(&mut buf) {
                if !feed(&mut rtc, source, direct_local, &buf[..n]) {
                    return;
                }
            }
        } else {
            let Some(relay_dest) = plane.relay_destination() else {
                return;
            };
            // Relayed payloads queued while busy transacting come first.
            if let Some(client) = plane.turn.as_mut() {
                while let Some((peer, data)) = client.poll_data() {
                    if !feed(&mut rtc, peer, relay_dest, &data) {
                        return;
                    }
                }
                if client.is_due(Instant::now()) && client.refresh_if_due(Instant::now()).is_err() {
                    return; // relay lost: nothing else can carry the session
                }
            }
            let (direct_wait, turn_wait) = if plane.relay_only || plane.using_relay {
                (Duration::ZERO, wait)
            } else {
                let half = wait / 2;
                (
                    half.max(Duration::from_millis(1)),
                    half.max(Duration::from_millis(1)),
                )
            };
            if !plane.relay_only && direct_wait > Duration::ZERO {
                plane
                    .direct
                    .set_read_timeout(Some(direct_wait))
                    .expect("set read timeout");
                if let Some((source, n)) = plane.read_direct(&mut buf) {
                    if !feed(&mut rtc, source, direct_local, &buf[..n]) {
                        return;
                    }
                }
            }
            if turn_wait > Duration::ZERO {
                if let Some(client) = plane.turn.as_mut() {
                    client.poll_socket(turn_wait);
                }
                if let Some(client) = plane.turn.as_mut() {
                    while let Some((peer, data)) = client.poll_data() {
                        if !feed(&mut rtc, peer, relay_dest, &data) {
                            return;
                        }
                    }
                }
            }
        }
        rtc.handle_input(Input::Timeout(Instant::now()))
            .expect("handle timeout");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turn::mock::MockTurnServer;

    async fn video_ready(transport: &mut Str0mTransport) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match transport.next_event().await {
                    Some(Str0mEvent::ChannelOpen(_, label)) if label == VIDEO_LABEL => break,
                    Some(_) => {}
                    None => panic!("transport closed before video opened"),
                }
            }
        })
        .await
        .expect("video channel opens");
    }

    #[tokio::test]
    async fn saturated_video_receiver_is_bounded_and_worker_exits() {
        let (mut a, mut b) = connect_pair().unwrap();
        tokio::join!(video_ready(&mut a), video_ready(&mut b));
        let mut events = b.take_events().unwrap();
        let original = Frame::new(
            1,
            0,
            true,
            Bytes::from(vec![7; CHUNK - rd_protocol::HEADER_LEN]),
        );
        tokio::time::timeout(Duration::from_secs(15), async {
            // No video consumer: fill the actual worker receive queue through
            // ICE/DTLS/SCTP. Pacing reduces unrelated UDP burst loss.
            for id in 0..4096 {
                if b.join.as_ref().unwrap().is_finished() {
                    break;
                }
                let packet = fragment(&original, id).unwrap().remove(0);
                a.video_cmd.send(Cmd::Send(packet)).await.unwrap();
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            while !b.join.as_ref().unwrap().is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("saturated receiver worker must stop");
        b.join.take().unwrap().join().unwrap();
        {
            let queue = b.video_events.lock().await;
            assert_eq!(queue.len(), EVENT_BUFFER);
            assert_eq!(queue.max_capacity(), EVENT_BUFFER);
        }
        assert!(matches!(
            b.send_control(b"closed").await,
            Err(TransportError::Closed)
        ));
        // The retained transport handle must not keep lifecycle observation
        // alive after saturation has terminated the worker.
        tokio::time::timeout(Duration::from_secs(1), async {
            while events.recv().await.is_some() {}
        })
        .await
        .unwrap();
        for _ in 0..EVENT_BUFFER {
            let frame = b.recv().await.unwrap();
            assert_eq!(frame.header, original.header);
            assert_eq!(frame.payload, original.payload);
        }
        assert!(matches!(b.recv().await, Err(TransportError::Closed)));
        assert_eq!(b.reassembler.buffered_bytes(), 0);
    }

    #[tokio::test]
    async fn late_duplicate_does_not_disconnect_video() {
        let (_a, mut b) = connect_pair().unwrap();
        let original = Frame::new(1, 0, true, Bytes::from_static(b"first"));
        let first = b
            .reassembler
            .push(&fragment(&original, 0).unwrap()[0], 0)
            .unwrap()
            .unwrap();
        assert_eq!(first.payload, original.payload);
        // Feed the worker-to-receiver boundary in a deterministic order so
        // unordered network delivery cannot hide the stale-fragment path.
        let (packet_tx, packet_rx) = mpsc::channel(4);
        b.video_events = Mutex::new(packet_rx);
        for packet in fragment(&original, 0).unwrap() {
            packet_tx.send(packet.to_vec()).await.unwrap();
        }
        let next = Frame::new(1, 1, false, Bytes::from_static(b"next"));
        for packet in fragment(&next, 1).unwrap() {
            packet_tx.send(packet.to_vec()).await.unwrap();
        }
        let received = tokio::time::timeout(Duration::from_secs(5), b.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received.header, next.header);
        assert_eq!(received.payload, next.payload);
    }

    #[tokio::test]
    async fn closed_receive_releases_partial_frames_while_handle_is_retained() {
        let (_a, mut b) = connect_pair().unwrap();
        let original = Frame::new(1, 0, false, Bytes::from(vec![1; CHUNK * 2]));
        b.reassembler
            .push(&fragment(&original, 0).unwrap()[0], 0)
            .unwrap();
        assert!(b.reassembler.buffered_bytes() > 0);
        b.stop.store(true, Ordering::Release);
        b.join.take().unwrap().join().unwrap();
        assert!(matches!(b.recv().await, Err(TransportError::Closed)));
        assert_eq!(b.reassembler.buffered_bytes(), 0);
        assert!(matches!(b.recv().await, Err(TransportError::Closed)));
    }

    #[test]
    fn heartbeat_requires_matching_fresh_response_and_has_fixed_deadline() {
        let now = Instant::now();
        let mut state = HeartbeatState::new(now);
        let probe = state.poll(now).unwrap().unwrap();
        state.acknowledge(probe.nonce + 1, now + Duration::from_secs(1));
        assert!(state.poll(now + Duration::from_secs(7)).unwrap().is_none());
        assert!(state.poll(now + HEARTBEAT_TIMEOUT).is_err());
        // A late response cannot revive an expired probe.
        state.acknowledge(probe.nonce, now + HEARTBEAT_TIMEOUT);
        assert!(state.poll(now + HEARTBEAT_TIMEOUT).is_err());

        let mut state = HeartbeatState::new(now);
        let first = state.poll(now).unwrap().unwrap();
        state.acknowledge(first.nonce, now);
        assert!(state
            .poll(now + Duration::from_millis(999))
            .unwrap()
            .is_none());
        let next = state.poll(now + HEARTBEAT_INTERVAL).unwrap().unwrap();
        assert_ne!(first.nonce, next.nonce);
        state.acknowledge(first.nonce, now + HEARTBEAT_INTERVAL);
        assert!(state
            .poll(now + HEARTBEAT_INTERVAL + HEARTBEAT_TIMEOUT)
            .is_err());
    }

    #[tokio::test]
    async fn idle_peers_stay_live_without_application_control_polling() {
        let (mut a, offer, pending) = Str0mTransport::new_offerer().unwrap();
        let (mut rtc, plane, _local) = build_rtc(&IceConfig::default()).unwrap();
        // Retain the bound UDP endpoint after stopping its worker. This models
        // a silent peer without an immediate ICMP port-unreachable shortcut.
        let blackhole = plane.direct_socket().try_clone().unwrap();
        let answer = rtc.sdp_api().accept_offer(offer).unwrap();
        let b = Str0mTransport::spawn(rtc, plane);
        a.accept_answer(pending, answer).unwrap();
        a.enable_heartbeat();
        b.enable_heartbeat();
        tokio::time::sleep(Duration::from_secs(10)).await;
        let payload = b"still-alive";
        a.send_control(payload).await.unwrap();
        let mut b = b;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), b.recv_control())
                .await
                .unwrap()
                .unwrap(),
            payload
        );
        drop(b);
        let started = Instant::now();
        assert!(
            tokio::time::timeout(Duration::from_secs(10), a.recv_control())
                .await
                .unwrap()
                .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        drop(blackhole);
    }

    #[tokio::test]
    async fn fragmented_frame_preserves_metadata() {
        let (mut a, mut b) = connect_pair().unwrap();
        let original = Frame::new(
            9,
            u16::MAX,
            true,
            Bytes::from(
                (0..CHUNK * 4 + 17)
                    .map(|i| (i % 251) as u8)
                    .collect::<Vec<_>>(),
            ),
        );
        a.send(original.clone()).await.unwrap();
        let got = tokio::time::timeout(Duration::from_secs(10), b.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.header, original.header);
        assert_eq!(got.payload, original.payload);
    }

    #[tokio::test]
    async fn control_and_video_use_independent_channels() {
        let (mut a, mut b) = connect_pair().unwrap();
        let original = Frame::new(3, 18, true, Bytes::from(vec![4; CHUNK + 17]));
        a.send_control(b"request-keyframe").await.unwrap();
        a.send(original.clone()).await.unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), b.recv_control())
                .await
                .unwrap()
                .unwrap(),
            b"request-keyframe"
        );
        let got = tokio::time::timeout(Duration::from_secs(10), b.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.header, original.header);
        assert_eq!(got.payload, original.payload);
        assert!(a
            .send_control(&vec![0; MAX_CONTROL_MESSAGE + 1])
            .await
            .is_err());
    }

    #[tokio::test]
    async fn control_delivers_while_video_queue_is_busy() {
        let (mut a, mut b) = connect_pair().unwrap();
        let fragment = Bytes::from(vec![0; CHUNK + ENVELOPE]);
        for _ in 0..64 {
            a.video_cmd.send(Cmd::Send(fragment.clone())).await.unwrap();
        }
        a.send_control(b"priority").await.unwrap();
        let control = tokio::time::timeout(Duration::from_secs(5), b.recv_control())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(control, b"priority");
    }

    #[test]
    fn full_command_queue_does_not_block_drop() {
        let (mut transport, offer, pending) = Str0mTransport::new_offerer().unwrap();
        let (answerer, answer) = Str0mTransport::new_answerer(offer).unwrap();
        for _ in 0..EVENT_BUFFER * 4 {
            if transport
                .cmd
                .try_send(Cmd::Send(Bytes::from(vec![0; CHUNK])))
                .is_err()
            {
                break;
            }
        }
        transport.accept_answer(pending, answer).unwrap();
        drop(answerer);
        let join = transport.join.take().unwrap();
        drop(transport);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !join.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(join.is_finished());
        join.join().unwrap();
    }

    #[tokio::test]
    async fn missing_fragment_requests_keyframe_and_recv_recovers() {
        let (mut a, mut b) = connect_pair().unwrap();
        let original = Frame::new(9, 1, false, Bytes::from(vec![1; CHUNK * 2]));
        let packets = fragment(&original, 0).unwrap();
        a.video_cmd
            .send(Cmd::Send(packets[0].clone()))
            .await
            .unwrap();
        a.next_id = 1;
        let recovery = Frame::new(9, 2, true, Bytes::from(vec![7; 256]));
        let (received, sent) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(5), b.recv()),
            async {
                // Start recovery after observing actual expiry, not a delay
                // measured before ICE/DTLS has delivered the first fragment.
                tokio::time::timeout(Duration::from_secs(4), async {
                    loop {
                        match a.next_event().await {
                            Some(Str0mEvent::KeyframeRequested {
                                stream_id: 9,
                                after_frame_id: 0,
                            }) => break,
                            Some(_) => {}
                            None => panic!("sender stopped before recovery request"),
                        }
                    }
                })
                .await
                .unwrap();
                a.send(recovery.clone()).await
            }
        );
        sent.unwrap();
        let received = received.unwrap().unwrap();
        assert_eq!(received.header, recovery.header);
        assert_eq!(received.payload, recovery.payload);
        assert_eq!(b.reassembler.buffered_bytes(), 0);

        let mut local_notice = false;
        let local_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < local_deadline {
            let remaining = local_deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, b.next_event()).await {
                Ok(Some(Str0mEvent::KeyframeNeeded {
                    stream_id: 9,
                    after_frame_id: 0,
                })) => {
                    local_notice = true;
                    break;
                }
                Ok(Some(_)) => {}
                _ => break,
            }
        }
        assert!(
            local_notice,
            "receiver should report the expired access unit"
        );
    }

    fn frame(payload: &[u8]) -> Frame {
        Frame::new(1, 0, false, Bytes::copy_from_slice(payload))
    }

    #[tokio::test]
    async fn two_nodes_exchange_frames_over_real_ice_dtls() {
        let (mut a, mut b) = connect_pair().expect("connect pair");
        tokio::join!(video_ready(&mut a), video_ready(&mut b));
        let payload: &[u8] = &[0xde, 0xad, 0xbe, 0xef];
        a.send(frame(payload)).await.expect("send from a");
        let got = tokio::time::timeout(Duration::from_secs(10), b.recv())
            .await
            .expect("recv within 10s")
            .expect("frame from b");
        assert_eq!(&got.payload[..], payload);

        // 反向再验一次，并确认通道打开事件先于数据到达被观测到。
        b.send(frame(b"pong")).await.expect("send from b");
        let got = tokio::time::timeout(Duration::from_secs(10), a.recv())
            .await
            .expect("recv within 10s")
            .expect("frame from a");
        assert_eq!(&got.payload[..], b"pong");
    }

    fn relay_config(server: &MockTurnServer, relay_only: bool) -> IceConfig {
        IceConfig {
            turn: Some(TurnConfig {
                addr: server.addr(),
                username: server.username.clone(),
                password: server.password.clone(),
            }),
            relay_only,
            ..IceConfig::default()
        }
    }

    async fn exchange_candidates(
        dialer: &mut Str0mTransport,
        dialer_local: &LocalCandidates,
        answerer: &mut Str0mTransport,
        answerer_local: &LocalCandidates,
    ) {
        for candidate in answerer_local.to_signaling() {
            dialer.add_remote_candidate(candidate).await.unwrap();
        }
        for candidate in dialer_local.to_signaling() {
            answerer.add_remote_candidate(candidate).await.unwrap();
        }
    }

    #[tokio::test]
    async fn two_nodes_exchange_frames_over_forced_turn_relay() {
        let server = MockTurnServer::start();
        let (mut a, offer, pending, a_local) =
            Str0mTransport::new_offerer_ice(&relay_config(&server, true)).unwrap();
        assert_eq!(a_local.host.len(), 0);
        assert!(a_local.srflx.is_none());
        let relayed = a_local.relayed.expect("relay candidate gathered");
        assert_eq!(relayed.ip().to_string(), "127.0.0.1");
        let (mut b, answer, b_local) =
            Str0mTransport::new_answerer_ice(offer, &relay_config(&server, true)).unwrap();
        assert!(b_local.relayed.is_some());
        a.accept_answer(pending, answer).unwrap();
        exchange_candidates(&mut a, &a_local, &mut b, &b_local).await;

        tokio::join!(video_ready(&mut a), video_ready(&mut b));
        let payload: &[u8] = &[0xde, 0xad, 0xbe, 0xef];
        a.send(frame(payload)).await.expect("send from a");
        let got = tokio::time::timeout(Duration::from_secs(10), b.recv())
            .await
            .expect("recv within 10s")
            .expect("frame from b");
        assert_eq!(&got.payload[..], payload);
        b.send(frame(b"pong-relayed")).await.expect("send from b");
        let got = tokio::time::timeout(Duration::from_secs(10), a.recv())
            .await
            .expect("recv within 10s")
            .expect("frame from a");
        assert_eq!(&got.payload[..], b"pong-relayed");
    }

    #[tokio::test]
    async fn relay_fallback_engages_when_direct_never_connects() {
        let server = MockTurnServer::start();
        // The dialer starts direct-first with a short fallback deadline; the
        // answerer is relay-only, so the dialer's direct checks to the
        // answerer's relayed address go nowhere and force the fallback.
        let mut dialer_config = relay_config(&server, false);
        dialer_config.relay_fallback = Some(Duration::from_millis(300));
        let (mut a, offer, pending, a_local) =
            Str0mTransport::new_offerer_ice(&dialer_config).unwrap();
        assert_eq!(a_local.host.len(), 1);
        assert!(a_local.relayed.is_some());
        let (mut b, answer, b_local) =
            Str0mTransport::new_answerer_ice(offer, &relay_config(&server, true)).unwrap();
        a.accept_answer(pending, answer).unwrap();
        exchange_candidates(&mut a, &a_local, &mut b, &b_local).await;

        let mut events = a.take_events().unwrap();
        let (fallback_tx, fallback_rx) = tokio::sync::oneshot::channel::<bool>();
        // Keep draining lifecycle events after the notice: dropping the
        // receiver would tear the worker down on its next event.
        tokio::spawn(async move {
            let mut noticed = false;
            let mut fallback_tx = Some(fallback_tx);
            loop {
                match events.recv().await {
                    Some(Str0mEvent::RelayFallback) if !noticed => {
                        noticed = true;
                        if let Some(tx) = fallback_tx.take() {
                            let _ = tx.send(true);
                        }
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        });
        tokio::join!(video_ready(&mut b), async {
            tokio::time::timeout(Duration::from_secs(15), fallback_rx)
                .await
                .expect("fallback notice within 15s")
                .expect("transport alive");
        });
        let payload: &[u8] = &[1, 2, 3, 4];
        a.send(frame(payload)).await.unwrap();
        let got = tokio::time::timeout(Duration::from_secs(10), b.recv())
            .await
            .expect("recv within 10s")
            .expect("frame after fallback");
        assert_eq!(&got.payload[..], payload);
    }

    #[tokio::test]
    async fn dropping_both_ends_shuts_threads_down() {
        let (mut a, mut b) = connect_pair().expect("connect pair");
        let a_join = a.join.take();
        let b_join = b.join.take();
        drop(a);
        drop(b);
        // 手动接管 join：线程应在 Shutdown/通道断开后于一个轮询周期内退出。
        let (Some(ja), Some(jb)) = (a_join, b_join) else {
            panic!("join handles missing");
        };
        ja.join().expect("a thread clean");
        jb.join().expect("b thread clean");
    }
}
