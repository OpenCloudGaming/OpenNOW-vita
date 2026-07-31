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

// a=ri.* stuff from the offer, echoed back in the nvst sdp. mirrors OpenNOW-Switch's ParseRiInputCapabilities
pub struct RiInputCapabilities {
    // sctp maxPacketLifeTime for the partial reliable input channel
    pub partial_reliable_threshold_ms: u16,
    pub hid_device_mask: u32,
    pub partial_reliable_gamepad_mask: u32,
    pub partial_reliable_hid_mask: u32,
}

impl Default for RiInputCapabilities {
    // same fallbacks as the desktop client, not the switch ones (16ms is too aggressive for vita wifi)
    fn default() -> Self {
        Self {
            partial_reliable_threshold_ms: 300,
            hid_device_mask: 0xffff_ffff,
            partial_reliable_gamepad_mask: 0x0f,
            partial_reliable_hid_mask: 0xffff_ffff,
        }
    }
}

// reads a=ri.* from the offer, accepts decimal or 0x hex like the ref client does
pub fn parse_ri_input_capabilities(sdp: &str) -> RiInputCapabilities {
    let mut caps = RiInputCapabilities::default();
    if let Some(threshold) = parse_ri_integer_attribute(sdp, "ri.partialReliableThresholdMs") {
        if threshold > 0 {
            caps.partial_reliable_threshold_ms = threshold.clamp(1, 5000) as u16;
        }
    }
    if let Some(mask) = parse_ri_integer_attribute(sdp, "ri.hidDeviceMask") {
        caps.hid_device_mask = mask as u32;
    }
    if let Some(mask) = parse_ri_integer_attribute(sdp, "ri.enablePartiallyReliableTransferGamepad")
    {
        caps.partial_reliable_gamepad_mask = mask as u32;
    }
    if let Some(mask) = parse_ri_integer_attribute(sdp, "ri.enablePartiallyReliableTransferHid") {
        caps.partial_reliable_hid_mask = mask as u32;
    }
    caps
}

fn parse_ri_integer_attribute(sdp: &str, attribute: &str) -> Option<i64> {
    let prefix = format!("a={attribute}:");
    let raw = sdp
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(&prefix))?
        .trim();
    if let Some(hex) = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
    {
        i64::from_str_radix(hex, 16).ok()
    } else {
        raw.parse().ok()
    }
}

