//! Real WebRTC peer for GeForce NOW streaming, built on the sans-I/O `rtc` crate.

use crate::gfn::cloudmatch::SessionInfo;
use crate::gfn::input_protocol::{
    GAMEPAD_BITMAP_PRIMARY, GamepadInput, InputEncoder, KeyStroke, MouseEvent,
    parse_input_handshake_version,
};
use crate::gfn::signaling::IceCandidate;
use crate::streaming::audio::AudioPacket;
use crate::streaming::video::{
    DecodedFrame, DecoderConfig, DirectVideoOutput, VideoDecodeWorker,
};
use anyhow::{Context, Result};
use bytes::BytesMut;
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const DEFAULT_STREAM_WIDTH: u32 = 1280;
const DEFAULT_STREAM_HEIGHT: u32 = 720;

const NATIVE_OUTPUT_WIDTH: u32 = 960;
const NATIVE_OUTPUT_HEIGHT: u32 = 544;

const PLI_MIN_INTERVAL: Duration = Duration::from_millis(250);


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
    // session time warning from control_channel
    TimeWarning { code: u32, seconds_left: u32 },
}

enum PeerCommand {
    RemoteIce(IceCandidate),
    Gamepad(GamepadInput),
    Mouse(MouseEvent),
    Key { key: KeyStroke, pressed: bool },
    SetMaxBitrate(u32),
    Close,
}

pub struct PeerEngine {
    command_tx: mpsc::UnboundedSender<PeerCommand>,
    event_rx: mpsc::UnboundedReceiver<PeerEvent>,
    is_connected: Arc<AtomicBool>,
    video_output: Arc<DirectVideoOutput>,
    latest_frame: Arc<Mutex<Option<(u64, DecodedFrame)>>>,
    /// Cumulative bytes of inbound media (RTP/RTCP) since the peer started - the raw material for
    /// estimating what this network actually delivered.
    media_bytes: Arc<AtomicU64>,
    /// Keyframes asked for because a frame arrived damaged. The honest signal for "this link is
    /// struggling": throughput alone only says how busy the game was.
    keyframe_requests: Arc<AtomicU64>,
}

