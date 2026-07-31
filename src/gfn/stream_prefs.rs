use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

const STORE_DIR: &str = "ux0:data/opennow-vita";
const SETTINGS_JSON_PATH: &str = "ux0:data/opennow-vita/settings.json";

// Old paths for one-time legacy migration
const FPS_STORE_PATH: &str = "ux0:data/opennow-vita/stream-fps.txt";
const TRIGGER_STORE_PATH: &str = "ux0:data/opennow-vita/trigger-intensity.txt";
const AUDIO_BOOST_STORE_PATH: &str = "ux0:data/opennow-vita/audio-boost.txt";
const CONTROLS_HINT_STORE_PATH: &str = "ux0:data/opennow-vita/controls-hint-seen.txt";
const STICK_ZONES_STORE_PATH: &str = "ux0:data/opennow-vita/stick-zones.txt";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub fps: u32,
    pub trigger_intensity: u8,
    pub audio_boost_percent: u16,
    pub controls_hint_seen: bool,
    pub stick_zones: String,
    #[serde(default = "default_catalog_sort")]
    pub catalog_sort: String,
    #[serde(default = "default_rear_touch_mode")]
    pub rear_touch_mode: String,
    #[serde(default = "default_catalog_filter")]
    pub catalog_filter: String,
}

fn default_catalog_sort() -> String {
    "last_played".to_owned()
}

fn default_catalog_filter() -> String {
    "my_games".to_owned()
}

fn default_rear_touch_mode() -> String {
    "quadrant".to_owned()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            fps: 60,
            trigger_intensity: 255,
            audio_boost_percent: 1200,
            controls_hint_seen: false,
            stick_zones: "hidden".to_owned(),
            catalog_sort: "last_played".to_owned(),
            rear_touch_mode: "quadrant".to_owned(),
            catalog_filter: "my_games".to_owned(),
        }
    }
}

static CACHED_SETTINGS: Mutex<Option<AppSettings>> = Mutex::new(None);

fn load_or_init_settings() -> AppSettings {
    let mut guard = CACHED_SETTINGS.lock().unwrap();
    if let Some(ref settings) = *guard {
        return settings.clone();
    }

    // Try reading settings.json
    if let Ok(content) = std::fs::read_to_string(SETTINGS_JSON_PATH) {
        if let Ok(settings) = serde_json::from_str::<AppSettings>(&content) {
            *guard = Some(settings.clone());
            return settings;
        }
    }

    // One-time migration from legacy .txt files if settings.json does not exist
    let mut settings = AppSettings::default();

    if let Ok(text) = std::fs::read_to_string(FPS_STORE_PATH) {
        if let Ok(val) = text.trim().parse::<u32>() {
            settings.fps = val;
        }
        let _ = std::fs::remove_file(FPS_STORE_PATH);
    }
    if let Ok(text) = std::fs::read_to_string(TRIGGER_STORE_PATH) {
        if let Ok(val) = text.trim().parse::<u8>() {
            settings.trigger_intensity = val;
        }
        let _ = std::fs::remove_file(TRIGGER_STORE_PATH);
    }
    if let Ok(text) = std::fs::read_to_string(AUDIO_BOOST_STORE_PATH) {
        if let Ok(val) = text.trim().parse::<u16>() {
            settings.audio_boost_percent = val;
        }
        let _ = std::fs::remove_file(AUDIO_BOOST_STORE_PATH);
    }
    if std::fs::metadata(CONTROLS_HINT_STORE_PATH).is_ok() {
        settings.controls_hint_seen = true;
        let _ = std::fs::remove_file(CONTROLS_HINT_STORE_PATH);
    }
    if let Ok(text) = std::fs::read_to_string(STICK_ZONES_STORE_PATH) {
        settings.stick_zones = text.trim().to_owned();
        let _ = std::fs::remove_file(STICK_ZONES_STORE_PATH);
    }

    // Persist new settings.json
    save_settings_disk(&settings);
    *guard = Some(settings.clone());
    settings
}

