//! Streaming preferences the player controls, persisted between sessions.

const STORE_DIR: &str = "ux0:data/opennow-vita";
const FPS_STORE_PATH: &str = "ux0:data/opennow-vita/stream-fps.txt";
const TRIGGER_STORE_PATH: &str = "ux0:data/opennow-vita/trigger-intensity.txt";
const AUDIO_BOOST_STORE_PATH: &str = "ux0:data/opennow-vita/audio-boost.txt";
const CONTROLS_HINT_STORE_PATH: &str = "ux0:data/opennow-vita/controls-hint-seen.txt";
const STICK_ZONES_STORE_PATH: &str = "ux0:data/opennow-vita/stick-zones.txt";

/// Frame rate to request from GFN.
///
/// This is the sharpest quality lever on a bandwidth-starved link, and the trade is not obvious
/// from the numbers alone. The encoder gets a fixed budget of bits per second, so halving the
/// frame rate doubles the bits available for each frame: on a link delivering ~4.5 Mbit/s at
/// 960x544, 60 fps works out to about 0.14 bits per pixel where 30 fps gets 0.29. Halving the
/// packet rate also halves the exposure to loss on a 2.4 GHz radio.
///
/// Which is better is genuinely a matter of taste and of the game, so it is the player's call
/// rather than a hardcoded constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamFps {
    /// Smoothest motion, least detail per frame.
    #[default]
    Sixty,
    /// Twice the bits per frame; noticeably sharper when bandwidth is the constraint.
    Thirty,
}

impl StreamFps {
    pub const ALL: [StreamFps; 2] = [Self::Sixty, Self::Thirty];

    pub fn value(self) -> u32 {
        match self {
            Self::Sixty => 60,
            Self::Thirty => 30,
        }
    }

    /// Fluent message id for this option's label.

    fn from_value(fps: u32) -> Self {
        match fps {
            30 => Self::Thirty,
            _ => Self::Sixty,
        }
    }
}

pub fn fps() -> StreamFps {
    std::fs::read_to_string(FPS_STORE_PATH)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .map(StreamFps::from_value)
        .unwrap_or_default()
}

pub fn set_fps(fps: StreamFps) {
    if std::fs::create_dir_all(STORE_DIR).is_err() {
        return;
    }
    if let Err(error) = std::fs::write(FPS_STORE_PATH, fps.value().to_string()) {
        eprintln!("Could not persist stream fps: {error}");
    }
}

/// How hard a rear-panel touch presses L2/R2.
///
/// The panel is a digital surface standing in for an analog axis, so it has to pick a value.
/// Full travel is right for anything that treats the trigger as a button (shooting, braking), but
/// games that read the axis - a car's throttle, drawing a bow - become all-or-nothing at 100%.
/// Which one a player wants depends entirely on what they are playing, so it is a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TriggerIntensity {
    #[default]
    Full,
    High,
    Half,
}

impl TriggerIntensity {
    pub const ALL: [TriggerIntensity; 3] = [Self::Full, Self::High, Self::Half];

    /// The 0-255 value reported on the input channel while a half of the panel is held.
    pub fn value(self) -> u8 {
        match self {
            Self::Full => 255,
            Self::High => 192,
            Self::Half => 128,
        }
    }


    fn from_value(value: u8) -> Self {
        match value {
            v if v >= 255 => Self::Full,
            v if v >= 192 => Self::High,
            _ => Self::Half,
        }
    }
}

use std::sync::atomic::{AtomicU8, Ordering};

static CACHED_STICK_ZONES: AtomicU8 = AtomicU8::new(255); // 255 = uninitialized
static CACHED_TRIGGER_INTENSITY: AtomicU8 = AtomicU8::new(255);

pub fn trigger_intensity() -> TriggerIntensity {
    let cached = CACHED_TRIGGER_INTENSITY.load(Ordering::Relaxed);
    if cached != 255 {
        return TriggerIntensity::from_value(cached);
    }
    let val = std::fs::read_to_string(TRIGGER_STORE_PATH)
        .ok()
        .and_then(|text| text.trim().parse::<u8>().ok())
        .map(TriggerIntensity::from_value)
        .unwrap_or_default();
    CACHED_TRIGGER_INTENSITY.store(val.value(), Ordering::Relaxed);
    val
}

pub fn set_trigger_intensity(intensity: TriggerIntensity) {
    CACHED_TRIGGER_INTENSITY.store(intensity.value(), Ordering::Relaxed);
    if std::fs::create_dir_all(STORE_DIR).is_err() {
        return;
    }
    if let Err(error) = std::fs::write(TRIGGER_STORE_PATH, intensity.value().to_string()) {
        eprintln!("Could not persist trigger intensity: {error}");
    }
}

