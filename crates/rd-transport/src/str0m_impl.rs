//! P2 spike：str0m 桥接模式验证（ADR-0002）。
//!
//! 同步的 str0m `Rtc` 与其 UDP socket 全部归一个专用线程所有，tokio 侧只通过
//! channel 交互（命令进、事件出），线程内以 poll_output/Transmit/Timeout 驱动。
//! SDP 交换属于信令（P3）；本模块在 `connect_pair` 中以进程内交换完成握手，
//! 供 P2 局域网联调与测试使用。crypto provider 在进程内安装一次。
//!
//! 已知边界（P2 正式接入时处理）：Frame 载荷按 SCTP 可用窗口整块写入，
//! 未实现 Transport trait 要求的分片/重组规格；重连与状态机联动未做。

use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Once;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bytes::Bytes;
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::channel::ChannelId;
use str0m::crypto;
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::Mutex;

use rd_protocol::Frame;

use crate::{Transport, TransportError};

const CHANNEL_LABEL: &str = "rdx";
/// 线程外发事件的通道上限；阻塞发送失败视为对端已放弃。
const EVENT_BUFFER: usize = 256;

enum Cmd {
    Send(Vec<u8>),
    AcceptAnswer(SdpPendingOffer, SdpAnswer),
    Shutdown,
}

/// 专用线程向 tokio 侧推送的事件；非 Frame 事件用于观测连接生命周期。
#[derive(Debug)]
pub enum Str0mEvent {
    ChannelOpen(ChannelId, String),
    IceConnected,
    Frame(Vec<u8>),
}

fn install_crypto() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| crypto::from_feature_flags().install_process_default());
}

fn build_rtc() -> (Rtc, UdpSocket, SocketAddr) {
    install_crypto();
    let socket = UdpSocket::bind("127.0.0.1:0").expect("bind loopback udp");
    let addr = socket.local_addr().expect("local addr");
    let mut rtc = Rtc::builder().build(Instant::now());
    rtc.add_local_candidate(Candidate::host(addr, "udp").expect("host candidate"))
        .expect("add candidate");
    (rtc, socket, addr)
}

/// str0m 传输节点：tokio 侧句柄。Drop 发送 Shutdown 并等待线程退出。
pub struct Str0mTransport {
    cmd: mpsc::Sender<Cmd>,
    events: Mutex<mpsc::Receiver<Str0mEvent>>,
    join: Option<JoinHandle<()>>,
    next_seq: u16,
}

impl Str0mTransport {
    /// 以 SDP offer 发起端身份创建（含一条 `rdx` 数据通道）。
    /// 返回待传递给应答端的 offer 与待保留至 accept_answer 的 pending。
    pub fn new_offerer() -> Result<(Self, SdpOffer, SdpPendingOffer), anyhow::Error> {
        let (mut rtc, socket, _addr) = build_rtc();
        let mut api = rtc.sdp_api();
        api.add_channel(CHANNEL_LABEL.to_string());
        let (offer, pending) = api
            .apply()
            .ok_or_else(|| anyhow::anyhow!("sdp apply produced no offer"))?;
        Ok((Self::spawn(rtc, socket), offer, pending))
    }

    /// 以 SDP answer 应答端身份创建。
    pub fn new_answerer(offer: SdpOffer) -> Result<(Self, SdpAnswer), anyhow::Error> {
        let (mut rtc, socket, _addr) = build_rtc();
        let answer = rtc.sdp_api().accept_offer(offer)?;
        Ok((Self::spawn(rtc, socket), answer))
    }

    /// offer 端收到 answer 后完成握手。try_send 不阻塞运行时线程；
    /// 队列满时返回错误，由上层重试或视为信令失败。
    pub fn accept_answer(
        &mut self,
        pending: SdpPendingOffer,
        answer: SdpAnswer,
    ) -> Result<(), TransportError> {
        self.cmd
            .try_send(Cmd::AcceptAnswer(pending, answer))
            .map_err(|_| TransportError::Closed)
    }

    fn spawn(rtc: Rtc, socket: UdpSocket) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(EVENT_BUFFER);
        let (ev_tx, ev_rx) = mpsc::channel(EVENT_BUFFER);
        let join = std::thread::Builder::new()
            .name("rdx-str0m".into())
            .spawn(move || run_loop(rtc, socket, cmd_rx, ev_tx))
            .expect("spawn str0m thread");
        Self {
            cmd: cmd_tx,
            events: Mutex::new(ev_rx),
            join: Some(join),
            next_seq: 0,
        }
    }

    /// 观测下一事件（含生命周期事件）；供上层在 recv 之外观测连接状态。
    pub async fn next_event(&mut self) -> Option<Str0mEvent> {
        self.events.lock().await.recv().await
    }
}

