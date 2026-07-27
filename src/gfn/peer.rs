//! Real WebRTC peer for GeForce NOW streaming, built on the sans-I/O `rtc` crate.
//!
//! Owns a dedicated OS thread running its own single-threaded tokio runtime: the sans-I/O
//! `RTCPeerConnection` is driven there (UDP socket + timers + poll loop), decrypted video RTP
//! is depacketized into H.264 access units, and those are fed to the hardware decode worker
//! (`streaming::video::VideoDecodeWorker`), which writes decoded RGB565 frames straight into
//! the SDL textures registered by the shell (the direct-texture path).
//!
//! The app talks to this through the same non-blocking channel shape as `signaling`: commands
//! in (`add_remote_ice`), events out (`try_recv` once per tick).

use crate::gfn::cloudmatch::SessionInfo;
use crate::gfn::input_protocol::{
    GAMEPAD_BITMAP_PRIMARY, GamepadInput, InputEncoder, parse_input_handshake_version,
};
use crate::gfn::signaling::IceCandidate;
use crate::streaming::video::{
    DecodedFrame, DecoderConfig, DirectVideoOutput, VideoDecodeWorker,
};
use anyhow::{Context, Result};
use bytes::{Bytes, BytesMut};
use rtc::peer_connection::RTCPeerConnectionBuilder;
use rtc::peer_connection::configuration::RTCConfigurationBuilder;
use rtc::peer_connection::configuration::media_engine::MediaEngine;
use rtc::peer_connection::configuration::setting_engine::SettingEngine;
use rtc::interceptor::Registry;
use rtc::peer_connection::configuration::interceptor_registry::{
    configure_nack, configure_rtcp_reports,
};
use rtc::peer_connection::event::{RTCDataChannelEvent, RTCPeerConnectionEvent, RTCTrackEvent};
use rtc::peer_connection::message::RTCMessage;
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::peer_connection::state::RTCPeerConnectionState;
use rtc::peer_connection::transport::{
    CandidateConfig, CandidateHostConfig, RTCDtlsRole, RTCIceCandidate, RTCIceCandidateInit,
    RTCIceServer,
};
use rtc::rtp_transceiver::RTCRtpReceiverId;
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use rtc::sansio::Protocol;
use rtc::shared::{TaggedBytesMut, TransportContext, TransportProtocol};
use rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

// GFN clamps to its own supported modes regardless of what we request (960x544 is not one of
// them), so size the decoder from what CloudMatch reports and fall back to GFN's minimum.
const DEFAULT_STREAM_WIDTH: u32 = 1280;
const DEFAULT_STREAM_HEIGHT: u32 = 720;

// The Vita's actual panel resolution. Real hardware's AVCDEC can decode at the stream's real
// (larger) coded size while writing scaled-down output directly to this resolution (see
// `SceAvcdecFrame::frameWidth/frameHeight` in `streaming::video::decoder`), so there's no
// reason to allocate/upload textures any bigger than what the screen can ever show:
// at 1280x720 the decoder-to-texture traffic is roughly double what 960x544 needs per frame.
// NOTE: this path is for real hardware only - Vita3K's HLE AVCDEC doesn't reproduce this
// scaling (it crops a 960x544 window out of the decoded picture instead), which is why this
// was reverted once already. Re-enable only when testing on a real Vita.
const NATIVE_OUTPUT_WIDTH: u32 = 960;
const NATIVE_OUTPUT_HEIGHT: u32 = 544;

// Sized to match `streaming::audio::MAX_PENDING_OPUS_PACKETS` - this Vec is the layer that
// feeds that channel, so it shouldn't hold more backlog than the channel behind it does.
const MAX_PENDING_AUDIO_PACKETS: usize = 6;

// GFN's server has no NACK/retransmission for the video stream: a lost UDP packet corrupts
// reference-frame decode until the next keyframe, which without prompting can be seconds away
// (seen as a stutter-then-black-frame freeze on lossy WiFi). Requesting one via RTCP PLI as
// soon as loss is detected recovers in one round-trip instead of waiting it out. Rate-limited
// so a burst of losses in the same round-trip doesn't spam the server with PLIs.
const PLI_MIN_INTERVAL: Duration = Duration::from_millis(250);

/// Pins the calling thread to the given user-CPU mask, logging (not failing) on error - a
/// missed pin just risks scheduler jitter, not correctness. Mirrors
/// `streaming::video::worker::pin_decoder_thread`.
#[cfg(target_os = "vita")]
fn pin_thread_to_cpu(mask: u32, thread_label: &str) {
    let thread_id = unsafe { vitasdk_sys::sceKernelGetThreadId() };
    let result =
        unsafe { vitasdk_sys::sceKernelChangeThreadCpuAffinityMask(thread_id, mask as i32) };
    if result < 0 {
        eprintln!("Failed to pin {thread_label} thread to CPU mask {mask:#x}: {result:#x}");
    }
}

