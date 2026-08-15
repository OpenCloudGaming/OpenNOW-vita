
use crate::gfn::regions::StreamRegion;
use crate::gfn::stream_prefs::{
    AudioBoost, ColorDepth, GameLanguage, RearTouchMode, StickZones, StreamFps, TriggerIntensity,
};
use crate::i18n::I18n;
use crate::input::AppCommand;
use crate::locale::Locale;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Stream,
    Controls,
    App,
    Account,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 4] = [Self::Stream, Self::Controls, Self::App, Self::Account];

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Stream => "settings-tab-stream",
            Self::Controls => "settings-tab-controls",
            Self::App => "settings-tab-app",
            Self::Account => "settings-tab-account",
        }
    }

    pub fn group_key(self) -> &'static str {
        match self {
            Self::Stream => "settings-group-streaming",
            Self::Controls => "settings-group-controls",
            Self::App => "settings-group-app",
            Self::Account => "settings-group-account",
        }
    }

    fn next(self) -> Self {
        let all = Self::ALL;
        let index = all.iter().position(|&tab| tab == self).unwrap_or(0);
        all[(index + 1) % all.len()]
    }

    fn prev(self) -> Self {
        let all = Self::ALL;
        let index = all.iter().position(|&tab| tab == self).unwrap_or(0);
        all[(index + all.len() - 1) % all.len()]
    }

    pub fn shifted(self, delta: i32) -> Self {
        if delta < 0 { self.prev() } else { self.next() }
    }

    pub fn row_count(self) -> usize {
        match self {
            Self::Stream => 5,
            Self::Controls => 5,
            Self::App => 2,
            Self::Account => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Choice,
    Toggle(bool),
    Region,
}

pub struct RowInfo {
    pub label_key: &'static str,
    pub desc_key: Option<&'static str>,
    pub kind: RowKind,
}

pub fn row_info(tab: SettingsTab, row: usize) -> Option<RowInfo> {
    Some(match (tab, row) {
        (SettingsTab::Stream, 0) => RowInfo {
            label_key: "settings-region-heading",
            desc_key: Some("settings-region-desc"),
            kind: RowKind::Region,
        },
        (SettingsTab::Stream, 1) => RowInfo {
            label_key: "settings-game-language-heading",
            desc_key: Some("settings-game-language-desc"),
            kind: RowKind::Choice,
        },
        (SettingsTab::Stream, 2) => RowInfo {
            label_key: "settings-fps-heading",
            desc_key: Some("settings-fps-desc"),
            kind: RowKind::Choice,
        },
        (SettingsTab::Stream, 3) => RowInfo {
            label_key: "settings-audio-boost-heading",
            desc_key: Some("settings-audio-boost-desc"),
            kind: RowKind::Choice,
        },
        (SettingsTab::Stream, 4) => RowInfo {
            label_key: "settings-color-depth-heading",
            desc_key: Some("settings-color-depth-desc"),
            kind: RowKind::Choice,
        },
        (SettingsTab::Controls, 0) => RowInfo {
            label_key: "settings-rear-touch-mode-heading",
            desc_key: Some("settings-rear-touch-desc"),
            kind: RowKind::Choice,
        },
        (SettingsTab::Controls, 1) => RowInfo {
            label_key: "settings-stick-zones-heading",
            desc_key: Some("settings-stick-zones-desc"),
            kind: RowKind::Choice,
        },
        (SettingsTab::Controls, 2) => RowInfo {
            label_key: "settings-trigger-heading",
            desc_key: Some("settings-trigger-desc"),
            kind: RowKind::Choice,
        },
        (SettingsTab::Controls, 3) => RowInfo {
            label_key: "settings-game-profile-heading",
            desc_key: Some("settings-game-profile-desc"),
            kind: RowKind::Toggle(crate::gfn::stream_prefs::active_game_has_profile()),
        },
        (SettingsTab::Controls, 4) => RowInfo {
            label_key: "settings-trigger-swap-heading",
            desc_key: Some("settings-trigger-swap-desc"),
            kind: RowKind::Toggle(crate::gfn::stream_prefs::trigger_swap_enabled()),
        },
        (SettingsTab::App, 0) => RowInfo {
            label_key: "settings-language-heading",
            desc_key: Some("settings-language-desc"),
            kind: RowKind::Choice,
        },
        (SettingsTab::App, 1) => RowInfo {
            label_key: "settings-session-timer-heading",
            desc_key: Some("settings-session-timer-desc"),
            kind: RowKind::Toggle(crate::gfn::stream_prefs::session_timer_enabled()),
        },
        _ => return None,
    })
}

pub fn option_count(tab: SettingsTab, row: usize, regions_len: usize) -> usize {
    match row_info(tab, row).map(|info| info.kind) {
        Some(RowKind::Choice) => match (tab, row) {
            (SettingsTab::Stream, 1) => GameLanguage::ALL.len(),
            (SettingsTab::Stream, 2) => StreamFps::ALL.len(),
            (SettingsTab::Stream, 3) => AudioBoost::ALL.len(),
            (SettingsTab::Stream, 4) => ColorDepth::ALL.len(),
            (SettingsTab::Controls, 0) => RearTouchMode::ALL.len(),
            (SettingsTab::Controls, 1) => StickZones::ALL.len(),
            (SettingsTab::Controls, 2) => TriggerIntensity::ALL.len(),
            (SettingsTab::App, 0) => Locale::ALL.len(),
            _ => 0,
        },
        Some(RowKind::Region) => 1 + regions_len,
        _ => 0,
    }
}

pub fn current_option_index(
    tab: SettingsTab,
    row: usize,
    regions: &[StreamRegion],
    current_locale: Locale,
) -> usize {
    match (tab, row) {
        (SettingsTab::Stream, 1) => GameLanguage::ALL
            .iter()
            .position(|&c| c == crate::gfn::stream_prefs::game_language())
            .unwrap_or(0),
        (SettingsTab::Stream, 2) => StreamFps::ALL
            .iter()
            .position(|&c| c == crate::gfn::stream_prefs::fps())
            .unwrap_or(0),
        (SettingsTab::Stream, 3) => AudioBoost::ALL
            .iter()
            .position(|&c| c == crate::gfn::stream_prefs::audio_boost())
            .unwrap_or(0),
        (SettingsTab::Stream, 4) => ColorDepth::ALL
            .iter()
            .position(|&c| c == crate::gfn::stream_prefs::color_depth())
            .unwrap_or(0),
        (SettingsTab::Controls, 0) => RearTouchMode::ALL
            .iter()
            .position(|&c| c == crate::gfn::stream_prefs::rear_touch_mode())
            .unwrap_or(0),
        (SettingsTab::Controls, 1) => StickZones::ALL
            .iter()
            .position(|&c| c == crate::gfn::stream_prefs::stick_zones())
            .unwrap_or(0),
        (SettingsTab::Controls, 2) => TriggerIntensity::ALL
            .iter()
            .position(|&c| c == crate::gfn::stream_prefs::trigger_intensity())
            .unwrap_or(0),
        (SettingsTab::App, 0) => Locale::ALL
            .iter()
            .position(|&c| c == current_locale)
            .unwrap_or(0),
        (SettingsTab::Stream, 0) => {
            let selected = crate::gfn::stream_prefs::region();
            if selected.is_empty() {
                0
            } else {
                regions
                    .iter()
                    .position(|region| region.url == selected)
                    .map(|index| index + 1)
                    .unwrap_or(0)
            }
        }
        _ => 0,
    }
}

pub fn option_label(
    tab: SettingsTab,
    row: usize,
    index: usize,
    i18n: &I18n,
    regions: &[StreamRegion],
) -> String {
    match (tab, row) {
        (SettingsTab::Stream, 1) => GameLanguage::ALL
            .get(index)
            .map(|c| c.label().to_owned())
            .unwrap_or_default(),
        (SettingsTab::Stream, 2) => StreamFps::ALL
            .get(index)
            .map(|c| c.value().to_string())
            .unwrap_or_default(),
        (SettingsTab::Stream, 3) => AudioBoost::ALL
            .get(index)
            .map(|c| format!("{}x", c.percent() / 100))
            .unwrap_or_default(),
        (SettingsTab::Stream, 4) => ColorDepth::ALL
            .get(index)
            .map(|c| i18n.text(c.label_key()).to_string())
            .unwrap_or_default(),
        (SettingsTab::Controls, 0) => RearTouchMode::ALL
            .get(index)
            .map(|c| i18n.text(c.label_key()).to_string())
            .unwrap_or_default(),
        (SettingsTab::Controls, 1) => StickZones::ALL
            .get(index)
            .map(|c| i18n.text(c.label_key()).to_string())
            .unwrap_or_default(),
        (SettingsTab::Controls, 2) => TriggerIntensity::ALL
            .get(index)
            .map(|c| format!("{}%", u32::from(c.value()) * 100 / 255))
            .unwrap_or_default(),
        (SettingsTab::App, 0) => Locale::ALL
            .get(index)
            .map(|c| c.label().to_owned())
            .unwrap_or_default(),
        (SettingsTab::Stream, 0) => {
            if index == 0 {
                i18n.text("settings-region-auto").to_string()
            } else {
                match regions.get(index - 1) {
                    Some(region) => match region.ping_ms {
                        Some(ms) => format!("{} · {ms} ms", region.name),
                        None => region.name.clone(),
                    },
                    None => String::new(),
                }
            }
        }
        _ => String::new(),
    }
}

pub fn current_summary(
    tab: SettingsTab,
    row: usize,
    i18n: &I18n,
    regions: &[StreamRegion],
    current_locale: Locale,
) -> String {
    let index = current_option_index(tab, row, regions, current_locale);
    option_label(tab, row, index, i18n, regions)
}

pub fn command_for(
    tab: SettingsTab,
    row: usize,
    index: usize,
    regions: &[StreamRegion],
) -> Option<AppCommand> {
    match (tab, row) {
        (SettingsTab::Stream, 1) => GameLanguage::ALL
            .get(index)
            .copied()
            .map(AppCommand::SetGameLanguage),
        (SettingsTab::Stream, 2) => StreamFps::ALL.get(index).copied().map(AppCommand::SetStreamFps),
        (SettingsTab::Stream, 3) => AudioBoost::ALL.get(index).copied().map(AppCommand::SetAudioBoost),
        (SettingsTab::Stream, 4) => ColorDepth::ALL
            .get(index)
            .copied()
            .map(AppCommand::SetColorDepth),
        (SettingsTab::Controls, 0) => RearTouchMode::ALL
            .get(index)
            .copied()
            .map(AppCommand::SetRearTouchMode),
        (SettingsTab::Controls, 1) => StickZones::ALL.get(index).copied().map(AppCommand::SetStickZones),
        (SettingsTab::Controls, 2) => TriggerIntensity::ALL
            .get(index)
            .copied()
            .map(AppCommand::SetTriggerIntensity),
        (SettingsTab::App, 0) => Locale::ALL.get(index).copied().map(AppCommand::SetLocale),
        (SettingsTab::App, 1) => Some(AppCommand::ToggleSessionTimer),
        (SettingsTab::Controls, 3) => Some(AppCommand::ToggleGameProfile),
        (SettingsTab::Controls, 4) => Some(AppCommand::ToggleTriggerSwap),
        (SettingsTab::Stream, 0) => {
            if index == 0 {
                Some(AppCommand::SetRegion(String::new()))
            } else {
                regions.get(index - 1).map(|region| AppCommand::SetRegion(region.url.clone()))
            }
        }
        _ => None,
    }
}