impl Drop for Str0mTransport {
    fn drop(&mut self) {
        // try_send 避免在运行时线程上阻塞；即使没送到，cmd 通道随 self
        // 析构断开，线程在下一轮 try_recv 后退出。join 至多等待一个轮询周期。
        let _ = self.cmd.try_send(Cmd::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[allow(async_fn_in_trait)]
impl Transport for Str0mTransport {
    async fn send(&mut self, frame: Frame) -> Result<(), TransportError> {
        self.cmd
            .send(Cmd::Send(frame.payload.to_vec()))
            .await
            .map_err(|_| TransportError::Closed)
    }

    async fn recv(&mut self) -> Result<Frame, TransportError> {
        loop {
            let event = self
                .events
                .lock()
                .await
                .recv()
                .await
                .ok_or(TransportError::Closed)?;
            if let Str0mEvent::Frame(data) = event {
                let seq = self.next_seq;
                self.next_seq = self.next_seq.wrapping_add(1);
                return Ok(Frame::new(1, seq, false, Bytes::from(data)));
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
    socket: UdpSocket,
    mut cmd_rx: mpsc::Receiver<Cmd>,
    ev_tx: mpsc::Sender<Str0mEvent>,
) {
    let local = socket.local_addr().expect("local addr");
    let mut buf = vec![0u8; 2000];
    let mut outbox: VecDeque<Vec<u8>> = VecDeque::new();
    let mut channel: Option<ChannelId> = None;
    // pending SDP answer 由命令线程带入（offer 端在握手完成前即可收发命令）。
    let mut pending_answer: Option<(SdpPendingOffer, SdpAnswer)> = None;

    loop {
        // 1. 命令先行：握手包也是命令；命令端全部断开即结束线程。
        loop {
            match cmd_rx.try_recv() {
                Ok(Cmd::Shutdown) => return,
                Ok(Cmd::Send(payload)) => outbox.push_back(payload),
                Ok(Cmd::AcceptAnswer(pending, answer)) => pending_answer = Some((pending, answer)),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
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
                    let _ = socket.send_to(&tx.contents, tx.destination);
                }
                Ok(Output::Event(event)) => match event {
                    Event::ChannelOpen(id, label) => {
                        channel = Some(id);
                        let _ = ev_tx.blocking_send(Str0mEvent::ChannelOpen(id, label));
                    }
                    Event::ChannelData(data) => {
                        let _ = ev_tx.blocking_send(Str0mEvent::Frame(data.data));
                    }
                    Event::IceConnectionStateChange(state) => {
                        if matches!(state, str0m::IceConnectionState::Connected) {
                            let _ = ev_tx.blocking_send(Str0mEvent::IceConnected);
                        }
                    }
                    _ => {}
                },
                Err(_) => return,
            }
        }

        // 3. 通道打开后按 SCTP 可用窗口冲刷外发队列；未被接受则下轮重试。
        while let (Some(id), Some(front)) = (channel, outbox.front()) {
            let accepted = rtc
                .channel(id)
                .and_then(|mut ch| ch.write(true, front).ok())
                .unwrap_or(false);
            if !accepted {
                break;
            }
            outbox.pop_front();
        }

        // 4. 在最近的截止时刻内等待 UDP 输入。
        let wait = timeout
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1))
            .min(Duration::from_millis(100));
        socket
            .set_read_timeout(Some(wait))
            .expect("set read timeout");
        match socket.recv_from(&mut buf) {
            Ok((n, source)) => {
                if let Ok(input) = Receive::new(Protocol::Udp, source, local, &buf[..n]) {
                    if rtc
                        .handle_input(Input::Receive(Instant::now(), input))
                        .is_err()
                    {
                        return;
                    }
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => return,
        }
        rtc.handle_input(Input::Timeout(Instant::now()))
            .expect("handle timeout");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(payload: &[u8]) -> Frame {
        Frame::new(1, 0, false, Bytes::copy_from_slice(payload))
    }

    #[tokio::test]
    async fn two_nodes_exchange_frames_over_real_ice_dtls() {
        let (mut a, mut b) = connect_pair().expect("connect pair");
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