fn save_settings_disk(settings: &AppSettings) {
    if std::fs::create_dir_all(STORE_DIR).is_ok() {
        if let Ok(json) = serde_json::to_string_pretty(settings) {
            let _ = std::fs::write(SETTINGS_JSON_PATH, json);
        }
    }
}

fn update_settings<F: FnOnce(&mut AppSettings)>(f: F) {
    let mut guard = CACHED_SETTINGS.lock().unwrap();
    let mut settings = guard.clone().unwrap_or_else(load_or_init_settings);
    f(&mut settings);
    save_settings_disk(&settings);
    *guard = Some(settings);
}

/// Frame rate to request from GFN.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamFps {
    #[default]
    Sixty,
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

    fn from_value(fps: u32) -> Self {
        match fps {
            30 => Self::Thirty,
            _ => Self::Sixty,
        }
    }
}

pub fn fps() -> StreamFps {
    let s = load_or_init_settings();
    StreamFps::from_value(s.fps)
}

pub fn set_fps(fps: StreamFps) {
    update_settings(|s| s.fps = fps.value());
}

/// How hard a rear-panel touch presses L2/R2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TriggerIntensity {
    #[default]
    Full,
    High,
    Half,
}

impl TriggerIntensity {
    pub const ALL: [TriggerIntensity; 3] = [Self::Full, Self::High, Self::Half];

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

pub fn trigger_intensity() -> TriggerIntensity {
    let s = load_or_init_settings();
    TriggerIntensity::from_value(s.trigger_intensity)
}

pub fn set_trigger_intensity(intensity: TriggerIntensity) {
    update_settings(|s| s.trigger_intensity = intensity.value());
}

pub fn stick_zones() -> StickZones {
    let s = load_or_init_settings();
    StickZones::from_text(&s.stick_zones)
}

pub fn set_stick_zones(zones: StickZones) {
    update_settings(|s| s.stick_zones = zones.as_text().to_owned());
}

/// How much the decoded stream is amplified, in percent of unity gain.
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
    let s = load_or_init_settings();
    AudioBoost::from_percent(s.audio_boost_percent)
}

pub fn set_audio_boost(boost: AudioBoost) {
    update_settings(|s| s.audio_boost_percent = boost.percent());
}

pub fn controls_hint_seen() -> bool {
    let s = load_or_init_settings();
    s.controls_hint_seen
}

pub fn mark_controls_hint_seen() {
    update_settings(|s| s.controls_hint_seen = true);
}

/// Whether the front screen's bottom corners act as L3/R3, and whether they are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StickZones {
    Off,
    #[default]
    Hidden,
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

pub fn saved_catalog_sort() -> String {
    let s = load_or_init_settings();
    s.catalog_sort
}

pub fn set_saved_catalog_sort(sort_text: &str) {
    update_settings(|s| s.catalog_sort = sort_text.to_owned());
}

pub fn saved_catalog_filter() -> String {
    let s = load_or_init_settings();
    s.catalog_filter
}

pub fn set_saved_catalog_filter(filter_text: &str) {
    update_settings(|s| s.catalog_filter = filter_text.to_owned());
}

// rear panel layout mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RearTouchMode {
    // left half L2, right half R2
    Halves,
    // 4 corners: TL L2, TR R2, BL L3, BR R3
    #[default]
    Quadrant,
}

impl RearTouchMode {
    pub const ALL: [RearTouchMode; 2] = [Self::Quadrant, Self::Halves];

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Quadrant => "settings-rear-touch-quadrant",
            Self::Halves => "settings-rear-touch-halves",
        }
    }

    fn from_text(text: &str) -> Self {
        match text.trim() {
            "halves" => Self::Halves,
            _ => Self::Quadrant,
        }
    }

    pub fn as_text(self) -> &'static str {
        match self {
            Self::Halves => "halves",
            Self::Quadrant => "quadrant",
        }
    }
}

pub fn rear_touch_mode() -> RearTouchMode {
    let s = load_or_init_settings();
    RearTouchMode::from_text(&s.rear_touch_mode)
}

pub fn set_rear_touch_mode(mode: RearTouchMode) {
    update_settings(|s| s.rear_touch_mode = mode.as_text().to_owned());
}
