//! Remembers what this network actually delivered, so the next session asks for a ceiling the
//! link has been seen to reach rather than a hardcoded guess.
//!
//! Within a session GFN's own bandwidth estimation does the adapting; this only sets the upper
//! bound it is allowed to climb to. Asking for far more than the link can carry is not free -
//! the stream spends the opening seconds losing packets and dropping resolution before
//! estimation settles.

use std::time::{Duration, Instant};

const STORE_PATH: &str = "ux0:data/opennow-vita/link-bitrate.txt";
const STORE_DIR: &str = "ux0:data/opennow-vita";

/// Never ask for less than this: below it the picture is unusable anyway, so a bad measurement
/// should not be able to strand the client on a permanently terrible stream.
pub const MIN_CEILING_MBPS: u32 = 5;
pub const MAX_CEILING_MBPS: u32 = 12;
pub const DEFAULT_CEILING_MBPS: u32 = 8;

/// Ignore the opening moments: the stream ramps up, so an early sample understates the link.
const WARMUP: Duration = Duration::from_secs(5);
/// How often throughput is sampled.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// How many one-second samples a window averages over. A single second can be lucky - a burst
/// after a stall, or a scene change - so capacity is judged on what the link held for a while.
const WINDOW_SAMPLES: usize = 5;

/// Rolling throughput measurement for one streaming session.
///
/// Deliberately tracks the best *sustained* window rather than the mean. Video bitrate follows
/// scene complexity, so a quiet menu screen produces a low sample that says nothing about the
/// link - averaging those in would measure how busy the game was, not what the connection can
/// carry. The high-water mark of a multi-second window is the closest thing to capacity that
/// passive observation can give.
pub struct LinkMeter {
    started: Instant,
    last_sample: Instant,
    last_bytes: u64,
    window: [u32; WINDOW_SAMPLES],
    filled: usize,
    next_slot: usize,
    peak_sustained_kbps: u32,
}

impl LinkMeter {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last_sample: now,
            last_bytes: 0,
            window: [0; WINDOW_SAMPLES],
            filled: 0,
            next_slot: 0,
            peak_sustained_kbps: 0,
        }
    }

    /// Feeds the peer's cumulative inbound byte count. Call once per frame; sampling is rate
    /// limited internally.
    pub fn sample(&mut self, cumulative_bytes: u64) {
        let now = Instant::now();
        if now.duration_since(self.last_sample) < SAMPLE_INTERVAL {
            return;
        }
        let elapsed = now.duration_since(self.last_sample).as_secs_f64();
        let delta = cumulative_bytes.saturating_sub(self.last_bytes);
        self.last_sample = now;
        self.last_bytes = cumulative_bytes;

        if now.duration_since(self.started) < WARMUP || elapsed <= 0.0 {
            return;
        }
        let kbps = ((delta as f64 * 8.0) / elapsed / 1000.0) as u32;

        self.window[self.next_slot] = kbps;
        self.next_slot = (self.next_slot + 1) % WINDOW_SAMPLES;
        self.filled = (self.filled + 1).min(WINDOW_SAMPLES);

        // Only judge once the window is genuinely full, so a short session cannot report a
        // sustained rate it never sustained.
        if self.filled == WINDOW_SAMPLES {
            let average = self.window.iter().map(|&v| v as u64).sum::<u64>() / WINDOW_SAMPLES as u64;
            self.peak_sustained_kbps = self.peak_sustained_kbps.max(average as u32);
        }
    }

    /// How long this session has been running, for judging whether its fault count is a lot.
    pub fn elapsed_secs(&self) -> u32 {
        self.started.elapsed().as_secs() as u32
    }

    /// The best sustained throughput seen, in Mbps, or `None` if the session was too short to
    /// measure anything trustworthy.
    pub fn peak_mbps(&self) -> Option<u32> {
        (self.peak_sustained_kbps > 0).then_some(self.peak_sustained_kbps.div_ceil(1000))
    }
}

impl Default for LinkMeter {
    fn default() -> Self {
        Self::new()
    }
}