/// The resolution NVIDIA actually streams at, per the session response.
fn stream_dimensions(session: &SessionInfo) -> (u32, u32) {
    session
        .negotiated_stream_profile
        .as_ref()
        .and_then(|profile| profile.resolution.as_deref())
        .and_then(|resolution| {
            let (width, height) = resolution.split_once('x')?;
            Some((width.parse().ok()?, height.parse().ok()?))
        })
        .filter(|(width, height)| *width > 0 && *height > 0)
        .unwrap_or((DEFAULT_STREAM_WIDTH, DEFAULT_STREAM_HEIGHT))
}

pub enum PeerEvent {
    /// Our SDP answer (plus its NVST parameter blob) is ready to go out via signaling.
    LocalAnswer {
        answer_sdp: String,
        nvst_sdp: String,
    },
    /// A local ICE candidate to trickle to the server via signaling.
    LocalIce(IceCandidate),
    /// Progress through the pipeline stages, for on-screen diagnostics.
    Status(String),
    Connected,
    Disconnected(String),
    Error(String),
}

enum PeerCommand {
    RemoteIce(IceCandidate),
    Gamepad(GamepadInput),
    Close,
}

pub struct PeerEngine {
    command_tx: mpsc::UnboundedSender<PeerCommand>,
    event_rx: mpsc::UnboundedReceiver<PeerEvent>,
    is_connected: Arc<AtomicBool>,
    video_output: Arc<DirectVideoOutput>,
    latest_frame: Arc<Mutex<Option<(u64, DecodedFrame)>>>,
    pending_audio: Arc<Mutex<Vec<Bytes>>>,
}

impl PeerEngine {
    pub fn new(offer_sdp: &str, session: &SessionInfo) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let is_connected = Arc::new(AtomicBool::new(false));
        let (stream_width, stream_height) = stream_dimensions(session);
        // Textures are sized to the Vita's actual panel resolution, not the (larger) resolution
        // GFN encodes at - AVCDEC downscales during decode-to-texture, see `NATIVE_OUTPUT_*`.
        // Real-hardware-only: see the note on those constants about Vita3K.
        let video_output = Arc::new(DirectVideoOutput::new(
            NATIVE_OUTPUT_WIDTH,
            NATIVE_OUTPUT_HEIGHT,
        ));
        let latest_frame: Arc<Mutex<Option<(u64, DecodedFrame)>>> = Arc::new(Mutex::new(None));
        let pending_audio: Arc<Mutex<Vec<Bytes>>> = Arc::new(Mutex::new(Vec::new()));

        let setup = PeerSetup {
            offer_sdp: offer_sdp.to_owned(),
            server_ip: session.server_ip.clone(),
            ice_servers: session
                .ice_servers
                .iter()
                .map(|server| RTCIceServer {
                    urls: server.urls.clone(),
                    username: server.username.clone().unwrap_or_default(),
                    credential: server.credential.clone().unwrap_or_default(),
                })
                .collect(),
            stream_width,
            stream_height,
        };

        let thread_events = event_tx.clone();
        let thread_connected = is_connected.clone();
        let thread_output = video_output.clone();
        let thread_frames = latest_frame.clone();
        let thread_audio = pending_audio.clone();
        std::thread::Builder::new()
            .name("jade-vita-peer".to_owned())
            .spawn(move || {
                // Pin to a dedicated core, so the OS scheduler can't
                // migrate/contend this thread against the UI (default core) or video decode
                // (pinned to USER_2 - see streaming::video::worker) threads under load.
                #[cfg(target_os = "vita")]
                pin_thread_to_cpu(vitasdk_sys::SCE_KERNEL_CPU_MASK_USER_1, "peer");
                // The sans-I/O peer loop gets its own runtime so its socket/timer waits never
                // touch the single-threaded runtime driving the UI (see main.rs).
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = thread_events
                            .send(PeerEvent::Error(format!("peer runtime failed: {error}")));
                        return;
                    }
                };
                let result = runtime.block_on(run_peer(
                    setup,
                    command_rx,
                    thread_events.clone(),
                    thread_connected,
                    thread_output,
                    thread_frames,
                    thread_audio,
                ));
                if let Err(error) = result {
                    let _ = thread_events
                        .send(PeerEvent::Disconnected(format!("peer loop ended: {error:#}")));
                }
            })
            .context("failed to spawn peer thread")?;

        Ok(Self {
            command_tx,
            event_rx,
            is_connected,
            video_output,
            latest_frame,
            pending_audio,
        })
    }

    pub fn is_connected(&self) -> bool {
        self.is_connected.load(Ordering::Relaxed)
    }

    pub fn try_recv(&mut self) -> Option<PeerEvent> {
        self.event_rx.try_recv().ok()
    }

    pub fn add_remote_ice(&self, candidate: IceCandidate) {
        let _ = self.command_tx.send(PeerCommand::RemoteIce(candidate));
    }

    /// Ships one controller snapshot to the game (timestamped inside the peer thread on the
    /// session clock). Dropped silently until the input channel handshake completes.
    pub fn send_gamepad(&self, input: GamepadInput) {
        let _ = self.command_tx.send(PeerCommand::Gamepad(input));
    }

    pub fn direct_video_output(&self) -> Arc<DirectVideoOutput> {
        self.video_output.clone()
    }

    pub fn video_frame(&self) -> Option<(u64, DecodedFrame)> {
        *self.latest_frame.lock().ok()?
    }

    /// Drains and returns whatever Opus packets have arrived since the last call - meant to
    /// be fed straight into `streaming::audio::AudioRenderer::submit_packets` once per frame.
    pub fn take_audio_packets(&self) -> Vec<Bytes> {
        self.pending_audio
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default()
    }
}