pub fn stick_zones() -> StickZones {
    let cached = CACHED_STICK_ZONES.load(Ordering::Relaxed);
    if cached != 255 {
        return match cached {
            0 => StickZones::Off,
            1 => StickZones::Hidden,
            _ => StickZones::Visible,
        };
    }
    let val = std::fs::read_to_string(STICK_ZONES_STORE_PATH)
        .map(|text| StickZones::from_text(&text))
        .unwrap_or_default();
    let code = match val {
        StickZones::Off => 0,
        StickZones::Hidden => 1,
        StickZones::Visible => 2,
    };
    CACHED_STICK_ZONES.store(code, Ordering::Relaxed);
    val
}

pub fn set_stick_zones(zones: StickZones) {
    let code = match zones {
        StickZones::Off => 0,
        StickZones::Hidden => 1,
        StickZones::Visible => 2,
    };
    CACHED_STICK_ZONES.store(code, Ordering::Relaxed);
    if std::fs::create_dir_all(STORE_DIR).is_err() {
        return;
    }
    if let Err(error) = std::fs::write(STICK_ZONES_STORE_PATH, zones.as_text()) {
        eprintln!("Could not persist stick zone setting: {error}");
    }
}

/// How much the decoded stream is amplified, in percent of unity gain.
///
/// GFN delivers noticeably quieter audio than a local GameStream host, and there is no headroom
/// left in hardware to make up for it: SDL's Vita backend already opens the port at
/// `SCE_AUDIO_MAX_VOLUME` (0 dB), and Moonlight's own Vita port never touches the volume at all.
/// The only remaining place to get loudness is gain above unity on the decoded PCM, which is what
/// OpenNOW-Switch settled on too (its default is 12x, exposed as "Volume boost").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioBoost {
    Off,
    Low,
    #[default]
    Normal,
    High,
    Max,
}

impl AudioBoost {
    pub const ALL: [AudioBoost; 5] = [Self::Off, Self::Low, Self::Normal, Self::High, Self::Max];

    /// Requested gain in percent - 100 is unity, 1200 is OpenNOW-Switch's default.
    pub fn percent(self) -> u16 {
        match self {
            Self::Off => 100,
            Self::Low => 800,
            Self::Normal => 1200,
            Self::High => 1400,
            Self::Max => 1600,
        }
    }


    fn from_percent(percent: u16) -> Self {
        match percent {
            p if p >= 1600 => Self::Max,
            p if p >= 1400 => Self::High,
            p if p >= 1200 => Self::Normal,
            p if p >= 800 => Self::Low,
            _ => Self::Off,
        }
    }
}

pub fn audio_boost() -> AudioBoost {
    std::fs::read_to_string(AUDIO_BOOST_STORE_PATH)
        .ok()
        .and_then(|text| text.trim().parse::<u16>().ok())
        .map(AudioBoost::from_percent)
        .unwrap_or_default()
}

pub fn set_audio_boost(boost: AudioBoost) {
    if std::fs::create_dir_all(STORE_DIR).is_err() {
        return;
    }
    if let Err(error) = std::fs::write(AUDIO_BOOST_STORE_PATH, boost.percent().to_string()) {
        eprintln!("Could not persist audio boost: {error}");
    }
}

/// Whether the player has already been shown how the rear panel maps to the missing buttons.
///
/// The Vita has no L2/R2/L3/R3 and no mouse, so those are all improvised on the touch panels.
/// Nothing on the hardware hints at that, which makes it the one thing worth explaining once.
pub fn controls_hint_seen() -> bool {
    std::fs::metadata(CONTROLS_HINT_STORE_PATH).is_ok()
}

pub fn mark_controls_hint_seen() {
    if std::fs::create_dir_all(STORE_DIR).is_err() {
        return;
    }
    if let Err(error) = std::fs::write(CONTROLS_HINT_STORE_PATH, "1") {
        eprintln!("Could not persist the controls hint flag: {error}");
    }
}

/// Whether the front screen's bottom corners act as L3/R3, and whether they are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StickZones {
    /// The whole screen is mouse, as it was before the zones existed.
    Off,
    /// Active but not drawn - they would otherwise cover part of the game for good.
    #[default]
    Hidden,
    /// Drawn translucent, for learning where they fall.
    Visible,
}

impl StickZones {
    pub const ALL: [StickZones; 3] = [Self::Off, Self::Hidden, Self::Visible];

    pub fn is_active(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub fn is_visible(self) -> bool {
        matches!(self, Self::Visible)
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Off => "settings-stick-zones-off",
            Self::Hidden => "settings-stick-zones-hidden",
            Self::Visible => "settings-stick-zones-visible",
        }
    }

    fn from_text(text: &str) -> Self {
        match text.trim() {
            "off" => Self::Off,
            "visible" => Self::Visible,
            _ => Self::Hidden,
        }
    }

    /// Short tag for diagnostics, distinct from `label_key` (which is a translated UI string).
    pub fn debug_label(self) -> &'static str {
        self.as_text()
    }

    fn as_text(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Hidden => "hidden",
            Self::Visible => "visible",
        }
    }
}