impl PeerEngine {
    pub fn new(offer_sdp: &str, session: &SessionInfo) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let is_connected = Arc::new(AtomicBool::new(false));
        let (stream_width, stream_height) = stream_dimensions(session);
        let video_output = Arc::new(DirectVideoOutput::new(
            NATIVE_OUTPUT_WIDTH,
            NATIVE_OUTPUT_HEIGHT,
        ));
        let latest_frame: Arc<Mutex<Option<(u64, DecodedFrame)>>> = Arc::new(Mutex::new(None));
        let media_bytes = Arc::new(AtomicU64::new(0));
        let keyframe_requests = Arc::new(AtomicU64::new(0));

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
        let thread_media_bytes = media_bytes.clone();
        let thread_keyframe_requests = keyframe_requests.clone();
        std::thread::Builder::new()
            .name("opennow-vita-peer".to_owned())
            .spawn(move || {
                crate::thread_affinity::pin_current_thread(
                    crate::thread_affinity::VitaCore::Network,
                    "peer",
                );
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
                    thread_media_bytes,
                    thread_keyframe_requests,
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
            media_bytes,
            keyframe_requests,
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
    /// session clock).
    pub fn send_gamepad(&self, input: GamepadInput) {
        let _ = self.command_tx.send(PeerCommand::Gamepad(input));
    }

    /// Ships one pointer event to the host desktop. Unlike gamepad snapshots these are discrete
    /// events, so they are queued rather than coalesced - dropping a button-up would leave the
    /// host holding the mouse down.
    pub fn send_mouse(&self, event: MouseEvent) {
        let _ = self.command_tx.send(PeerCommand::Mouse(event));
    }

    /// Ships one key press or release to the game. Queued like mouse events rather than
    /// coalesced: a dropped key-up would leave the host holding the key down.
    pub fn send_key(&self, key: KeyStroke, pressed: bool) {
        let _ = self.command_tx.send(PeerCommand::Key { key, pressed });
    }

    /// Presses and releases a key, which is what tapping one on the on-screen keyboard means.
    pub fn tap_key(&self, key: KeyStroke) {
        self.send_key(key, true);
        self.send_key(key, false);
    }

    /// Cumulative inbound media bytes, sampled by the app to estimate the link's real capacity.
    pub fn media_bytes(&self) -> u64 {
        self.media_bytes.load(Ordering::Relaxed)
    }

    /// How many times a damaged frame forced a keyframe request this session.
    pub fn keyframe_requests(&self) -> u64 {
        self.keyframe_requests.load(Ordering::Relaxed)
    }

    // reapplies local desc with the new bitrate baked in
    pub fn set_max_bitrate(&self, kbps: u32) {
        let _ = self.command_tx.send(PeerCommand::SetMaxBitrate(kbps));
    }

    pub fn direct_video_output(&self) -> Arc<DirectVideoOutput> {
        self.video_output.clone()
    }

    pub fn video_frame(&self) -> Option<(u64, DecodedFrame)> {
        *self.latest_frame.lock().ok()?
    }

}

impl Drop for PeerEngine {
    fn drop(&mut self) {
        let _ = self.command_tx.send(PeerCommand::Close);
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
    target_wait_us: u64,
    target_wait_calls: u64,
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
            target_wait_us: metrics.target_wait_us.load(Ordering::Relaxed),
            target_wait_calls: metrics.target_wait_calls.load(Ordering::Relaxed),
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
    media_bytes: Arc<AtomicU64>,
    keyframe_requests: Arc<AtomicU64>,
) -> Result<()> {
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
            let _ = event_tx.send(PeerEvent::Error(format!(
                "hardware decoder unavailable: {error:#}"
            )));
            None
        }
    };

    let sanitized_offer = crate::gfn::sdp::sanitize_offer(&setup.offer_sdp, &setup.server_ip);
    let ri_caps = crate::gfn::sdp::parse_ri_input_capabilities(&setup.offer_sdp);
    let _ = std::fs::write("ux0:data/opennow-vita/offer-raw.sdp", &setup.offer_sdp);
    let _ = std::fs::write("ux0:data/opennow-vita/offer-sanitized.sdp", &sanitized_offer);
    let video_payload_types = crate::gfn::sdp::h264_payload_types(&sanitized_offer);
    let audio_payload_types = crate::gfn::sdp::opus_payload_types(&sanitized_offer);

    let mut media_engine = MediaEngine::default();
    media_engine
        .register_default_codecs()
        .context("failed to register codecs")?;
    let registry = configure_nack(Registry::new(), &mut media_engine);
    let registry = configure_rtcp_reports(registry);
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

    let input_channel_id = match pc.create_data_channel("input_channel_v1", None) {
        Ok(channel) => Some(channel.id()),
        Err(error) => {
            let _ = event_tx.send(PeerEvent::Error(format!(
                "input channel creation failed: {error}"
            )));
            None
        }
    };
    let partial_input_channel_id = match pc.create_data_channel(
        "input_channel_partially_reliable",
        Some(rtc::data_channel::RTCDataChannelInit {
            ordered: false,
            max_packet_life_time: Some(ri_caps.partial_reliable_threshold_ms),
            ..Default::default()
        }),
    ) {
        Ok(channel) => Some(channel.id()),
        Err(error) => {
            let _ = event_tx.send(PeerEvent::Status(format!(
                "partial input channel creation failed: {error}"
            )));
            None
        }
    };
    let mut partial_input_ready = false;
    let mut partial_sequence: u16 = 0;

    let mut control_channel_id: Option<rtc::data_channel::RTCDataChannelId> = None;

    let mut input_encoder = InputEncoder::default();
    let mut input_ready = false;
    let session_clock = Instant::now();

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

    let answer = pc.create_answer(None).context("failed to create answer")?;
    pc.set_local_description(answer.clone())
        .context("failed to set local description")?;
    let stream_settings = crate::gfn::cloudmatch::StreamSettings::for_vita();
    let munged_answer_sdp = crate::gfn::sdp::munge_answer_sdp(
        &answer.sdp,
        stream_settings.max_bitrate_mbps * 1000,
    );
    let mut saved_answer_sdp = munged_answer_sdp.clone();
    let answer_sdp = answer.sdp.clone();
    let _ = std::fs::write("ux0:data/opennow-vita/answer.sdp", &saved_answer_sdp);
    let nvst_sdp = crate::gfn::sdp::build_nvst_sdp_from_answer(
        &answer_sdp,
        &stream_settings,
        &ri_caps,
    );
    let our_ufrag = crate::gfn::sdp::extract_ice_credentials(&answer_sdp).ufrag;
    let _ = event_tx.send(PeerEvent::LocalAnswer {
        answer_sdp: saved_answer_sdp.clone(),
        nvst_sdp,
    });
    let _ = event_tx.send(PeerEvent::LocalIce(IceCandidate {
        candidate: local_candidate_init.candidate.clone(),
        sdp_mid: Some("0".to_owned()),
        sdp_m_line_index: Some(0),
        username_fragment: Some(our_ufrag),
    }));

    let mut video_rtp = crate::gfn::rtp::VideoRtp::new(setup.stream_width, setup.stream_height);
    let mut buf = vec![0u8; 2000];
    let mut first_rtp_seen = false;
    let mut first_au_submitted = false;
    let mut video_receiver_id: Option<RTCRtpReceiverId> = None;
    let mut video_ssrc: Option<u32> = None;
    let mut last_pli_sent: Option<Instant> = None;
    let mut pli_sent_count: u64 = 0;
    let mut dropped_frames_total: u64 = 0;
    let mut in_stun: u64 = 0;
    let mut in_dtls: u64 = 0;
    let mut in_media: u64 = 0;
    let mut out_stun: u64 = 0;
    let mut out_dtls: u64 = 0;
    let mut out_media: u64 = 0;
    let mut rtp_packets: u64 = 0;
    let mut access_units_sent: u64 = 0;
    let mut frames_decoded_last: u64 = 0;
    let mut rtp_packets_last: u64 = 0;
    let mut access_units_last: u64 = 0;
    let mut dropped_frames_last: u64 = 0;
    let mut stats_last_at = Instant::now();
    let decoder_metrics = decode_worker.as_ref().map(|worker| worker.metrics());
    let mut metrics_last = MetricsSnapshot::default();
    fn classify(first_byte: Option<&u8>) -> usize {
        match first_byte {
            Some(0..=3) => 0,
            Some(20..=63) => 1,
            Some(128..=191) => 2,
            _ => 2,
        }
    }
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
                        // stays english, this is just debug status from the peer thread, no locale here
                        let _ = event_tx.send(PeerEvent::Status(format!("Connection: {other}")));
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
                        video_ssrc = Some(init.ssrc);
                    }
                    let _ = event_tx.send(PeerEvent::Status("Track de media abierto".to_owned()));
                }
                RTCPeerConnectionEvent::OnDataChannel(RTCDataChannelEvent::OnOpen(channel_id)) => {
                    if Some(channel_id) == input_channel_id {
                        input_ready = true;
                        let _ = event_tx
                            .send(PeerEvent::Status("Canal de input abierto".to_owned()));
                    } else if Some(channel_id) == partial_input_channel_id {
                        partial_input_ready = true;
                        let _ = event_tx
                            .send(PeerEvent::Status("Canal parcial abierto".to_owned()));
                    } else if let Some(channel) = pc.data_channel(channel_id) {
                        if channel.label() == "control_channel" {
                            control_channel_id = Some(channel_id);
                            let _ = event_tx.send(PeerEvent::Status("Control channel abierto".to_owned()));
                        }
                    }
                }
                _ => {}
            }
        }

        while let Some(message) = pc.poll_read() {
            if let RTCMessage::DataChannelMessage(channel_id, dc_message) = &message {
                if Some(*channel_id) == control_channel_id {
                    if let Ok(text) = std::str::from_utf8(&dc_message.data) {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(text) {
                            if val.get("type").and_then(|v| v.as_str()) == Some("timerNotification") {
                                let code = val.get("code").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                                let seconds_left = val.get("secondsLeft").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                                let _ = event_tx.send(PeerEvent::TimeWarning { code, seconds_left });
                            }
                        }
                    }
                    continue;
                }
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
                    // Straight to the decode thread, on arrival. The sequence number and payload
                    // type travel with the payload: the audio pipeline needs them to reorder the
                    // stream and to unwrap RED redundancy.
                    crate::streaming::audio::submit_packet(AudioPacket {
                        payload: packet.payload.clone(),
                        sequence: packet.header.sequence_number,
                        payload_type: packet.header.payload_type,
                    });
                    continue;
                }
                let is_video = video_payload_types.is_empty()
                    || video_payload_types.contains(&packet.header.payload_type);
                if !is_video {
                    continue;
                }
                video_ssrc = Some(packet.header.ssrc);
                let mut keyframe_requested = false;
                let arrival_us = session_clock.elapsed().as_micros() as u64;
                let sample_stats = if let Some(worker) = &decode_worker {
                    video_rtp.receive(worker, packet, &mut keyframe_requested, arrival_us)
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
                        keyframe_requests.fetch_add(1, Ordering::Relaxed);
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

                let elapsed = stats_last_at.elapsed().as_secs_f32().max(0.001);
                stats_last_at = Instant::now();
                let rate = |now: u64, then: u64| (now.saturating_sub(then)) as f32 / elapsed;
                let fps = rate(frames, frames_decoded_last);
                let rtp_rate = rate(rtp_packets, rtp_packets_last);
                let src_rate = rate(access_units_sent, access_units_last);
                let drop_rate = rate(dropped_frames_total, dropped_frames_last);
                rtp_packets_last = rtp_packets;
                access_units_last = access_units_sent;
                dropped_frames_last = dropped_frames_total;

                let (sub, qfull, calls, dec_us, noframe, errs, rebuilds, stalls, wait_us, wait_calls) = match &decoder_metrics {
                    Some(m) => (
                        rate(m.submitted.load(Ordering::Relaxed), metrics_last.submitted),
                        rate(m.queue_full.load(Ordering::Relaxed), metrics_last.queue_full),
                        m.decode_calls.load(Ordering::Relaxed),
                        m.decode_us.load(Ordering::Relaxed),
                        rate(m.no_frame.load(Ordering::Relaxed), metrics_last.no_frame),
                        rate(m.decode_errors.load(Ordering::Relaxed), metrics_last.decode_errors),
                        m.decoder_rebuilds.load(Ordering::Relaxed),
                        rate(m.target_stalls.load(Ordering::Relaxed), metrics_last.target_stalls),
                        m.target_wait_us.load(Ordering::Relaxed),
                        m.target_wait_calls.load(Ordering::Relaxed),
                    ),
                    None => (0.0, 0.0, 0, 0, 0.0, 0.0, 0, 0.0, 0, 0),
                };
                let avg_wait_ms = if wait_calls > metrics_last.target_wait_calls {
                    (wait_us - metrics_last.target_wait_us) as f32
                        / (wait_calls - metrics_last.target_wait_calls) as f32
                        / 1000.0
                } else {
                    0.0
                };
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

                let jitter_ms = video_rtp.current_jitter_ms();
                let _ = event_tx.send(PeerEvent::Status(format!(
                    "fps:{fps:.0} src:{src_rate:.0} sub:{sub:.0} qf:{qfull:.0} dec:{avg_decode_ms:.1}ms wait:{avg_wait_ms:.1}ms jit:{jitter_ms:.1}ms nof:{noframe:.0} err:{errs:.0} reb:{rebuilds} stall:{stalls:.0} rtp:{rtp_rate:.0} drop:{drop_rate:.0} pli:{pli_sent_count} wfk:{} in:{} pr:{}",
                    u8::from(video_rtp.waiting_for_keyframe()),
                    u8::from(input_ready),
                    u8::from(partial_input_ready)
                )));

                if frames == 0 {
                    let _ = event_tx.send(PeerEvent::Status(format!(
                        "IN s:{in_stun} d:{in_dtls} m:{in_media} | OUT s:{out_stun} d:{out_dtls} m:{out_media} | RTP:{rtp_packets} AU:{access_units_sent}"
                    )));
                }

                if fps == 0.0 {
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
                            keyframe_requests.fetch_add(1, Ordering::Relaxed);
                        keyframe_requests.fetch_add(1, Ordering::Relaxed);
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
                        _ => {
                            in_media += 1;
                            media_bytes.fetch_add(n as u64, Ordering::Relaxed);
                        }
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

        while let Ok((n, peer_addr)) = socket.try_recv_from(&mut buf) {
            match classify(buf.first()) {
                0 => in_stun += 1,
                1 => in_dtls += 1,
                _ => {
                    in_media += 1;
                    media_bytes.fetch_add(n as u64, Ordering::Relaxed);
                }
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

        let mut latest_gamepad = None;
        let mut mouse_events = Vec::new();
        let mut key_events: Vec<(KeyStroke, bool)> = Vec::new();
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
                PeerCommand::Mouse(event) => mouse_events.push(event),
                PeerCommand::Key { key, pressed } => key_events.push((key, pressed)),
                PeerCommand::SetMaxBitrate(kbps) => {
                    saved_answer_sdp = crate::gfn::sdp::replace_video_bitrate_in_sdp(&saved_answer_sdp, kbps);
                }
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
        // Mouse goes over the reliable channel, matching GFN's own clients: only gamepad state
        // is eligible for the partially reliable one. A dropped button-up would leave the host
        // holding the mouse down, which is exactly the packet you cannot afford to lose.
        if !mouse_events.is_empty()
            && input_ready
            && let Some(id) = input_channel_id
            && let Some(mut channel) = pc.data_channel(id)
        {
            for event in mouse_events.drain(..) {
                let timestamp_us = session_clock.elapsed().as_micros() as u64;
                let packet = match event {
                    MouseEvent::MoveBy { dx, dy } => {
                        input_encoder.encode_mouse_move(dx, dy, timestamp_us)
                    }
                    MouseEvent::Button { button, pressed } => {
                        input_encoder.encode_mouse_button(button, pressed, timestamp_us)
                    }
                };
                let _ = channel.send(BytesMut::from(&packet[..]));
            }
        }
        // Keys go over the reliable channel for the same reason as mouse buttons: losing a
        // key-up leaves the host holding the key down, which in a game is worse than a lost press.
        if !key_events.is_empty()
            && input_ready
            && let Some(id) = input_channel_id
            && let Some(mut channel) = pc.data_channel(id)
        {
            for (key, pressed) in key_events.drain(..) {
                let timestamp_us = session_clock.elapsed().as_micros() as u64;
                let packet = input_encoder.encode_key(key, pressed, timestamp_us);
                let _ = channel.send(BytesMut::from(&packet[..]));
            }
        }
    }
}