impl Drop for PeerEngine {
    fn drop(&mut self) {
        let _ = self.command_tx.send(PeerCommand::Close);
        // Wake anything parked waiting for a free texture.
        self.video_output.clear_targets();
    }
}

struct PeerSetup {
    offer_sdp: String,
    server_ip: String,
    ice_servers: Vec<RTCIceServer>,
    stream_width: u32,
    stream_height: u32,
}

/// Discover the local IP the OS routes toward the server - classic connected-UDP trick.
fn local_ip_toward(server_ip: &str) -> IpAddr {
    let target = crate::gfn::sdp::extract_public_ip(server_ip)
        .and_then(|ip| ip.parse::<Ipv4Addr>().ok())
        .map(IpAddr::V4)
        .unwrap_or(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)));
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect(SocketAddr::new(target, 443))?;
            socket.local_addr()
        })
        .map(|addr| addr.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

/// Previous-tick values of the decoder counters, so the readout can show per-second rates.
#[derive(Default)]
struct MetricsSnapshot {
    submitted: u64,
    queue_full: u64,
    decode_calls: u64,
    decode_us: u64,
    no_frame: u64,
    decode_errors: u64,
    target_stalls: u64,
}

impl MetricsSnapshot {
    fn capture(metrics: &crate::streaming::video::VideoMetrics) -> Self {
        Self {
            submitted: metrics.submitted.load(Ordering::Relaxed),
            queue_full: metrics.queue_full.load(Ordering::Relaxed),
            decode_calls: metrics.decode_calls.load(Ordering::Relaxed),
            decode_us: metrics.decode_us.load(Ordering::Relaxed),
            no_frame: metrics.no_frame.load(Ordering::Relaxed),
            decode_errors: metrics.decode_errors.load(Ordering::Relaxed),
            target_stalls: metrics.target_stalls.load(Ordering::Relaxed),
        }
    }
}

async fn run_peer(
    setup: PeerSetup,
    mut command_rx: mpsc::UnboundedReceiver<PeerCommand>,
    event_tx: mpsc::UnboundedSender<PeerEvent>,
    is_connected: Arc<AtomicBool>,
    video_output: Arc<DirectVideoOutput>,
    latest_frame: Arc<Mutex<Option<(u64, DecodedFrame)>>>,
    pending_audio: Arc<Mutex<Vec<Bytes>>>,
) -> Result<()> {
    // --- Hardware decode worker (sets `decoder_ready` so the shell creates the textures) ---
    let decode_worker = match VideoDecodeWorker::spawn(
        DecoderConfig {
            decode_width: setup.stream_width,
            decode_height: setup.stream_height,
            output_width: video_output.width,
            output_height: video_output.height,
        },
        video_output.clone(),
        latest_frame.clone(),
    ) {
        Ok(worker) => Some(worker),
        Err(error) => {
            // Non-fatal: negotiation still proceeds so the network path can be validated on
            // targets without the hardware decoder.
            let _ = event_tx.send(PeerEvent::Error(format!(
                "hardware decoder unavailable: {error:#}"
            )));
            None
        }
    };

    // --- Peer connection from NVIDIA's (sanitized) offer ---
    let sanitized_offer = crate::gfn::sdp::sanitize_offer(&setup.offer_sdp, &setup.server_ip);
    // Negotiation dumps for offline protocol debugging (world-readable like the rest of
    // ux0:data/jade-vita; the SDP holds only per-session credentials).
    let _ = std::fs::write("ux0:data/jade-vita/offer-raw.sdp", &setup.offer_sdp);
    let _ = std::fs::write("ux0:data/jade-vita/offer-sanitized.sdp", &sanitized_offer);
    let video_payload_types = crate::gfn::sdp::h264_payload_types(&sanitized_offer);
    let audio_payload_types = crate::gfn::sdp::opus_payload_types(&sanitized_offer);

    let mut media_engine = MediaEngine::default();
    media_engine
        .register_default_codecs()
        .context("failed to register codecs")?;
    // Our NVST answer already tells NVIDIA we support NACK-based retransmission
    // (`a=video.enableRtpNack:1` - see gfn::sdp::build_nvst_sdp_from_answer), but that's a
    // no-op unless something on our side actually generates NACKs. Without this, every lost
    // UDP packet corrupts the H.264 reference chain until a keyframe arrives, which is what
    // showed up as random stutter/black-frame freezes that OpenNOW (whose GStreamer
    // webrtcbin does this automatically) doesn't suffer from. This registers the same
    // per-packet-loss NACK generator/responder pair as the free equivalent.
    let registry = configure_nack(Registry::new(), &mut media_engine);
    // Without this, NVIDIA's `bwe.useOwdCongestionControl` has nothing to react to from us -
    // NACK alone tells it about specific lost packets, not the ongoing loss/jitter picture RTCP
    // Receiver Reports give its congestion control. Cheap to add (also ships in this crate),
    // pure upside: it's receiver-generated feedback about our own inbound video/audio, not
    // something we depend on the server to also send.
    let registry = configure_rtcp_reports(registry);
    // NVIDIA's server is ICE-lite, which makes us the ICE controlling agent; rtc's Auto rule
    // (controlling → DTLS server) would answer `a=setup:passive` and then both sides sit
    // waiting for the other's ClientHello. GFN servers never act as DTLS client, so force the
    // standard browser behavior: answer `active` and initiate the handshake ourselves.
    let mut setting_engine = SettingEngine::default();
    setting_engine
        .set_answering_dtls_role(RTCDtlsRole::Client)
        .context("failed to force DTLS client role")?;
    let mut pc = RTCPeerConnectionBuilder::new()
        .with_configuration(
            RTCConfigurationBuilder::new()
                .with_ice_servers(setup.ice_servers.clone())
                .build(),
        )
        .with_media_engine(media_engine)
        .with_setting_engine(setting_engine)
        .with_interceptor_registry(registry)
        .build()
        .context("failed to build peer connection")?;

    let offer = RTCSessionDescription::offer(sanitized_offer)
        .context("NVIDIA offer SDP was rejected by the SDP parser")?;
    pc.set_remote_description(offer)
        .context("failed to apply NVIDIA offer")?;

    // --- Input data channels (must exist before the answer so their SCTP streams are
    //     negotiated; the offer already carries the m=application section) ---
    let input_channel_id = match pc.create_data_channel("input_channel_v1", None) {
        Ok(channel) => Some(channel.id()),
        Err(error) => {
            let _ = event_tx.send(PeerEvent::Error(format!(
                "input channel creation failed: {error}"
            )));
            None
        }
    };
    // NOTE: NVST advertises a second `input_channel_partially_reliable_v1` channel for
    // lower-latency unordered gamepad state, but that path hasn't been verified against the real
    // GFN server yet - on hardware it connected fine (video/audio worked) while every
    // button/stick press did nothing, meaning the server never picked up packets sent on that
    // second channel. Sending gamepad state on the known-working reliable `input_channel_v1`
    // instead until the partially-reliable framing can be confirmed against a real capture.
    let mut input_encoder = InputEncoder::default();
    let mut input_ready = false;
    let session_clock = Instant::now();

    // --- UDP socket + local host candidate ---
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0")
        .await
        .context("failed to bind media UDP socket")?;
    let bound_port = socket.local_addr()?.port();
    let local_ip = local_ip_toward(&setup.server_ip);
    let local_addr = SocketAddr::new(local_ip, bound_port);

    let host_candidate = CandidateHostConfig {
        base_config: CandidateConfig {
            network: "udp".to_owned(),
            address: local_ip.to_string(),
            port: bound_port,
            component: 1,
            ..Default::default()
        },
        ..Default::default()
    }
    .new_candidate_host()
    .context("failed to create host candidate")?;
    let local_candidate_init: RTCIceCandidateInit = RTCIceCandidate::from(&host_candidate)
        .to_json()
        .context("failed to serialize host candidate")?;
    pc.add_local_candidate(local_candidate_init.clone())
        .context("failed to add local candidate")?;

    // --- Answer ---
    let answer = pc.create_answer(None).context("failed to create answer")?;
    pc.set_local_description(answer.clone())
        .context("failed to set local description")?;
    let answer_sdp = answer.sdp.clone();
    let _ = std::fs::write("ux0:data/jade-vita/answer.sdp", &answer_sdp);
    let nvst_sdp = crate::gfn::sdp::build_nvst_sdp_from_answer(
        &answer_sdp,
        &crate::gfn::cloudmatch::StreamSettings::for_vita(),
    );
    let our_ufrag = crate::gfn::sdp::extract_ice_credentials(&answer_sdp).ufrag;
    let _ = event_tx.send(PeerEvent::LocalAnswer {
        answer_sdp,
        nvst_sdp,
    });
    let _ = event_tx.send(PeerEvent::LocalIce(IceCandidate {
        candidate: local_candidate_init.candidate.clone(),
        sdp_mid: Some("0".to_owned()),
        sdp_m_line_index: Some(0),
        username_fragment: Some(our_ufrag),
    }));

    // --- Sans-I/O event loop ---
    // Full reassembly + loss-aware recovery (buffered/sequence-verified frame assembly,
    // damage-score-gated decoder resync, keyframe-gated resume) - see `gfn::rtp` for why this
    // replaces a naive "extend on arrival, flush on marker" accumulator.
    let mut video_rtp = crate::gfn::rtp::VideoRtp::new(setup.stream_width, setup.stream_height);
    let mut buf = vec![0u8; 2000];
    let mut first_rtp_seen = false;
    let mut first_au_submitted = false;
    let mut video_receiver_id: Option<RTCRtpReceiverId> = None;
    let mut video_ssrc: Option<u32> = None;
    let mut last_pli_sent: Option<Instant> = None;
    let mut pli_sent_count: u64 = 0;
    let mut dropped_frames_total: u64 = 0;
    // Raw pipeline counters surfaced on-screen every few seconds - the fastest way to see
    // which stage a stalled stream died at without console access on the Vita. In/out packet
    // classes tell apart "our DTLS ClientHello never leaves" from "NVIDIA never answers it".
    let mut in_stun: u64 = 0;
    let mut in_dtls: u64 = 0;
    let mut in_media: u64 = 0;
    let mut out_stun: u64 = 0;
    let mut out_dtls: u64 = 0;
    let mut out_media: u64 = 0;
    let mut rtp_packets: u64 = 0;
    let mut access_units_sent: u64 = 0;
    let mut frames_decoded_last: u64 = 0;
    // Previous-tick snapshots, so the readout can report per-second *rates* rather than
    // ever-growing totals. Rates are what identify the bottleneck stage: see the comment on
    // the stats tick below.
    let mut rtp_packets_last: u64 = 0;
    let mut access_units_last: u64 = 0;
    let mut dropped_frames_last: u64 = 0;
    let mut stats_last_at = Instant::now();
    let decoder_metrics = decode_worker.as_ref().map(|worker| worker.metrics());
    let mut metrics_last = MetricsSnapshot::default();
    // First byte of a UDP payload: 0-3 STUN, 20-63 DTLS records, 128-191 RTP/RTCP.
    fn classify(first_byte: Option<&u8>) -> usize {
        match first_byte {
            Some(0..=3) => 0,
            Some(20..=63) => 1,
            Some(128..=191) => 2,
            _ => 2,
        }
    }
    // 1s, not 3s: this now reports live rates (fps etc.), and a 3s window smears a stutter that
    // lasts under a second into an average that looks fine.
    let mut stats_interval = tokio::time::interval(Duration::from_secs(1));
    stats_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(2));
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut pending_commands: Vec<PeerCommand> = Vec::new();
    const IDLE_TIMEOUT: Duration = Duration::from_secs(86400);

    loop {
        while let Some(msg) = pc.poll_write() {
            match classify(msg.message.first()) {
                0 => out_stun += 1,
                1 => out_dtls += 1,
                _ => out_media += 1,
            }
            let _ = socket.send_to(&msg.message, msg.transport.peer_addr).await;
        }

        while let Some(event) = pc.poll_event() {
            match event {
                RTCPeerConnectionEvent::OnConnectionStateChangeEvent(state) => match state {
                    RTCPeerConnectionState::Connected => {
                        is_connected.store(true, Ordering::Relaxed);
                        let _ = event_tx.send(PeerEvent::Connected);
                    }
                    RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
                        let _ = event_tx.send(PeerEvent::Disconnected(format!(
                            "peer connection state: {state}"
                        )));
                        return Ok(());
                    }
                    other => {
                        let _ = event_tx.send(PeerEvent::Status(format!("Conexión: {other}")));
                    }
                },
                RTCPeerConnectionEvent::OnIceConnectionStateChangeEvent(state) => {
                    let _ = event_tx.send(PeerEvent::Status(format!("ICE: {state}")));
                }
                RTCPeerConnectionEvent::OnTrack(track_event) => {
                    if let RTCTrackEvent::OnOpen(init) = &track_event
                        && let Some(receiver) = pc.rtp_receiver(init.receiver_id)
                        && receiver.track().kind() == RtpCodecKind::Video
                    {
                        video_receiver_id = Some(init.receiver_id);
                        // `OnOpen` fires on receipt of the first RTP packet for this stream, so
                        // its ssrc is already known here - no need to wait for that packet to
                        // reach the `RtpPacket` match arm below.
                        video_ssrc = Some(init.ssrc);
                    }
                    let _ = event_tx.send(PeerEvent::Status("Track de media abierto".to_owned()));
                }
                RTCPeerConnectionEvent::OnDataChannel(RTCDataChannelEvent::OnOpen(channel_id)) => {
                    // The channel reaching "open" is WebRTC's own authoritative readiness
                    // signal - it's what actually lets `channel.send` succeed, unlike the
                    // handshake-byte sniff below, which depends on the server choosing to send
                    // a specific payload we may never see (this was the actual bug: the server
                    // apparently doesn't send it, so `input_ready` never flipped and every
                    // button/stick press was silently dropped despite video/audio streaming
                    // fine over the same connection).
                    if Some(channel_id) == input_channel_id {
                        input_ready = true;
                        let _ = event_tx
                            .send(PeerEvent::Status("Canal de input abierto".to_owned()));
                    }
                }
                _ => {}
            }
        }

        while let Some(message) = pc.poll_read() {
            if let RTCMessage::DataChannelMessage(_channel_id, dc_message) = &message {
                // Opportunistic: if the server *does* send its handshake, pick up the protocol
                // version from it, but readiness itself no longer depends on this arriving -
                // see the `OnDataChannel`/`OnOpen` handling above.
                if let Some(version) = parse_input_handshake_version(&dc_message.data) {
                    input_ready = true;
                    input_encoder.set_protocol_version(version.min(u8::MAX as u16) as u8);
                    let _ = event_tx.send(PeerEvent::Status(format!(
                        "Canal de input listo (protocolo v{version})"
                    )));
                }
                continue;
            }
            if let RTCMessage::RtpPacket(_track_id, packet) = message {
                rtp_packets += 1;
                if !first_rtp_seen {
                    first_rtp_seen = true;
                    let _ = event_tx.send(PeerEvent::Status(format!(
                        "Recibiendo RTP (payload type {})",
                        packet.header.payload_type
                    )));
                }
                if audio_payload_types.contains(&packet.header.payload_type) {
                    // Opus packets are already complete frames on the wire - no
                    // depacketization needed, just hand the RTP payload to the decoder. Capped
                    // and discard-oldest so a delayed shell-frame drain can't let this grow
                    // into an ever-larger stale backlog (mirrors the gamepad
                    // pending_commands/latest_gamepad coalescing below, adapted to keep a short
                    // *ordered* run of packets rather than collapsing to one, since Opus frames
                    // must decode in sequence).
                    if let Ok(mut queue) = pending_audio.lock() {
                        if queue.len() >= MAX_PENDING_AUDIO_PACKETS {
                            queue.remove(0);
                        }
                        queue.push(packet.payload.clone());
                    }
                    continue;
                }
                let is_video = video_payload_types.is_empty()
                    || video_payload_types.contains(&packet.header.payload_type);
                if !is_video {
                    continue;
                }
                video_ssrc = Some(packet.header.ssrc);
                let mut keyframe_requested = false;
                let sample_stats = if let Some(worker) = &decode_worker {
                    video_rtp.receive(worker, packet, &mut keyframe_requested)
                } else {
                    continue;
                };
                dropped_frames_total += u64::from(sample_stats.dropped);
                if sample_stats.source_frame_duration_us.is_some() {
                    access_units_sent += 1;
                    if !first_au_submitted {
                        first_au_submitted = true;
                        let _ = event_tx.send(PeerEvent::Status("Decodificando H.264".to_owned()));
                    }
                }
                // Ask the server for a fresh keyframe instead of waiting out the corrupted
                // reference-frame chain until its next scheduled one (which is what showed up
                // as random stutter-then-black-frame freezes on lossy WiFi).
                if keyframe_requested
                    && let (Some(receiver_id), Some(ssrc)) = (video_receiver_id, video_ssrc)
                {
                    let now = Instant::now();
                    let should_send = last_pli_sent
                        .map(|last| now.duration_since(last) >= PLI_MIN_INTERVAL)
                        .unwrap_or(true);
                    if should_send
                        && let Some(mut receiver) = pc.rtp_receiver(receiver_id)
                        && receiver
                            .write_rtcp(vec![Box::new(PictureLossIndication {
                                sender_ssrc: 0,
                                media_ssrc: ssrc,
                            })])
                            .is_ok()
                    {
                        last_pli_sent = Some(now);
                        pli_sent_count += 1;
                    }
                }
            }
        }

        let timeout = pc
            .poll_timeout()
            .unwrap_or_else(|| Instant::now() + IDLE_TIMEOUT);
        let delay = timeout.saturating_duration_since(Instant::now());
        if delay.is_zero() {
            pc.handle_timeout(Instant::now())?;
            continue;
        }

        let timer = tokio::time::sleep(delay);
        tokio::pin!(timer);

        tokio::select! {
            biased;

            _ = &mut timer => {
                pc.handle_timeout(Instant::now())?;
            }
            _ = heartbeat_interval.tick() => {
                if input_ready && let Some(id) = input_channel_id {
                    let heartbeat = input_encoder.encode_heartbeat();
                    if let Some(mut channel) = pc.data_channel(id) {
                        let _ = channel.send(BytesMut::from(&heartbeat[..]));
                    }
                }
            }
            _ = stats_interval.tick() => {
                let frames = latest_frame
                    .lock()
                    .ok()
                    .and_then(|slot| slot.map(|(id, _)| id))
                    .unwrap_or(0);

                // Live pipeline rates, always shown while streaming. Previously this only
                // reported when the picture was completely frozen (`frames == frames_decoded_last`),
                // which meant a stream that was merely *slow* displayed nothing at all - leaving
                // no way to tell which stage was losing frames without console access.
                //
                // How to read it, in pipeline order - the first rate that is too low is the
                // culprit, and each rules out everything downstream of it:
                //   rtp/s  - packets off the wire. Low => network or the encoder isn't sending.
                //   au/s   - assembled H.264 access units. Much lower than the expected fps while
                //            rtp/s is healthy => packet loss is destroying frames (check pli).
                //   fps    - frames the decoder actually published. Below au/s => the hardware
                //            decoder can't keep up and `submit_access_unit` is dropping.
                //   drop/s - frames the RTP layer discarded as damaged.
                //   pli    - keyframe requests; climbing steadily means sustained loss.
                let elapsed = stats_last_at.elapsed().as_secs_f32().max(0.001);
                stats_last_at = Instant::now();
                let rate = |now: u64, then: u64| (now.saturating_sub(then)) as f32 / elapsed;
                let fps = rate(frames, frames_decoded_last);
                let rtp_rate = rate(rtp_packets, rtp_packets_last);
                // Note: this counts frame *timestamps observed by RTP*, i.e. the source
                // framerate - not access units handed to the decoder. `sub` below is the real
                // "reached the decode queue" number.
                let src_rate = rate(access_units_sent, access_units_last);
                let drop_rate = rate(dropped_frames_total, dropped_frames_last);
                rtp_packets_last = rtp_packets;
                access_units_last = access_units_sent;
                dropped_frames_last = dropped_frames_total;

                // Decoder-side counters, which is where the frames were actually going missing.
                let (sub, qfull, calls, dec_us, noframe, errs, rebuilds, stalls) = match &decoder_metrics {
                    Some(m) => (
                        rate(m.submitted.load(Ordering::Relaxed), metrics_last.submitted),
                        rate(m.queue_full.load(Ordering::Relaxed), metrics_last.queue_full),
                        m.decode_calls.load(Ordering::Relaxed),
                        m.decode_us.load(Ordering::Relaxed),
                        rate(m.no_frame.load(Ordering::Relaxed), metrics_last.no_frame),
                        rate(m.decode_errors.load(Ordering::Relaxed), metrics_last.decode_errors),
                        m.decoder_rebuilds.load(Ordering::Relaxed),
                        rate(m.target_stalls.load(Ordering::Relaxed), metrics_last.target_stalls),
                    ),
                    None => (0.0, 0.0, 0, 0, 0.0, 0.0, 0, 0.0),
                };
                // Mean wall time inside `HwVideoDecoder::decode`. This is the number that says
                // whether the hardware decoder itself is the bottleneck: at 60 fps it has to
                // average under ~16ms.
                let avg_decode_ms = if calls > metrics_last.decode_calls {
                    (dec_us - metrics_last.decode_us) as f32
                        / (calls - metrics_last.decode_calls) as f32
                        / 1000.0
                } else {
                    0.0
                };
                if let Some(m) = &decoder_metrics {
                    metrics_last = MetricsSnapshot::capture(m);
                }

                // `in:` is the input data channel. GFN ends a session it considers idle (see
                // `userIdleWarningTimeoutInMs` in the CloudMatch response), and idleness is
                // judged from what arrives on this channel - so `in:0` during play means the
                // session will be terminated early no matter how good the video looks.
                let _ = event_tx.send(PeerEvent::Status(format!(
                    "fps:{fps:.0} src:{src_rate:.0} sub:{sub:.0} qf:{qfull:.0} dec:{avg_decode_ms:.0}ms nof:{noframe:.0} err:{errs:.0} reb:{rebuilds} stall:{stalls:.0} rtp:{rtp_rate:.0} drop:{drop_rate:.0} pli:{pli_sent_count} wfk:{} in:{}",
                    u8::from(video_rtp.waiting_for_keyframe()),
                    u8::from(input_ready)
                )));

                // Handshake/transport totals stay available, but only while no picture has
                // arrived yet - that's the only time they're the interesting question.
                if frames == 0 {
                    let _ = event_tx.send(PeerEvent::Status(format!(
                        "IN s:{in_stun} d:{in_dtls} m:{in_media} | OUT s:{out_stun} d:{out_dtls} m:{out_media} | RTP:{rtp_packets} AU:{access_units_sent}"
                    )));
                }

                // Stall watchdog, as before - only when the picture is genuinely not advancing.
                if fps == 0.0 {
                    // Stall watchdog (generalized from an initial-keyframe deadline,
                    // generalized to any stall, not just the first frame): the per-packet PLI
                    // above only fires when new (damaged) RTP arrives, so if the stream has
                    // gone fully silent - e.g. the one PLI that would have unstuck it got lost
                    // too - nothing would ever ask again. Piggyback a periodic nudge on this
                    // same stats tick (rate-limited by `PLI_MIN_INTERVAL` regardless).
                    if is_connected.load(Ordering::Relaxed)
                        && let (Some(receiver_id), Some(ssrc)) = (video_receiver_id, video_ssrc)
                    {
                        let now = Instant::now();
                        let should_send = last_pli_sent
                            .map(|last| now.duration_since(last) >= PLI_MIN_INTERVAL)
                            .unwrap_or(true);
                        if should_send
                            && let Some(mut receiver) = pc.rtp_receiver(receiver_id)
                            && receiver
                                .write_rtcp(vec![Box::new(PictureLossIndication {
                                    sender_ssrc: 0,
                                    media_ssrc: ssrc,
                                })])
                                .is_ok()
                        {
                            last_pli_sent = Some(now);
                            pli_sent_count += 1;
                        }
                    }
                }
                frames_decoded_last = frames;
            }
            command = command_rx.recv() => {
                match command {
                    Some(command) => pending_commands.push(command),
                    None => pending_commands.push(PeerCommand::Close),
                }
            }
            received = socket.recv_from(&mut buf) => {
                if let Ok((n, peer_addr)) = received {
                    match classify(buf.first()) {
                        0 => in_stun += 1,
                        1 => in_dtls += 1,
                        _ => in_media += 1,
                    }
                    pc.handle_read(TaggedBytesMut {
                        now: Instant::now(),
                        transport: TransportContext {
                            local_addr,
                            peer_addr,
                            ecn: None,
                            transport_protocol: TransportProtocol::UDP,
                        },
                        message: BytesMut::from(&buf[..n]),
                    })?;
                }
            }
        }

        // Latency control: drain everything already queued before the next poll cycle.
        // Handling one datagram/command per wakeup lets the OS socket buffer (and with it,
        // glass-to-glass delay) grow without bound during video bursts.
        while let Ok((n, peer_addr)) = socket.try_recv_from(&mut buf) {
            match classify(buf.first()) {
                0 => in_stun += 1,
                1 => in_dtls += 1,
                _ => in_media += 1,
            }
            pc.handle_read(TaggedBytesMut {
                now: Instant::now(),
                transport: TransportContext {
                    local_addr,
                    peer_addr,
                    ecn: None,
                    transport_protocol: TransportProtocol::UDP,
                },
                message: BytesMut::from(&buf[..n]),
            })?;
        }
        while let Ok(command) = command_rx.try_recv() {
            pending_commands.push(command);
        }

        // Coalesce queued gamepad snapshots down to the newest one - the game only cares
        // about current stick/button state, and replaying a backlog adds input latency.
        let mut latest_gamepad = None;
        for command in pending_commands.drain(..) {
            match command {
                PeerCommand::RemoteIce(candidate) => {
                    let init = RTCIceCandidateInit {
                        candidate: candidate.candidate,
                        sdp_mid: candidate.sdp_mid,
                        sdp_mline_index: candidate.sdp_m_line_index.map(|index| index as u16),
                        username_fragment: candidate.username_fragment,
                        ..Default::default()
                    };
                    if let Err(error) = pc.add_remote_candidate(init) {
                        let _ = event_tx.send(PeerEvent::Error(format!(
                            "remote ICE candidate rejected: {error}"
                        )));
                    }
                }
                PeerCommand::Gamepad(input) => latest_gamepad = Some(input),
                PeerCommand::Close => {
                    let _ = pc.close();
                    return Ok(());
                }
            }
        }
        if let Some(mut input) = latest_gamepad
            && input_ready
            && let Some(id) = input_channel_id
        {
            input.timestamp_us = session_clock.elapsed().as_micros() as u64;
            let packet = input_encoder.encode_gamepad_state(GAMEPAD_BITMAP_PRIMARY, input);
            if let Some(mut channel) = pc.data_channel(id) {
                let _ = channel.send(BytesMut::from(&packet[..]));
            }
        }
    }
}
