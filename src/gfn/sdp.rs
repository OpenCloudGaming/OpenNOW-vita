//! SDP utilities - sanitizes NVIDIA's NVST-flavored offer for the `rtc` parser, and builds the
//! NVST answer blob the signaling channel wants alongside the standard WebRTC answer.

/// ICE/DTLS credentials of one side of the session, pulled out of its SDP.
pub struct IceCredentials {
    pub ufrag: String,
    pub pwd: String,
    pub fingerprint: String,
}

pub fn extract_ice_credentials(sdp: &str) -> IceCredentials {
    let mut credentials = IceCredentials {
        ufrag: String::new(),
        pwd: String::new(),
        fingerprint: String::new(),
    };
    for line in sdp.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("a=ice-ufrag:") {
            if credentials.ufrag.is_empty() {
                credentials.ufrag = val.to_string();
            }
        } else if let Some(val) = line.strip_prefix("a=ice-pwd:") {
            if credentials.pwd.is_empty() {
                credentials.pwd = val.to_string();
            }
        } else if let Some(val) = line.strip_prefix("a=fingerprint:") {
            if credentials.fingerprint.is_empty() {
                credentials.fingerprint = val.to_string();
            }
        }
    }
    credentials
}

/// The `x-nv-sdpver` NVST parameter blob sent next to the standard answer.
pub fn build_nvst_sdp_from_answer(
    answer_sdp: &str,
    settings: &crate::gfn::cloudmatch::StreamSettings,
) -> String {
    let ours = extract_ice_credentials(answer_sdp);
    let (width, height) = settings.dimensions();
    let max_bitrate_kbps = settings.max_bitrate_mbps * 1000;
    // The floor decides *which axis* a weak link degrades on. GFN's bandwidth estimation and DRC
    // are both already enabled below, so the stream does adapt - but a floor it cannot go under
    // leaves dropping the encode resolution as the only way to shed load, which is the blurry
    // picture. Floored low (was 35% / 5 Mbps, which a Vita's 2.4 GHz-only radio often cannot
    // hold) so congestion control spends bitrate before it spends pixels.
    let min_bitrate_kbps = 1_500.max(max_bitrate_kbps * 20 / 100);
    // Opening at 70% of the ceiling means a link that cannot take it starts by losing packets.
    // Starting nearer the ceiling lets estimation hold high quality instead of slowly ramping up.
    let initial_bitrate_kbps = min_bitrate_kbps.max(max_bitrate_kbps * 70 / 100);
    let max_reference_frames = crate::streaming::video::AVCDEC_NUM_REF_FRAMES;
    // Resolution is pinned rather than left to the server's dynamic resolution control. The panel
    // is exactly 960x544 and the stream is negotiated at exactly that, so a DRC drop means the
    // server encodes smaller and something upscales it back - the soft, washed-out picture. With
    // DRC/GRC/cpmRtc off and `minResolutionPercent:100`, a starved link spends quality on
    // compression artefacts at native resolution instead. GFN's own Switch client, facing the same
    // fixed-panel situation, makes the same choice.
    //
    // NACK was switched on but never sized, so retransmission had whatever the server defaults to
    // - likely far too small to cover a 2.4 GHz link's loss. Recovering a packet costs a round
    // trip; losing it costs macroblocks that smear until the next keyframe. The queue values here
    // are the ones GFN's own Switch client negotiates.
    //
    // Pacing matters for the same reason: spreading a frame's packets over several groups instead
    // of bursting them is what stops a weak radio from dropping the tail of every large frame.
    format!(
        "v=0\r\n\
        o=SdpTest test_id_13 14 IN IPv4 127.0.0.1\r\n\
        s=-\r\n\
        t=0 0\r\n\
        a=general.icePassword:{pwd}\r\n\
        a=general.iceUserNameFragment:{ufrag}\r\n\
        a=general.dtlsFingerprint:{fingerprint}\r\n\
        m=video 0 RTP/AVP\r\n\
        a=msid:fbc-video-0\r\n\
        a=vqos.fec.rateDropWindow:10\r\n\
        a=vqos.fec.minRequiredFecPackets:2\r\n\
        a=vqos.drc.minRequiredBitrateCheckEnabled:1\r\n\
        a=vqos.fec.repairMinPercent:6\r\n\
        a=vqos.fec.repairPercent:8\r\n\
        a=vqos.fec.repairMaxPercent:30\r\n\
        a=vqos.dynamicStreamingMode:0\r\n\
        a=vqos.drc.enable:0\r\n\
        a=vqos.dfc.enable:0\r\n\
        a=vqos.dfc.adjustResAndFps:0\r\n\
        a=vqos.resControl.cpmRtc.enable:0\r\n\
        a=vqos.resControl.cpmRtc.featureMask:0\r\n\
        a=vqos.resControl.cpmRtc.minResolutionPercent:100\r\n\
        a=vqos.resControl.cpmRtc.resolutionChangeHoldonMs:999999\r\n\
        a=vqos.drc.qpMaxResThresholdAdj:4\r\n\
        a=vqos.grc.qpMaxResThresholdAdj:4\r\n\
        a=vqos.drc.iirFilterFactor:100\r\n\
        a=vqos.grc.enable:0\r\n\
        a=vqos.grc.maximumBitrateKbps:{max_bitrate_kbps}\r\n\
        a=vqos.adjustStreamingFpsDuringOutOfFocus:1\r\n\
        a=vqos.resControl.cpmRtc.ignoreOutOfFocusWindowState:1\r\n\
        a=vqos.resControl.perfHistory.rtcIgnoreOutOfFocusWindowState:1\r\n\
        a=video.dx9EnableNv12:1\r\n\
        a=video.dx9EnableHdr:1\r\n\
        a=vqos.qpg.enable:1\r\n\
        a=vqos.resControl.qp.qpg.featureSetting:7\r\n\
        a=bwe.useOwdCongestionControl:1\r\n\
        a=video.enableRtpNack:1\r\n\
        a=vqos.bw.txRxLag.minFeedbackTxDeltaMs:200\r\n\
        a=vqos.drc.bitrateIirFilterFactor:18\r\n\
        a=video.packetSize:1140\r\n\
        a=video.rtpNackQueueLength:1024\r\n\
        a=video.rtpNackQueueMaxPackets:512\r\n\
        a=video.rtpNackMaxPacketCount:25\r\n\
        a=packetPacing.numGroups:6\r\n\
        a=packetPacing.minNumPacketsPerGroup:10\r\n\
        a=packetPacing.minNumPacketsFrame:10\r\n\
        a=packetPacing.maxDelayUs:1250\r\n\
        a=video.mapRtpTimestampsToFrames:1\r\n\
        a=video.encoderCscMode:3\r\n\
        a=video.dynamicRangeMode:0\r\n\
        a=video.bitDepth:8\r\n\
        a=video.scalingFeature1:0\r\n\
        a=video.prefilterParams.prefilterModel:0\r\n\
        a=vqos.bllFec.enable:0\r\n\
        a=video.clientViewportWd:{width}\r\n\
        a=video.clientViewportHt:{height}\r\n\
        a=video.maxFPS:{fps}\r\n\
        a=video.maxNumReferenceFrames:{max_reference_frames}\r\n\
        a=video.initialBitrateKbps:{initial_bitrate_kbps}\r\n\
        a=video.initialPeakBitrateKbps:{max_bitrate_kbps}\r\n\
        a=vqos.bw.maximumBitrateKbps:{max_bitrate_kbps}\r\n\
        a=vqos.bw.minimumBitrateKbps:{min_bitrate_kbps}\r\n\
        a=vqos.bw.peakBitrateKbps:{max_bitrate_kbps}\r\n\
        a=vqos.bw.serverPeakBitrateKbps:{max_bitrate_kbps}\r\n\
        a=vqos.bw.enableBandwidthEstimation:1\r\n\
        a=vqos.bw.disableBitrateLimit:0\r\n\
        m=application 0 RTP/AVP\r\n\
        a=msid:input_1\r\n",
        pwd = ours.pwd,
        ufrag = ours.ufrag,
        fingerprint = ours.fingerprint,
        fps = settings.fps,
    )
}