/// The `x-nv-sdpver` NVST parameter blob sent next to the standard answer.
pub fn build_nvst_sdp_from_answer(
    answer_sdp: &str,
    settings: &crate::gfn::cloudmatch::StreamSettings,
    ri: &RiInputCapabilities,
) -> String {
    let ours = extract_ice_credentials(answer_sdp);
    let (width, height) = settings.dimensions();
    let max_bitrate_kbps = settings.max_bitrate_mbps * 1000;
    // The floor decides *which axis* a weak link degrades on. GFN's bandwidth estimation and DRC
    // are both already enabled below, so the stream does adapt - but a floor it cannot go under
    // leaves dropping the encode resolution as the only way to shed load, which is the blurry
    // picture. Floored low (was 35% / 5 Mbps, which a Vita's 2.4 GHz-only radio often cannot
    // hold) so congestion control spends bitrate before it spends pixels.
    let min_bitrate_kbps = 4_000.max(max_bitrate_kbps * 25 / 100);
    // start at 75% of the ceiling, looks crisp right away instead of ramping up
    let initial_bitrate_kbps = min_bitrate_kbps.max(max_bitrate_kbps * 75 / 100);
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
        m=audio 0 RTP/AVP\r\n\
        a=msid:audio\r\n\
        m=application 0 RTP/AVP\r\n\
        a=msid:input_1\r\n\
        a=ri.partialReliableThresholdMs:{ri_threshold}\r\n\
        a=ri.hidDeviceMask:{ri_hid_mask}\r\n\
        a=ri.enablePartiallyReliableTransferGamepad:{ri_gamepad_mask}\r\n\
        a=ri.enablePartiallyReliableTransferHid:{ri_pr_hid_mask}\r\n",
        pwd = ours.pwd,
        ufrag = ours.ufrag,
        fingerprint = ours.fingerprint,
        fps = settings.fps,
        ri_threshold = ri.partial_reliable_threshold_ms,
        ri_hid_mask = ri.hid_device_mask,
        ri_gamepad_mask = ri.partial_reliable_gamepad_mask,
        ri_pr_hid_mask = ri.partial_reliable_hid_mask,
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

// sets b=AS bitrate + forces good opus params (stereo, minptime) on the answer sdp
pub fn munge_answer_sdp(sdp: &str, kbps: u32) -> String {
    let ending = line_ending(sdp);
    let mut lines: Vec<String> = Vec::new();
    let mut current_section = "";

    for line in sdp.split(ending) {
        if line.starts_with("m=") {
            if line.starts_with("m=video") {
                current_section = "video";
            } else if line.starts_with("m=audio") {
                current_section = "audio";
            } else {
                current_section = "other";
            }
            lines.push(line.to_owned());
            if current_section == "video" {
                lines.push(format!("b=AS:{kbps}"));
            } else if current_section == "audio" {
                lines.push("b=AS:256".to_owned());
            }
            continue;
        }

        if line.starts_with("b=AS:") {
            // drop old b=AS, we push our own right after m= above
            continue;
        }

        if current_section == "audio" && line.starts_with("a=fmtp:") {
            if line.contains("stereo=1") {
                lines.push(line.to_owned());
            } else {
                lines.push(format!("{line};stereo=1;minptime=10"));
            }
            continue;
        }

        lines.push(line.to_owned());
    }

    lines.join(ending)
}

// just swaps the b=AS video bitrate in an sdp thats already been munged once
pub fn replace_video_bitrate_in_sdp(sdp: &str, kbps: u32) -> String {
    let ending = line_ending(sdp);
    let mut lines: Vec<String> = Vec::new();
    let mut in_video = false;
    let mut replaced = false;

    for line in sdp.split(ending) {
        if line.starts_with("m=") {
            in_video = line.starts_with("m=video");
            lines.push(line.to_owned());
            continue;
        }

        if in_video && line.starts_with("b=AS:") {
            lines.push(format!("b=AS:{kbps}"));
            replaced = true;
            continue;
        }

        lines.push(line.to_owned());
    }

    if !replaced {
        // no b=AS under m=video, fall back to munge_answer_sdp to add it
        munge_answer_sdp(sdp, kbps)
    } else {
        lines.join(ending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ri_input_capabilities_parses_hex_and_decimal() {
        let offer = "v=0\r\n\
            m=application 0 RTP/AVP\r\n\
            a=ri.partialReliableThresholdMs:250\r\n\
            a=ri.hidDeviceMask:0xff\r\n\
            a=ri.enablePartiallyReliableTransferGamepad:0x03\r\n\
            a=ri.enablePartiallyReliableTransferHid:15\r\n";
        let caps = parse_ri_input_capabilities(offer);
        assert_eq!(caps.partial_reliable_threshold_ms, 250);
        assert_eq!(caps.hid_device_mask, 0xff);
        assert_eq!(caps.partial_reliable_gamepad_mask, 0x03);
        assert_eq!(caps.partial_reliable_hid_mask, 15);
    }

    #[test]
    fn parse_ri_input_capabilities_clamps_threshold() {
        let offer = "a=ri.partialReliableThresholdMs:10000\r\n";
        let caps = parse_ri_input_capabilities(offer);
        assert_eq!(caps.partial_reliable_threshold_ms, 5000);
    }

    #[test]
    fn munge_answer_sdp_adds_bitrate_and_opus_params() {
        let answer = "v=0\r\n\
            m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
            a=fmtp:111 useinbandfec=1\r\n\
            m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
            a=rtpmap:96 H264/90000\r\n";
        let munged = munge_answer_sdp(answer, 15000);
        assert!(munged.contains("m=video 9 UDP/TLS/RTP/SAVPF 96\r\nb=AS:15000"));
        assert!(munged.contains("m=audio 9 UDP/TLS/RTP/SAVPF 111\r\nb=AS:256"));
        assert!(munged.contains("a=fmtp:111 useinbandfec=1;stereo=1;minptime=10"));
    }

    #[test]
    fn replace_video_bitrate_in_sdp_updates_existing_value() {
        let answer = "v=0\r\n\
            m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
            b=AS:15000\r\n\
            a=rtpmap:96 H264/90000\r\n";
        let updated = replace_video_bitrate_in_sdp(answer, 20000);
        assert!(updated.contains("b=AS:20000"));
        assert!(!updated.contains("b=AS:15000"));
    }
}