/// Folds a session into the ceiling future sessions request.
///
/// The rule that matters: **throughput alone never lowers the ceiling.** What we can measure is
/// what the server *chose to send*, which tracks scene complexity - a quiet game or a session spent
/// in menus produces a low number that says nothing about the link. Feeding that back as capacity
/// ratcheted quality down a step every session (15 -> 10 -> 8 -> 7 ...) until it sat on the floor
/// and could never climb back, because discovering more capacity would have required asking for
/// more. That is why the picture was soft on a connection that was doing fine.
///
/// So the ceiling only comes down on evidence of actual strain - damaged frames forcing keyframe
/// requests - and otherwise creeps up toward what the hardware can use.
pub fn record(measured_mbps: u32, stressed: bool) {
    let previous = stored_ceiling().unwrap_or(DEFAULT_CEILING_MBPS);
    let next = if stressed {
        // Back off toward what the link actually carried, but only part of the way: one bad
        // session should not erase everything learned on good ones.
        let observed = measured_mbps + measured_mbps / 4;
        (previous + observed.min(previous)).div_ceil(2)
    } else {
        // No strain, so the ceiling was not the thing holding quality back - offer more and let
        // NVIDIA's own bandwidth estimation decide whether to use it.
        previous + previous / 4 + 1
    }
    .clamp(MIN_CEILING_MBPS, MAX_CEILING_MBPS);

    if std::fs::create_dir_all(STORE_DIR).is_err() {
        return;
    }
    if let Err(error) = std::fs::write(STORE_PATH, next.to_string()) {
        eprintln!("Could not persist link estimate: {error}");
    }
}

fn stored_ceiling() -> Option<u32> {
    std::fs::read_to_string(STORE_PATH)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
}

/// The ceiling to request, from the last measured session or the default.
pub fn ceiling_mbps() -> u32 {
    let Some(raw) = stored_ceiling() else {
        return DEFAULT_CEILING_MBPS;
    };
    let clamped = raw.clamp(MIN_CEILING_MBPS, MAX_CEILING_MBPS);
    if clamped != raw {
        let _ = std::fs::create_dir_all(STORE_DIR);
        let _ = std::fs::write(STORE_PATH, clamped.to_string());
    }
    clamped
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pure half of `record`, so the ratchet can be reasoned about without a memory card.
    fn next_ceiling(previous: u32, measured_mbps: u32, stressed: bool) -> u32 {
        if stressed {
            let observed = measured_mbps + measured_mbps / 4;
            (previous + observed.min(previous)).div_ceil(2)
        } else {
            previous + previous / 4 + 1
        }
        .clamp(MIN_CEILING_MBPS, MAX_CEILING_MBPS)
    }

    /// The regression this replaces: a quiet session used to drag the ceiling down, and repeating
    /// that walked it to the floor where it stayed.
    #[test]
    fn a_quiet_session_no_longer_lowers_the_ceiling() {
        let mut ceiling = DEFAULT_CEILING_MBPS;
        for _ in 0..10 {
            // Barely any traffic, but nothing went wrong.
            ceiling = next_ceiling(ceiling, 3, false);
        }
        assert!(
            ceiling >= DEFAULT_CEILING_MBPS,
            "an unstressed link must never ratchet down, got {ceiling}"
        );
    }

    #[test]
    fn an_unstressed_link_climbs_toward_the_maximum() {
        let mut ceiling = MIN_CEILING_MBPS;
        for _ in 0..40 {
            ceiling = next_ceiling(ceiling, 5, false);
        }
        assert_eq!(ceiling, MAX_CEILING_MBPS);
    }

    #[test]
    fn strain_backs_the_ceiling_off() {
        let lowered = next_ceiling(20, 8, true);
        assert!(lowered < 20, "a struggling link should give ground");
        assert!(
            lowered >= MIN_CEILING_MBPS,
            "but never below the floor, got {lowered}"
        );
    }

    /// Even repeated strain settles rather than collapsing to nothing.
    #[test]
    fn repeated_strain_settles_at_the_floor_not_below() {
        let mut ceiling = MAX_CEILING_MBPS;
        for _ in 0..20 {
            ceiling = next_ceiling(ceiling, 1, true);
        }
        assert_eq!(ceiling, MIN_CEILING_MBPS);
    }
}