/// A literal `a.b.c.d`, or an Alliance-style host whose first DNS label encodes one
/// (`62-210-1-2.host...`).
pub fn extract_public_ip(host_or_ip: &str) -> Option<String> {
    if host_or_ip.is_empty() {
        return None;
    }
    if host_or_ip.parse::<std::net::Ipv4Addr>().is_ok() {
        return Some(host_or_ip.to_owned());
    }
    let first_label = host_or_ip.split('.').next().unwrap_or_default();
    let parts: Vec<&str> = first_label.split('-').collect();
    if parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty() && part.len() <= 3 && part.as_bytes().iter().all(u8::is_ascii_digit)
        })
    {
        return Some(parts.join("."));
    }
    None
}

/// NVIDIA's offer advertises `0.0.0.0` in its connection line and candidates; substitute the real
/// server address so ICE has something to connect to.
pub fn fix_server_ip(sdp: &str, server_ip: &str) -> String {
    let Some(ip) = extract_public_ip(server_ip) else {
        return sdp.to_owned();
    };

    let ending = line_ending(sdp);
    sdp.split(ending)
        .map(|line| {
            if line.starts_with("c=IN IP4 0.0.0.0") {
                line.replacen("0.0.0.0", &ip, 1)
            } else if line.starts_with("a=candidate:") {
                let mut parts: Vec<&str> = line.split(' ').collect();
                if parts.len() >= 6 && parts[4] == "0.0.0.0" {
                    parts[4] = &ip;
                    parts.join(" ")
                } else {
                    line.to_owned()
                }
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(ending)
}

/// NVST offers carry `a=ice-ufrag`/`a=ice-pwd`/`a=fingerprint`/`a=setup` only at session level;
/// standard WebRTC parsers expect them per media section.
pub fn duplicate_session_attributes_to_media(sdp: &str) -> String {
    let ending = line_ending(sdp);
    let lines: Vec<&str> = sdp.split(ending).collect();
    let Some(first_media_index) = lines.iter().position(|line| line.starts_with("m=")) else {
        return sdp.to_owned();
    };

    let is_shared_attribute = |line: &str| {
        line.starts_with("a=ice-ufrag:")
            || line.starts_with("a=ice-pwd:")
            || line.starts_with("a=fingerprint:")
            || line.starts_with("a=setup:")
    };
    let session_attributes: Vec<&str> = lines[..first_media_index]
        .iter()
        .copied()
        .filter(|line| is_shared_attribute(line))
        .collect();
    if session_attributes.is_empty() {
        return sdp.to_owned();
    }

    let mut output: Vec<String> = Vec::with_capacity(lines.len() + session_attributes.len() * 2);
    let section_has_attributes = |start: usize| {
        lines[start + 1..]
            .iter()
            .take_while(|line| !line.starts_with("m="))
            .any(|line| is_shared_attribute(line))
    };

    for (index, line) in lines.iter().enumerate() {
        output.push((*line).to_owned());
        if line.starts_with("m=") && !section_has_attributes(index) {
            for attribute in &session_attributes {
                output.push((*attribute).to_owned());
            }
        }
    }

    output.join(ending)
}

/// Full offer sanitation pipeline applied before handing NVIDIA's SDP to the `rtc` parser.
pub fn sanitize_offer(offer_sdp: &str, server_ip: &str) -> String {
    let fixed = fix_server_ip(offer_sdp, server_ip);
    duplicate_session_attributes_to_media(&fixed)
}

/// Payload types the offer maps to H264 (`a=rtpmap:<pt> H264/90000`).
pub fn h264_payload_types(sdp: &str) -> Vec<u8> {
    rtpmap_payload_types(sdp, "H264/")
}

/// Payload types the offer maps to Opus (`a=rtpmap:<pt> opus/48000/2`), including any RED
/// (`a=rtpmap:<pt> red/48000/2`) types that wrap it.
///
/// RED has to be in here: NVST uses RFC 2198 redundancy as its audio FEC, and a stream negotiated
/// on the RED payload type would otherwise be discarded wholesale as "not audio".
pub fn opus_payload_types(sdp: &str) -> Vec<u8> {
    let mut types = rtpmap_payload_types(sdp, "OPUS/");
    types.extend(rtpmap_payload_types(sdp, "RED/"));
    types
}

fn rtpmap_payload_types(sdp: &str, codec_prefix: &str) -> Vec<u8> {
    sdp.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("a=rtpmap:")?;
            let (pt, codec) = rest.split_once(' ')?;
            if codec.to_ascii_uppercase().starts_with(codec_prefix) {
                pt.parse().ok()
            } else {
                None
            }
        })
        .collect()
}

fn line_ending(sdp: &str) -> &'static str {
    if sdp.contains("\r\n") { "\r\n" } else { "\n" }
}
