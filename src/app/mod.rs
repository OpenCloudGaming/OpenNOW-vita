pub mod ui;

use crate::gfn::auth::{self, AuthTokens, DeviceCodeChallenge, DevicePollOutcome, GfnUser};
use crate::gfn::catalog::{self, GameSummary};
use crate::gfn::cloudmatch::{self, SessionInfo};
use crate::gfn::covers::{self, CoverStore};
use crate::gfn::signaling::{self, SignalingEvent, SignalingHandle};
use crate::input::{AppCommand, InputCommand};
use crate::jobs::{PollJob, poll_job};
use crate::locale::Locale;
use anyhow::Result;
use reqwest::Client;
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinHandle;

/// The outcome of trying to renew the saved GFN login.
enum SessionRefresh {
    /// New tokens are in place; whatever hit 401 can be retried as-is.
    Renewed,
    /// NVIDIA rejected the credential itself - the saved login is worthless now.
    ReauthenticationRequired,
    /// Something transient went wrong. The saved login is still good and must be kept.
    Failed(String),
}

/// What Confirm should retry from the `Error` screen.
pub enum ErrorRetry {
    RestartLogin,
    ReloadCatalog(GfnUser),
    BackToCatalog {
        user: GfnUser,
        games: Vec<GameSummary>,
        selected: usize,
        filtered_indices: Vec<usize>,
        search_query: String,
        search_requested: bool,
        covers: CoverStore,
    },
}

#[derive(Clone, Copy)]
enum ListStep {
    Up,
    Down,
}

/// Moves `selected` through the single-column library list by one row in `step`'s direction,
/// clamping at either end.
fn move_in_list(len: usize, selected: usize, step: ListStep) -> usize {
    if len == 0 {
        return selected;
    }
    let max = len - 1;
    match step {
        ListStep::Up => selected.saturating_sub(1),
        ListStep::Down => (selected + 1).min(max),
    }
}

/// Library sort order, picked from the catalog screen's sort dropdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CatalogSort {
    /// Most-recently-launched-by-this-account first (`GameSummary::last_played`), titles never
    /// played pushed to the end in whatever order they already had.
    #[default]
    LastPlayed,
    /// GFN's own server-side ranking (relevance + name) - the order `games` already arrives in,
    /// so this is a no-op past filtering.
    Relevance,
    TitleAsc,
    TitleDesc,
}

impl CatalogSort {
    pub const ALL: [CatalogSort; 4] =
        [Self::LastPlayed, Self::Relevance, Self::TitleAsc, Self::TitleDesc];

    /// Fluent message id for this option's label in the sort dropdown.
    pub fn label_key(self) -> &'static str {
        match self {
            Self::LastPlayed => "catalog-sort-last-played",
            Self::Relevance => "catalog-sort-relevance",
            Self::TitleAsc => "catalog-sort-title-asc",
            Self::TitleDesc => "catalog-sort-title-desc",
        }
    }

    pub fn as_text(self) -> &'static str {
        match self {
            Self::LastPlayed => "last_played",
            Self::Relevance => "relevance",
            Self::TitleAsc => "title_asc",
            Self::TitleDesc => "title_desc",
        }
    }

    pub fn from_text(text: &str) -> Self {
        match text.trim() {
            "relevance" => Self::Relevance,
            "title_asc" => Self::TitleAsc,
            "title_desc" => Self::TitleDesc,
            _ => Self::LastPlayed,
        }
    }
}

// my games vs whole gfn catalog, default is my games obviously
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CatalogFilter {
    #[default]
    MyGames,
    AllGames,
}

impl CatalogFilter {
    pub const ALL: [CatalogFilter; 2] = [Self::MyGames, Self::AllGames];

    // label for the dropdown
    pub fn label_key(self) -> &'static str {
        match self {
            Self::MyGames => "catalog-filter-my-games",
            Self::AllGames => "catalog-filter-all-games",
        }
    }

    pub fn as_text(self) -> &'static str {
        match self {
            Self::MyGames => "my_games",
            Self::AllGames => "all_games",
        }
    }

    pub fn from_text(text: &str) -> Self {
        match text.trim() {
            "all_games" => Self::AllGames,
            _ => Self::MyGames,
        }
    }
}

/// How many catalog pages (`CATALOG_PAGE_SIZE` titles each) are walked before we stop.
const MAX_CATALOG_PAGES: usize = 5;

/// Which local filter to apply on top of a set of server results.
fn effective_local_query<'a>(typed: &'a str, server_query: &str) -> &'a str {
    if typed.trim().eq_ignore_ascii_case(server_query.trim()) {
        ""
    } else {
        typed
    }
}

/// Cursor-pagination bookkeeping for the catalog currently in `AppState::Catalog`.
#[derive(Default)]
struct CatalogPaging {
    /// The server query these pages belong to (`""` = plain browse).
    server_query: String,
    /// Cursor for the next page, `None` once the server says there are no more.
    next_cursor: Option<String>,
    pages_loaded: usize,
    total_count: Option<usize>,
    /// In-flight next-page fetch, tagged with the `generation` it was spawned under.
    job: Option<(u64, PollJob<catalog::CatalogPage>)>,
    /// Bumped whenever `games` is replaced wholesale (new search, reload).
    generation: u64,
}

impl CatalogPaging {
    /// Resets to "page 1 of `server_query` just landed", invalidating any in-flight page job.
    fn restart(&mut self, server_query: String, page: &catalog::CatalogPage) {
        self.abort_job();
        self.generation = self.generation.wrapping_add(1);
        self.server_query = server_query;
        self.next_cursor = page.next_cursor.clone();
        self.pages_loaded = 1;
        self.total_count = page.total_count;
    }

    fn abort_job(&mut self) {
        if let Some((_, PollJob::Pending(handle))) = self.job.take() {
            handle.abort();
        }
    }

    fn has_more(&self) -> bool {
        self.next_cursor.is_some() && self.pages_loaded < MAX_CATALOG_PAGES
    }
}

/// Returns the indices of `games` whose title contains `query` (case-insensitive), ordered per
/// `sort`.
fn filter_indices(games: &[GameSummary], query: &str, sort: CatalogSort) -> Vec<usize> {
    filter_indices_with_favorites(
        games,
        query,
        sort,
        &crate::gfn::favorites::ids(&crate::gfn::favorites::load()),
    )
}

/// Same, but with the starred set passed in - so a caller that already has it does not re-read the
/// memory card, and so the ordering is testable without one.
///
/// Favorites float to the top of whatever `sort` produced. That is the whole point of starring a
/// game: not having to walk a library of hundreds to reach the three you actually play.
fn filter_indices_with_favorites(
    games: &[GameSummary],
    query: &str,
    sort: CatalogSort,
    favorites: &std::collections::BTreeSet<String>,
) -> Vec<usize> {
    let mut indices = sorted_indices(games, query, sort);
    // Only while browsing. Pinning these into *search* results puts games at the top that the
    // player did not ask for, which is exactly the complaint this replaces.
    if query.trim().is_empty() {
        indices.sort_by_key(|&index| group_rank(&games[index], favorites));
    }
    indices
}

/// Which band of the browse list a game belongs to: starred, played recently, everything else.
///
/// `sort_by_key` is stable, so within a band the order the chosen sort produced survives.
fn group_rank(
    game: &GameSummary,
    favorites: &std::collections::BTreeSet<String>,
) -> u8 {
    if favorites.contains(&game.app_id) {
        0
    } else if game.last_played.is_some() {
        1
    } else {
        2
    }
}

/// Adds any starred game the catalog has not paged in, so a favourite past the 1000-title cut-off
/// still has a row. Anything already present is left alone rather than duplicated.
fn merge_favorites(
    mut games: Vec<GameSummary>,
    favorites: &[crate::gfn::favorites::FavoriteGame],
) -> Vec<GameSummary> {
    let present: std::collections::BTreeSet<&str> =
        games.iter().map(|game| game.app_id.as_str()).collect();
    let missing: Vec<GameSummary> = favorites
        .iter()
        .filter(|favorite| !present.contains(favorite.app_id.as_str()))
        .map(|favorite| favorite.to_summary())
        .collect();
    games.extend(missing);
    games
}

fn sorted_indices(games: &[GameSummary], query: &str, sort: CatalogSort) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let mut indices: Vec<usize> = if query.is_empty() {
        (0..games.len()).collect()
    } else {
        games
            .iter()
            .enumerate()
            .filter(|(_, game)| game.search_key.contains(&query))
            .map(|(index, _)| index)
            .collect()
    };
    match sort {
        CatalogSort::Relevance => {}
        CatalogSort::TitleAsc => {
            indices.sort_unstable_by(|&a, &b| games[a].search_key.cmp(&games[b].search_key))
        }
        CatalogSort::TitleDesc => {
            indices.sort_unstable_by(|&a, &b| games[b].search_key.cmp(&games[a].search_key))
        }
        CatalogSort::LastPlayed => {
            indices.sort_by(|&a, &b| {
                match (&games[a].last_played, &games[b].last_played) {
                    (Some(x), Some(y)) => y.cmp(x),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            })
        }
    }
    indices
}

/// Single-argument localized text, for the associated fns that have no `&self` to reach the
/// current locale through.
fn tr(locale: Locale, id: &'static str, key: &'static str, value: impl ToString) -> String {
    let mut args = fluent_bundle::FluentArgs::new();
    args.set(key, crate::i18n::arg_string(value.to_string()));
    crate::i18n::I18n::new(locale).text_with(id, args)
}

// digs thru the error chain for a gfn code, the outer error is usually just anyhow context by now
fn gfn_error_code(error: &anyhow::Error) -> Option<crate::gfn::error_codes::GfnErrorCode> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<crate::gfn::error_codes::GfnError>())
        .map(|gfn| gfn.code)
}

/// Top-level screen the shell is currently rendering.
pub enum AppState {
    /// Press Confirm to start the device-code flow.
    Login,
    /// `POST /device/authorize` in flight.
    StartingDeviceLogin(PollJob<DeviceCodeChallenge>),
    /// Waiting for the user to complete login on another device; polling `/token` on
    /// `challenge.interval`.
    WaitingForDeviceAuthorization {
        challenge: DeviceCodeChallenge,
        poll_job: Option<PollJob<DevicePollOutcome>>,
        next_poll_at: Instant,
    },
    /// Fetching the VPC id + the catalog's first page after a successful (or restored) login.
    LoadingCatalog {
        user: GfnUser,
        job: PollJob<catalog::CatalogPage>,
    },
    /// Game library, browsable and searchable.
    Catalog {
        user: GfnUser,
        games: Vec<GameSummary>,
        /// `selected` indexes into `filtered_indices`, not directly into `games`.
        selected: usize,
        /// Indices into `games` that match the current `search_query`.
        filtered_indices: Vec<usize>,
        search_query: String,
        /// Set to `true` when the user presses the search button; the shell uses SDL's text input
        /// API to open the system/on-screen keyboard and feed the resulting text back via
        /// `AppCommand::SetSearchQuery`.
        search_requested: bool,
        /// Shared cover-art cache: lazily filled by async download tasks spawned from the UI loop
        /// as tiles become visible (see `app::ui::catalog_screen`).
        covers: CoverStore,
    },
    /// CloudMatch session creation + polling in progress.
    CreatingSession {
        user: GfnUser,
        games: Vec<GameSummary>,
        selected: usize,
        filtered_indices: Vec<usize>,
        search_query: String,
        search_requested: bool,
        covers: CoverStore,
        job: PollJob<SessionInfo>,
        queue_tracker: cloudmatch::QueueProgressTracker,
    },
    /// CloudMatch session is ready.
    SessionReady {
        user: GfnUser,
        games: Vec<GameSummary>,
        selected: usize,
        filtered_indices: Vec<usize>,
        search_query: String,
        search_requested: bool,
        covers: CoverStore,
        session: SessionInfo,
    },
    /// Connected to the session's NVST signaling WebSocket.
    Signaling {
        user: GfnUser,
        games: Vec<GameSummary>,
        selected: usize,
        filtered_indices: Vec<usize>,
        search_query: String,
        search_requested: bool,
        covers: CoverStore,
        session: SessionInfo,
        handle: SignalingHandle,
        offer_sdp: Option<String>,
    },
    /// Active WebRTC video/audio streaming session state.
    Streaming {
        user: GfnUser,
        games: Vec<GameSummary>,
        selected: usize,
        filtered_indices: Vec<usize>,
        search_query: String,
        search_requested: bool,
        covers: CoverStore,
        session: SessionInfo,
        handle: SignalingHandle,
        peer: crate::gfn::peer::PeerEngine,
    },
    Error {
        message: String,
        retry: ErrorRetry,
        // kept separate from message so we can look up real wording instead of grepping text
        code: Option<crate::gfn::error_codes::GfnErrorCode>,
    },
}

pub struct App {
    pub(crate) state: AppState,
    /// Used both for GFN REST/GraphQL calls (from the async `AppState` tasks below) and - via
    /// `app::ui::build_ui`, which also borrows `&App` - for the per-frame lazy cover-art download
    /// requests kicked off from the catalog grid renderer.
    pub(crate) http_client: Client,
    /// Set on every successful (or restored) login, cleared on sign-out.
    tokens: Option<AuthTokens>,
    /// When the last token refresh was attempted, so a failing refresh backs off instead of
    /// retrying on every one of the 60 ticks a second.
    last_refresh_attempt: Option<Instant>,
    /// Keyframe requests seen in the running session, sampled while the peer is still alive.
    link_keyframe_requests: u64,
    /// Whether the streaming diagnostics panel is showing. Off by default - it covers the game and
    /// only means anything while something is being debugged.
    pub(crate) show_stream_stats: bool,
    /// Starred app ids, read once at startup. The catalog list is rebuilt on every repaint, so
    /// hitting the memory card there would be a file read per frame.
    pub(crate) favorites: std::collections::BTreeSet<String>,
    /// The full records, kept so a starred game the catalog never paged in can still be drawn.
    favorite_games: Vec<crate::gfn::favorites::FavoriteGame>,
    /// Whether to explain the improvised buttons over the first session. The Vita has no L2/R2,
    /// no stick clicks and no mouse, and nothing on the hardware says where they went.
    pub(crate) show_controls_hint: bool,
    /// Debug readout of the last navigation command received, shown on the placeholder screen so
    /// input mapping can be sanity-checked on real hardware before there is anything else to look
    /// at.
    pub(crate) last_input: Option<InputCommand>,
    /// Transient one-line status message (e.g.
    pub(crate) status_note: Option<String>,
    /// Debounce/dispatch state for server-side catalog search.
    search_job: Option<(String, PollJob<catalog::CatalogPage>)>,
    /// Set when the query changed and a debounced server search hasn't fired for it yet.
    search_pending_since: Option<Instant>,
    /// The last query a server search was actually dispatched for - avoids re-firing once the
    /// debounce elapses if the user hasn't typed anything new since.
    last_dispatched_search_query: Option<String>,
    pub(crate) confirm_exit: bool,
    /// Whether the streaming toolbar is expanded (all buttons visible) or collapsed (only ▶ arrow).
    pub(crate) toolbar_expanded: bool,
    /// Whether the in-stream controls quick modal (L2/R2 & L3/R3 settings) is showing.
    pub(crate) show_controls_modal: bool,
    /// Whether front-touch trackpad input drives host mouse movement.
    pub(crate) mouse_trackpad_enabled: bool,
    /// UI display language, changed via the gear icon next to the avatar in the catalog screen.
    pub(crate) locale: Locale,
    /// Library sort order, changed via the sort dropdown next to the library header.
    pub(crate) catalog_sort: CatalogSort,
    // my games vs full catalog, next to the sort dropdown
    pub(crate) catalog_filter: CatalogFilter,
    /// Cached account VPC id, shared with every spawned catalog task so the id is fetched once
    /// per session instead of before every catalog/search request.
    vpc_id_cache: catalog::VpcIdCache,
    /// Background cursor-pagination state for the catalog list.
    paging: CatalogPaging,
    /// The CloudMatch session the in-flight launch task has created, published as soon as it
    /// exists so cancelling can release it. Without this the `SessionInfo` stayed trapped inside
    /// the spawned task: cancelling left a live session on NVIDIA's side, and the next launch
    /// tripped over it.
    launching_session: Arc<std::sync::Mutex<Option<SessionInfo>>>,
    /// Measures what the link actually delivered during the current session, so the next one can
    /// ask for a ceiling this network has been seen to reach.
    link_meter: Option<crate::gfn::link_estimate::LinkMeter>,
    /// Whether the launch in progress ever actually sat in NVIDIA's queue. The queue position
    /// reported by CloudMatch only describes the present moment, and the later launch states do
    /// not carry the queue tracker at all, so the answer is latched here for the stepper.
    pub(crate) launch_was_queued: bool,
    /// The plain-browse catalog (no search query), captured the moment a server-side search is
    /// about to overwrite `games` - see `advance_catalog_search`. Restored when the search is
    /// cleared instead of re-fetching page 1, which used to throw away every page paged in before
    /// the search started: clearing "holl" would shrink the list back down to a single fresh page
    /// and slowly regrow it, discarding scroll position along the way.
    browse_snapshot: Option<BrowseSnapshot>,
}

struct BrowseSnapshot {
    games: Vec<GameSummary>,
    next_cursor: Option<String>,
    pages_loaded: usize,
    total_count: Option<usize>,
}

impl App {
    /// Returns the current Bearer token if the user is logged in.
    /// Localized text in the user's chosen UI language. The error and status strings these build
    /// were hardcoded Spanish, so an English UI still reported failures in Spanish.
    fn tr(&self, id: &'static str) -> String {
        crate::i18n::I18n::new(self.locale).text(id)
    }

    fn tr1(&self, id: &'static str, key: &'static str, value: impl ToString) -> String {
        tr(self.locale, id, key, value)
    }

    fn tr2(
        &self,
        id: &'static str,
        first: (&'static str, impl ToString),
        second: (&'static str, impl ToString),
    ) -> String {
        let mut args = fluent_bundle::FluentArgs::new();
        args.set(first.0, crate::i18n::arg_string(first.1.to_string()));
        args.set(second.0, crate::i18n::arg_string(second.1.to_string()));
        crate::i18n::I18n::new(self.locale).text_with(id, args)
    }

    pub fn bearer_token(&self) -> Option<&str> {
        self.tokens.as_ref().map(|tokens| tokens.bearer())
    }

    /// How many titles the server says match in total, for the catalog header's "N of M".
    pub(crate) fn catalog_total_count(&self) -> Option<usize> {
        self.paging.total_count
    }

    /// Whether another catalog page is on its way, so the UI can say the list is still growing.
    pub(crate) fn is_loading_more_catalog(&self) -> bool {
        self.paging.job.is_some()
    }

    pub fn new() -> Result<Self> {
        let favorite_games = crate::gfn::favorites::load();
        let http_client = auth::client();
        let tokens = auth::load_tokens();
        let vpc_id_cache = catalog::VpcIdCache::default();
        let catalog_filter =
            CatalogFilter::from_text(&crate::gfn::stream_prefs::saved_catalog_filter());
        let owned_only = catalog_filter == CatalogFilter::MyGames;
        let state = match &tokens {
            Some(tokens) => match auth::user_from_tokens(tokens) {
                Ok(user) => {
                    Self::start_catalog_fetch(&http_client, tokens, &vpc_id_cache, user, owned_only)
                }
                Err(error) => {
                    eprintln!("Saved GFN login could not be decoded, clearing it: {error:#}");
                    auth::clear_tokens();
                    AppState::Login
                }
            },
            None => AppState::Login,
        };

        Ok(Self {
            state,
            http_client,
            tokens,
            last_refresh_attempt: None,
            show_stream_stats: false,
            link_keyframe_requests: 0,
            favorites: crate::gfn::favorites::ids(&favorite_games),
            favorite_games,
            show_controls_hint: !crate::gfn::stream_prefs::controls_hint_seen(),
            last_input: None,
            status_note: None,
            search_job: None,
            search_pending_since: None,
            last_dispatched_search_query: None,
            confirm_exit: false,
            toolbar_expanded: false,
            show_controls_modal: false,
            mouse_trackpad_enabled: true,
            locale: Locale::default(),
            catalog_sort: CatalogSort::from_text(&crate::gfn::stream_prefs::saved_catalog_sort()),
            catalog_filter,
            vpc_id_cache,
            paging: CatalogPaging::default(),
            browse_snapshot: None,
            launching_session: Arc::new(std::sync::Mutex::new(None)),
            launch_was_queued: false,
            link_meter: None,
        })
    }

    pub async fn handle_command(&mut self, command: AppCommand) -> Result<()> {
        let bearer_token = self.bearer_token().map(|s| s.to_owned());
        let http_client = self.http_client.clone();

        let current_state = std::mem::replace(&mut self.state, AppState::Login);
        self.state = match command {
            AppCommand::SetSearchQuery(query) => {
                return self.apply_search_query(current_state, query);
            }
            AppCommand::RequestSearch => {
                return self.request_search(current_state);
            }
            AppCommand::CloseSearch => {
                return self.close_search(current_state);
            }
            AppCommand::ToggleConfirmExit => {
                self.confirm_exit = !self.confirm_exit;
                current_state
            }
            AppCommand::CancelConfirmExit => {
                self.confirm_exit = false;
                current_state
            }
            AppCommand::ConfirmExitSession => {
                self.confirm_exit = false;
                self.exit_session(current_state)?
            }
            AppCommand::SetLocale(locale) => {
                self.locale = locale;
                current_state
            }
            AppCommand::SetTriggerIntensity(intensity) => {
                crate::gfn::stream_prefs::set_trigger_intensity(intensity);
                current_state
            }
            AppCommand::SetRearTouchMode(mode) => {
                crate::gfn::stream_prefs::set_rear_touch_mode(mode);
                if mode == crate::gfn::stream_prefs::RearTouchMode::Quadrant {
                    // turn off front L3/R3 so they dont fight with rear panel
                    crate::gfn::stream_prefs::set_stick_zones(crate::gfn::stream_prefs::StickZones::Off);
                }
                current_state
            }
            AppCommand::SetStickZones(zones) => {
                crate::gfn::stream_prefs::set_stick_zones(zones);
                if zones != crate::gfn::stream_prefs::StickZones::Off {
                    // same deal but reverse, drop rear back to 2 zones
                    crate::gfn::stream_prefs::set_rear_touch_mode(crate::gfn::stream_prefs::RearTouchMode::Halves);
                }
                current_state
            }
            AppCommand::DismissControlsHint => {
                crate::gfn::stream_prefs::mark_controls_hint_seen();
                self.show_controls_hint = false;
                current_state
            }
            AppCommand::ToggleFavorite(app_id) => {
                // Works on `current_state`, not `self.state`: the caller moved the real state out
                // with `mem::replace` above, so `self.state` is a placeholder for the duration of
                // this call. Reading the catalog from it found no games and starring did nothing.
                let mut state = current_state;
                if let AppState::Catalog {
                    games,
                    filtered_indices,
                    search_query,
                    ..
                } = &mut state
                {
                    // Starring stores the whole summary, not just the id, so the game survives
                    // being outside whatever the catalog happens to have paged in.
                    if let Some(game) = games.iter().find(|g| g.app_id == app_id).cloned() {
                        self.favorite_games = crate::gfn::favorites::toggle(&game);
                        self.favorites = crate::gfn::favorites::ids(&self.favorite_games);
                        // Re-sort now so the game visibly moves, rather than on some later rebuild.
                        *filtered_indices = filter_indices_with_favorites(
                            games,
                            search_query,
                            self.catalog_sort,
                            &self.favorites,
                        );
                    }
                }
                state
            }
            AppCommand::ToggleStreamStats => {
                self.show_stream_stats = !self.show_stream_stats;
                current_state
            }
            AppCommand::ToggleToolbar => {
                self.toolbar_expanded = !self.toolbar_expanded;
                current_state
            }
            AppCommand::RightClick => {
                current_state
            }
            AppCommand::ToggleControlsModal => {
                self.show_controls_modal = !self.show_controls_modal;
                current_state
            }
            AppCommand::ToggleMouseTrackpad => {
                self.mouse_trackpad_enabled = !self.mouse_trackpad_enabled;
                current_state
            }
            AppCommand::SetMaxBitrate(kbps) => {
                if let AppState::Streaming { peer, .. } = &current_state {
                    peer.set_max_bitrate(kbps);
                }
                current_state
            }
            AppCommand::ToggleKeyboard => {
                if crate::ime::is_open() {
                    crate::ime::close();
                } else {
                    crate::ime::open();
                }
                current_state
            }
            AppCommand::SendKey(key) => {
                if let AppState::Streaming { peer, .. } = &current_state {
                    peer.tap_key(key);
                }
                current_state
            }
            AppCommand::SetAudioBoost(boost) => {
                // Read when the decode worker starts, so this applies from the next session on
                // rather than mid-stream.
                crate::gfn::stream_prefs::set_audio_boost(boost);
                current_state
            }
            AppCommand::SetStreamFps(fps) => {
                // Takes effect on the next launch: the frame rate is negotiated in the SDP answer
                // and cannot be renegotiated mid-session.
                crate::gfn::stream_prefs::set_fps(fps);
                current_state
            }
            AppCommand::SelectGame(index) => {
                let mut state = current_state;
                if let AppState::Catalog {
                    selected,
                    filtered_indices,
                    ..
                } = &mut state
                    && index < filtered_indices.len()
                {
                    *selected = index;
                }
                state
            }
            AppCommand::SetSort(sort) => {
                self.catalog_sort = sort;
                crate::gfn::stream_prefs::set_saved_catalog_sort(sort.as_text());
                match current_state {
                    AppState::Catalog {
                        user,
                        games,
                        selected: _,
                        filtered_indices: _,
                        search_query,
                        search_requested,
                        covers,
                    } => {
                        let local = effective_local_query(&search_query, &self.paging.server_query);
                        let filtered_indices = filter_indices(&games, local, sort);
                        AppState::Catalog {
                            user,
                            games,
                            selected: 0,
                            filtered_indices,
                            search_query,
                            search_requested,
                            covers,
                        }
                    }
                    other => other,
                }
            }
            AppCommand::SetFilter(filter) => {
                self.catalog_filter = filter;
                crate::gfn::stream_prefs::set_saved_catalog_filter(filter.as_text());
                match current_state {
                    AppState::Catalog { user, .. } => {
                        self.browse_snapshot = None;
                        self.paging.abort_job();
                        Self::start_catalog_fetch(
                            &self.http_client,
                            self.tokens.as_ref().expect("catalog requires a saved login"),
                            &self.vpc_id_cache,
                            user,
                            filter == CatalogFilter::MyGames,
                        )
                    }
                    other => other,
                }
            }
            AppCommand::Input(input) => {
                self.last_input = Some(input);
                self.handle_input_command(current_state, input, bearer_token, http_client)
                    .await?
            }
        };
        Ok(())
    }

    fn apply_search_query(&mut self, state: AppState, query: String) -> Result<()> {
        self.state = match state {
            AppState::Catalog {
                user,
                games,
                selected: _,
                filtered_indices: _,
                search_query: _,
                search_requested,
                covers,
            } => {
                let filtered_indices = filter_indices(&games, &query, self.catalog_sort);
                let selected = 0;
                self.search_pending_since = Some(Instant::now());
                AppState::Catalog {
                    user,
                    games,
                    selected,
                    filtered_indices,
                    search_query: query,
                    search_requested,
                    covers,
                }
            }
            other => other,
        };
        Ok(())
    }

    /// Flip the `search_requested` flag to true so the shell can start the platform text-input
    /// method (SDL IME / on-screen keyboard).
    fn request_search(&mut self, state: AppState) -> Result<()> {
        self.state = match state {
            AppState::Catalog {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested: _,
                covers,
            } => AppState::Catalog {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested: true,
                covers,
            },
            other => other,
        };
        Ok(())
    }

    fn close_search(&mut self, state: AppState) -> Result<()> {
        self.state = match state {
            AppState::Catalog {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested: _,
                covers,
            } => AppState::Catalog {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested: false,
                covers,
            },
            other => other,
        };
        Ok(())
    }

    /// Tells CloudMatch to release the session, fired off as a background task so the caller (an
    /// exit button press, or a disconnect being turned into an error screen) never blocks on it.
    fn stop_cloudmatch_session(&self, session: &SessionInfo) {
        let Some(token) = self.bearer_token().map(str::to_owned) else {
            return;
        };
        let client = self.http_client.clone();
        let session = session.clone();
        tokio::spawn(async move {
            cloudmatch::stop_session(&client, &token, &session).await;
        });
    }

    fn exit_session(&mut self, state: AppState) -> Result<AppState> {
        let new_state = match state {
            AppState::CreatingSession {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested,
                covers,
                job,
                ..
            } => {
                // Dropping a `JoinHandle` detaches the task, it does not stop it - the launch task
                // used to keep polling CloudMatch for up to 1800 attempts after the user
                // cancelled, holding the session open and burning request quota (a good way to
                // earn a REQUEST_LIMIT_EXCEEDED 429 on the next launch).
                if let PollJob::Pending(handle) = job {
                    handle.abort();
                }
                // The aborted task never gets to run its own cleanup, so releasing the session it
                // had already created is on us. Without this the next launch collided with a
                // session that was still alive server-side.
                let created = self
                    .launching_session
                    .lock()
                    .ok()
                    .and_then(|mut slot| slot.take());
                if let Some(session) = created {
                    self.stop_cloudmatch_session(&session);
                }
                AppState::Catalog {
                    user,
                    games,
                    selected,
                    filtered_indices,
                    search_query,
                    search_requested,
                    covers,
                }
            }
            AppState::SessionReady {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested,
                covers,
                session,
            } => {
                self.stop_cloudmatch_session(&session);
                AppState::Catalog {
                    user,
                    games,
                    selected,
                    filtered_indices,
                    search_query,
                    search_requested,
                    covers,
                }
            }
            AppState::Signaling {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested,
                covers,
                session,
                handle,
                ..
            }
            | AppState::Streaming {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested,
                covers,
                session,
                handle,
                ..
            } => {
                handle.close();
                self.stop_cloudmatch_session(&session);
                AppState::Catalog {
                    user,
                    games,
                    selected,
                    filtered_indices,
                    search_query,
                    search_requested,
                    covers,
                }
            }
            other => other,
        };
        Ok(new_state)
    }

    async fn handle_input_command(
        &mut self,
        current_state: AppState,
        input: InputCommand,
        bearer_token: Option<String>,
        http_client: Client,
    ) -> Result<AppState> {
        Ok(match (current_state, input) {
            (AppState::Login, InputCommand::Confirm) => self.start_login_state(),
            (AppState::WaitingForDeviceAuthorization { .. }, InputCommand::Back) => AppState::Login,
            (
                AppState::Catalog {
                    user,
                    games,
                    selected,
                    filtered_indices,
                    search_query,
                    search_requested,
                    covers,
                },
                InputCommand::MoveUp,
            ) => {
                if search_requested {
                    AppState::Catalog {
                        user,
                        games,
                        selected,
                        filtered_indices,
                        search_query,
                        search_requested,
                        covers,
                    }
                } else {
                    let selected = move_in_list(filtered_indices.len(), selected, ListStep::Up);
                    AppState::Catalog {
                        user,
                        games,
                        selected,
                        filtered_indices,
                        search_query,
                        search_requested,
                        covers,
                    }
                }
            }
            (
                AppState::Catalog {
                    user,
                    games,
                    selected,
                    filtered_indices,
                    search_query,
                    search_requested,
                    covers,
                },
                InputCommand::MoveDown,
            ) => {
                if search_requested {
                    AppState::Catalog {
                        user,
                        games,
                        selected,
                        filtered_indices,
                        search_query,
                        search_requested,
                        covers,
                    }
                } else {
                    let selected = move_in_list(filtered_indices.len(), selected, ListStep::Down);
                    AppState::Catalog {
                        user,
                        games,
                        selected,
                        filtered_indices,
                        search_query,
                        search_requested,
                        covers,
                    }
                }
            }
            (
                AppState::Catalog {
                    user,
                    games,
                    selected,
                    filtered_indices,
                    search_query,
                    search_requested,
                    covers,
                },
                InputCommand::Confirm,
            ) => {
                if search_requested {
                    AppState::Catalog {
                        user,
                        games,
                        selected,
                        filtered_indices,
                        search_query,
                        search_requested: false,
                        covers,
                    }
                } else {
                    let game_index = filtered_indices.get(selected).copied();
                    match (
                        game_index.and_then(|index| games.get(index)),
                        bearer_token.clone(),
                    ) {
                        (Some(game), Some(token)) => {
                            let app_id = game.app_id.clone();
                            let queue_tracker = Arc::new(std::sync::Mutex::new(
                                cloudmatch::QueueStatus::default(),
                            ));
                            let tracker_clone = queue_tracker.clone();
                            // Republished for the cancel path; cleared here so a cancelled launch
                            // can't leave the previous attempt's session behind to be stopped
                            // twice.
                            if let Ok(mut slot) = self.launching_session.lock() {
                                *slot = None;
                            }
                            self.launch_was_queued = false;
                            let launching_session = self.launching_session.clone();
                            let handle: JoinHandle<Result<SessionInfo>> =
                                tokio::spawn(async move {
                                    let settings = cloudmatch::StreamSettings::for_vita();
                                    let session = cloudmatch::create_session(
                                        &http_client,
                                        cloudmatch::CreateSessionRequest {
                                            token: token.as_str(),
                                            app_id: &app_id,
                                            vpc_id: "",
                                            settings: &settings,
                                        },
                                    )
                                    .await?;
                                    if let Ok(mut slot) = launching_session.lock() {
                                        *slot = Some(session.clone());
                                    }
                                    let polled = cloudmatch::poll_session(
                                        &http_client,
                                        cloudmatch::PollSessionRequest {
                                            token: token.as_str(),
                                            session_id: &session.session_id,
                                            session: &session,
                                        },
                                        Some(tracker_clone),
                                    )
                                    .await;
                                    if polled.is_err() {
                                        cloudmatch::stop_session(
                                            &http_client,
                                            token.as_str(),
                                            &session,
                                        )
                                        .await;
                                    }
                                    polled
                                });
                            AppState::CreatingSession {
                                user,
                                games,
                                selected,
                                filtered_indices,
                                search_query,
                                search_requested,
                                covers,
                                job: PollJob::Pending(handle),
                                queue_tracker,
                            }
                        }
                        _ => {
                            self.status_note =
                                Some(self.tr("status-session-start-failed"));
                            AppState::Catalog {
                                user,
                                games,
                                selected,
                                filtered_indices,
                                search_query,
                                search_requested,
                                covers,
                            }
                        }
                    }
                }
            }
            (
                AppState::Catalog {
                    user,
                    games,
                    selected,
                    filtered_indices,
                    search_query,
                    search_requested,
                    covers,
                },
                InputCommand::Back,
            ) => {
                // One Back press both dismisses the on-screen keyboard and clears the query, same
                // as tapping the search box's × button - it used to take a Back press per step,
                // which read as "clearing the search doesn't work" when the first press only
                // closed the keyboard and left the old results filtered in.
                if search_requested || !search_query.is_empty() {
                    let new_filtered = filter_indices(&games, "", self.catalog_sort);
                    self.search_pending_since = Some(std::time::Instant::now());
                    AppState::Catalog {
                        user,
                        games,
                        selected: 0,
                        filtered_indices: new_filtered,
                        search_query: String::new(),
                        search_requested: false,
                        covers,
                    }
                } else {
                    AppState::Catalog {
                        user,
                        games,
                        selected,
                        filtered_indices,
                        search_query,
                        search_requested,
                        covers,
                    }
                }
            }
            (
                state @ (AppState::CreatingSession { .. }
                | AppState::SessionReady { .. }
                | AppState::Signaling { .. }),
                InputCommand::Back,
            ) => {
                self.confirm_exit = !self.confirm_exit;
                state
            }
            (
                AppState::SessionReady {
                    user,
                    games,
                    selected,
                    filtered_indices,
                    search_query,
                    search_requested,
                    covers,
                    session,
                },
                InputCommand::Confirm,
            ) => match signaling::connect(&session.signaling_url, &session.session_id) {
                Ok(handle) => {
                    self.status_note =
                        Some(self.tr("status-signaling-connecting"));
                    AppState::Signaling {
                        user,
                        games,
                        selected,
                        filtered_indices,
                        search_query,
                        search_requested,
                        covers,
                        session,
                        handle,
                        offer_sdp: None,
                    }
                }
                Err(error) => {
                    self.status_note =
                        Some(self.tr1("status-signaling-connect-failed", "error", format!("{error:#}")));
                    AppState::SessionReady {
                        user,
                        games,
                        selected,
                        filtered_indices,
                        search_query,
                        search_requested,
                        covers,
                        session,
                    }
                }
            }
            (
                AppState::Error {
                    code: None,
                    retry: ErrorRetry::RestartLogin,
                    ..
                },
                InputCommand::Confirm,
            ) => self.start_login_state(),
            (
                AppState::Error {
                    code: None,
                    retry: ErrorRetry::ReloadCatalog(user),
                    ..
                },
                InputCommand::Confirm,
            ) => {
                self.browse_snapshot = None;
                Self::start_catalog_fetch(
                    &self.http_client,
                    self.tokens.as_ref().expect("retry requires a saved login"),
                    &self.vpc_id_cache,
                    user,
                    self.catalog_filter == CatalogFilter::MyGames,
                )
            }
            (
                AppState::Error {
                    code: None,
                    retry:
                        ErrorRetry::BackToCatalog {
                            user,
                            games,
                            selected,
                            filtered_indices,
                            search_query,
                            search_requested,
                            covers,
                        },
                    ..
                },
                InputCommand::Confirm,
            ) => AppState::Catalog {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested,
                covers,
            },
            // Back dismisses the error the same way Confirm does. It used to fall through to
            // `Login`, which threw the session away and signed the user out over something as
            // recoverable as a failed launch.
            (
                AppState::Error {
                    code: None,
                    retry:
                        ErrorRetry::BackToCatalog {
                            user,
                            games,
                            selected,
                            filtered_indices,
                            search_query,
                            search_requested,
                            covers,
                        },
                    ..
                },
                InputCommand::Back,
            ) => AppState::Catalog {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested,
                covers,
            },
            (
                AppState::Error {
                    code: None,
                    retry: ErrorRetry::ReloadCatalog(user),
                    ..
                },
                InputCommand::Back,
            ) => {
                self.browse_snapshot = None;
                Self::start_catalog_fetch(
                    &self.http_client,
                    self.tokens.as_ref().expect("retry requires a saved login"),
                    &self.vpc_id_cache,
                    user,
                    self.catalog_filter == CatalogFilter::MyGames,
                )
            }
            (
                AppState::Error {
                    code: None,
                    retry: ErrorRetry::RestartLogin,
                    ..
                },
                InputCommand::Back,
            ) => AppState::Login,
            (other, _) => other,
        })
    }

    fn start_login_state(&self) -> AppState {
        let client = self.http_client.clone();
        let handle: JoinHandle<Result<DeviceCodeChallenge>> =
            tokio::spawn(async move { auth::start_device_login(&client).await });
        AppState::StartingDeviceLogin(PollJob::Pending(handle))
    }

    /// Kicks off the catalog load.
    fn start_catalog_fetch(
        client: &Client,
        tokens: &AuthTokens,
        vpc_id_cache: &catalog::VpcIdCache,
        user: GfnUser,
        owned_only: bool,
    ) -> AppState {
        let client = client.clone();
        let tokens = tokens.clone();
        let cache = vpc_id_cache.clone();
        let user_id = user.user_id.clone();
        let handle: JoinHandle<Result<catalog::CatalogPage>> = tokio::spawn(async move {
            // Renew first when the saved token is near expiry. This runs at startup with whatever
            // was on the memory card, and the proactive refresh in `tick` only gets a turn *after*
            // this request is already in flight - so a stale token would fail the VPC lookup and
            // land the player on a catalog that is wrong rather than on a sign-in prompt.
            let bearer = if tokens.needs_refresh() {
                match crate::gfn::auth::refresh_tokens(&client, &tokens, &user_id).await {
                    Ok(refreshed) => {
                        if let Err(error) = crate::gfn::auth::save_tokens(&refreshed) {
                            eprintln!("Could not persist refreshed GFN tokens: {error:#}");
                        }
                        refreshed.bearer().to_owned()
                    }
                    // Let the request go out anyway: the error path already knows how to turn a
                    // rejection into a sign-in prompt, and the token may still be good.
                    Err(error) => {
                        eprintln!("Startup token refresh failed: {error}");
                        tokens.bearer().to_owned()
                    }
                }
            } else {
                tokens.bearer().to_owned()
            };
            catalog::fetch_catalog_page_for_account(&client, &bearer, &cache, None, "", owned_only)
                .await
        });
        AppState::LoadingCatalog {
            user,
            job: PollJob::Pending(handle),
        }
    }

    /// How long to wait after the last keystroke before actually hitting the network.
    const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);

    /// Drives server-side catalog search: polls any in-flight search job to completion, then -
    /// once the debounce timer has elapsed for the current query and that query hasn't already
    /// been dispatched - fires a new one.
    async fn advance_catalog_search(&mut self) {
        let AppState::Catalog { search_query, .. } = &self.state else {
            self.search_job = None;
            self.search_pending_since = None;
            return;
        };
        let query = search_query.clone();

        if let Some((job_query, PollJob::Pending(handle))) = self.search_job.take() {
            match poll_job(handle).await {
                PollJob::Pending(handle) => {
                    self.search_job = Some((job_query, PollJob::Pending(handle)));
                }
                // The query moved on while this was in flight, so its results are for a search
                // nobody is looking at any more. Forgetting that it was dispatched matters: if the
                // user types back to exactly this query, the guard below would otherwise see it as
                // already dispatched and never fetch it, leaving the list stuck on stale results.
                PollJob::Done(_) if job_query != query => {
                    if self.last_dispatched_search_query.as_deref() == Some(job_query.as_str()) {
                        self.last_dispatched_search_query = None;
                    }
                }
                PollJob::Done(Ok(page)) => {
                    let result_count = page.games.len();
                    self.paging.restart(job_query, &page);
                    if let AppState::Catalog {
                        games,
                        filtered_indices,
                        selected,
                        ..
                    } = &mut self.state
                    {
                        *filtered_indices = filter_indices(&page.games, "", self.catalog_sort);
                        *games = page.games;
                        *selected = 0;
                    }
                    self.status_note = Some(self.tr2("status-search-results", ("count", result_count), ("query", &query)));
                }
                PollJob::Done(Err(error)) => {
                    self.status_note = Some(self.tr1("status-search-failed", "error", format!("{error:#}")));
                }
            }
            return;
        }

        let Some(pending_since) = self.search_pending_since else {
            return;
        };
        if pending_since.elapsed() < Self::SEARCH_DEBOUNCE {
            return;
        }
        if self.last_dispatched_search_query.as_deref() == Some(query.as_str()) {
            self.search_pending_since = None;
            return;
        }
        let trimmed = query.trim();
        let currently_searching = !self.paging.server_query.is_empty();

        // Clearing the query back to browse: if we kept a snapshot of what browse mode looked
        // like right before this search started, restore it directly instead of re-fetching page
        // 1 from the server - which used to throw away every page paged in before the search and
        // reset the scroll position, making "clear the search" look like it emptied the library.
        if trimmed.is_empty() && currently_searching {
            if let Some(snapshot) = self.browse_snapshot.take() {
                self.paging.abort_job();
                self.paging.generation = self.paging.generation.wrapping_add(1);
                self.paging.server_query = String::new();
                self.paging.next_cursor = snapshot.next_cursor;
                self.paging.pages_loaded = snapshot.pages_loaded;
                self.paging.total_count = snapshot.total_count;
                if let AppState::Catalog {
                    games,
                    filtered_indices,
                    selected,
                    ..
                } = &mut self.state
                {
                    *filtered_indices = filter_indices(&snapshot.games, "", self.catalog_sort);
                    *games = snapshot.games;
                    *selected = 0;
                }
                self.search_pending_since = None;
                self.last_dispatched_search_query = Some(query);
                return;
            }
        }

        // First step away from browse mode: remember what it looked like so clearing the search
        // later can restore it instead of re-fetching. A later, non-empty-to-non-empty edit (e.g.
        // "holl" -> "holla") must not overwrite this with the search-scoped state.
        if !trimmed.is_empty() && !currently_searching && self.browse_snapshot.is_none() {
            if let AppState::Catalog { games, .. } = &self.state {
                self.browse_snapshot = Some(BrowseSnapshot {
                    games: games.clone(),
                    next_cursor: self.paging.next_cursor.clone(),
                    pages_loaded: self.paging.pages_loaded,
                    total_count: self.paging.total_count,
                });
            }
        }

        let Some(token) = self.bearer_token().map(str::to_owned) else {
            return;
        };

        self.paging.abort_job();
        self.search_pending_since = None;
        self.last_dispatched_search_query = Some(query.clone());
        let client = self.http_client.clone();
        let cache = self.vpc_id_cache.clone();
        let dispatched = query.clone();
        let owned_only = self.catalog_filter == CatalogFilter::MyGames;
        let handle: JoinHandle<Result<catalog::CatalogPage>> = tokio::spawn(async move {
            let trimmed = dispatched.trim();
            let server_query = (!trimmed.is_empty()).then_some(trimmed);
            catalog::fetch_catalog_page_for_account(
                &client,
                &token,
                &cache,
                server_query,
                "",
                owned_only,
            )
            .await
        });
        self.search_job = Some((query, PollJob::Pending(handle)));
    }

    /// Streams the remaining catalog pages in behind the UI, appending each to `games` as it
    /// lands so the list grows while the user browses.
    async fn advance_catalog_paging(&mut self) {
        if !matches!(self.state, AppState::Catalog { .. }) {
            self.paging.abort_job();
            return;
        }

        if let Some((generation, PollJob::Pending(handle))) = self.paging.job.take() {
            match poll_job(handle).await {
                PollJob::Pending(handle) => {
                    self.paging.job = Some((generation, PollJob::Pending(handle)));
                }
                PollJob::Done(_) if generation != self.paging.generation => {}
                PollJob::Done(Ok(page)) => {
                    self.paging.next_cursor = page.next_cursor.clone();
                    self.paging.pages_loaded += 1;
                    if page.total_count.is_some() {
                        self.paging.total_count = page.total_count;
                    }
                    self.append_catalog_page(page.games);
                }
                PollJob::Done(Err(error)) => {
                    eprintln!("catalog page fetch failed (non-fatal): {error:#}");
                    self.paging.next_cursor = None;
                }
            }
            return;
        }

        if !self.paging.has_more()
            || self.search_job.is_some()
            || self.search_pending_since.is_some()
        {
            return;
        }
        let (Some(token), Some(cursor)) = (
            self.bearer_token().map(str::to_owned),
            self.paging.next_cursor.clone(),
        ) else {
            return;
        };

        let client = self.http_client.clone();
        let cache = self.vpc_id_cache.clone();
        let server_query = self.paging.server_query.clone();
        let generation = self.paging.generation;
        let owned_only = self.catalog_filter == CatalogFilter::MyGames;
        let handle: JoinHandle<Result<catalog::CatalogPage>> = tokio::spawn(async move {
            let trimmed = server_query.trim();
            let query = (!trimmed.is_empty()).then_some(trimmed);
            catalog::fetch_catalog_page_for_account(&client, &token, &cache, query, &cursor, owned_only)
                .await
        });
        self.paging.job = Some((generation, PollJob::Pending(handle)));
    }

    /// Appends a freshly fetched page to the catalog, keeping the highlighted title highlighted.
    fn append_catalog_page(&mut self, incoming: Vec<GameSummary>) {
        let sort = self.catalog_sort;
        let server_query = self.paging.server_query.clone();
        let AppState::Catalog {
            games,
            filtered_indices,
            selected,
            search_query,
            ..
        } = &mut self.state
        else {
            return;
        };

        let anchor = filtered_indices.get(*selected).copied();
        let seen: std::collections::HashSet<&str> =
            games.iter().map(|game| game.app_id.as_str()).collect();
        let fresh: Vec<GameSummary> = incoming
            .into_iter()
            .filter(|game| !seen.contains(game.app_id.as_str()))
            .collect();
        if fresh.is_empty() {
            return;
        }
        games.extend(fresh);

        let local = effective_local_query(search_query, &server_query);
        *filtered_indices = filter_indices(games, local, sort);
        *selected = anchor
            .and_then(|anchor| filtered_indices.iter().position(|&index| index == anchor))
            .unwrap_or_else(|| (*selected).min(filtered_indices.len().saturating_sub(1)));
    }

    /// Samples the link while streaming and files the result when the session ends, so the
    /// measurement survives into the next launch's requested bitrate ceiling.
    fn track_link_quality(&mut self) {
        match &self.state {
            AppState::Streaming { peer, .. } => {
                let bytes = peer.media_bytes();
                self.link_meter
                    .get_or_insert_with(crate::gfn::link_estimate::LinkMeter::new)
                    .sample(bytes);
                // Sampled while the session is up, because the peer is gone by the time the
                // measurement is folded in.
                self.link_keyframe_requests = peer.keyframe_requests();
            }
            _ => {
                if let Some(meter) = self.link_meter.take()
                    && let Some(mbps) = meter.peak_mbps()
                {
                    // Damaged frames are the signal that the link could not carry the stream.
                    // More than one keyframe request per 30 s of play is past what a healthy
                    // connection produces.
                    let stressed =
                        self.link_keyframe_requests > u64::from(meter.elapsed_secs() / 30).max(1);
                    crate::gfn::link_estimate::record(mbps, stressed);
                }
                self.link_keyframe_requests = 0;
            }
        }
    }

    /// Bounds how much decoded cover art stays resident, pruned on every tick.
    fn prune_covers(&self) {
        match &self.state {
            AppState::Catalog {
                games,
                selected,
                filtered_indices,
                covers,
                ..
            } => {
                let keep = ui::selected_game(games, filtered_indices, *selected)
                    .map(|game| game.app_id.as_str());
                covers.prune(keep, covers::MAX_CACHED_COVERS);
            }
            // The launch overlay shows the cover of the title being started, so that one has to
            // survive. Pruning these states to zero - which `prune_covers` runs every tick - threw
            // away the cover the catalog had just downloaded, the overlay re-requested it, and the
            // next tick threw it away again: a title could sit on the loading spinner forever,
            // re-fetching the same image.
            AppState::CreatingSession {
                games,
                selected,
                filtered_indices,
                covers,
                ..
            }
            | AppState::SessionReady {
                games,
                selected,
                filtered_indices,
                covers,
                ..
            }
            | AppState::Signaling {
                games,
                selected,
                filtered_indices,
                covers,
                ..
            } => {
                let keep = ui::selected_game(games, filtered_indices, *selected)
                    .map(|game| game.app_id.as_str());
                covers.prune(keep, 1);
            }
            // Nothing on screen needs cover art once video is up, and the decoder wants the
            // memory far more than the cache does.
            AppState::Streaming { covers, .. } => covers.prune(None, 0),
            _ => {}
        }
    }

    /// Per-frame housekeeping: advances whatever async step is in flight.
    pub async fn tick(&mut self) -> Result<()> {
        self.prune_covers();
        self.track_link_quality();
        self.pump_keyboard();
        self.maintain_session().await;
        self.advance_catalog_search().await;
        self.advance_catalog_paging().await;
        match std::mem::replace(&mut self.state, AppState::Login) {
            AppState::StartingDeviceLogin(job) => self.state = self.advance_login_start(job).await,
            AppState::WaitingForDeviceAuthorization {
                challenge,
                poll_job: pending_poll,
                next_poll_at,
            } => {
                self.state = self
                    .advance_login_poll(challenge, pending_poll, next_poll_at)
                    .await
            }
            AppState::LoadingCatalog { user, job } => {
                self.state = self.advance_catalog_load(user, job).await
            }
            AppState::CreatingSession {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested,
                covers,
                job,
                queue_tracker,
            } => {
                if let Ok(status) = queue_tracker.lock() {
                    self.launch_was_queued |= status.was_queued;
                }
                self.state = Self::advance_session_creation(
                    self.locale,
                    user,
                    games,
                    selected,
                    filtered_indices,
                    search_query,
                    search_requested,
                    covers,
                    job,
                    queue_tracker,
                )
                .await
            }
            AppState::Signaling {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested,
                covers,
                session,
                handle,
                offer_sdp,
            } => {
                self.state = self.advance_signaling(
                    user,
                    games,
                    selected,
                    filtered_indices,
                    search_query,
                    search_requested,
                    covers,
                    session,
                    handle,
                    offer_sdp,
                )
            }
            AppState::Streaming {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested,
                covers,
                session,
                mut handle,
                mut peer,
            } => {
                let mut fatal_reason: Option<String> = None;

                while let Some(event) = handle.try_recv() {
                    match event {
                        SignalingEvent::RemoteIce(candidate) => {
                            peer.add_remote_ice(candidate);
                        }
                        SignalingEvent::Disconnected(reason) => {
                            fatal_reason.get_or_insert(self.tr1("error-signaling-disconnected", "reason", &reason));
                            break;
                        }
                        _ => {}
                    }
                }
                while let Some(event) = peer.try_recv() {
                    match event {
                        crate::gfn::peer::PeerEvent::LocalAnswer { answer_sdp, nvst_sdp } => {
                            self.status_note =
                                Some("Answer SDP generado, enviado a NVIDIA...".to_owned());
                            handle.send_answer(answer_sdp, nvst_sdp);
                        }
                        crate::gfn::peer::PeerEvent::LocalIce(candidate) => {
                            handle.send_local_ice(candidate);
                        }
                        crate::gfn::peer::PeerEvent::Status(status) => {
                            self.status_note = Some(status);
                        }
                        crate::gfn::peer::PeerEvent::Connected => {
                            self.status_note = Some(self.tr("status-stream-live"));
                        }
                        crate::gfn::peer::PeerEvent::Error(err) => {
                            eprintln!("Streaming peer error: {err}");
                            self.status_note = Some(self.tr1("status-peer-error", "error", &err));
                        }
                        crate::gfn::peer::PeerEvent::TimeWarning { code, seconds_left } => {
                            let mins = (seconds_left + 59) / 60;
                            let msg = match code {
                                1 | 2 => format!("La sesión terminará en ~{mins} min"),
                                4 => format!("La sesión finalizará en breve ({seconds_left}s)"),
                                _ => format!("Aviso de tiempo de sesión: ~{mins} min restantes"),
                            };
                            self.status_note = Some(msg);
                        }
                        crate::gfn::peer::PeerEvent::Disconnected(reason) => {
                            eprintln!("Streaming peer disconnected: {reason}");
                            fatal_reason
                                .get_or_insert(self.tr1("error-stream-lost", "reason", &reason));
                            break;
                        }
                    }
                }

                if let Some(message) = fatal_reason {
                    handle.close();
                    self.stop_cloudmatch_session(&session);
                    self.state = AppState::Error {
                        code: None,
                        message,
                        retry: ErrorRetry::BackToCatalog {
                            user,
                            games,
                            selected,
                            filtered_indices,
                            search_query,
                            search_requested,
                            covers,
                        },
                    };
                } else {
                    self.state = AppState::Streaming {
                        user,
                        games,
                        selected,
                        filtered_indices,
                        search_query,
                        search_requested,
                        covers,
                        session,
                        handle,
                        peer,
                    };
                }
            }
            other => self.state = other,
        }
        Ok(())
    }

    /// Drains a bounded number of signaling events per tick (rather than all of them) so a burst
    /// of trickled ICE candidates can't stall a single frame indefinitely.
    #[allow(clippy::too_many_arguments)]
    fn advance_signaling(
        &mut self,
        user: GfnUser,
        games: Vec<GameSummary>,
        selected: usize,
        filtered_indices: Vec<usize>,
        search_query: String,
        search_requested: bool,
        covers: CoverStore,
        session: SessionInfo,
        mut handle: SignalingHandle,
        mut offer_sdp: Option<String>,
    ) -> AppState {
        const MAX_EVENTS_PER_TICK: usize = 8;
        let mut disconnected_reason: Option<String> = None;

        for _ in 0..MAX_EVENTS_PER_TICK {
            match handle.try_recv() {
                Some(SignalingEvent::Connected) => {
                    self.status_note =
                        Some(self.tr("status-signaling-connected"));
                }
                Some(SignalingEvent::Offer(sdp)) => {
                    self.status_note = Some(self.tr1("status-offer-received", "bytes", sdp.len()));
                    match crate::gfn::peer::PeerEngine::new(&sdp, &session) {
                        Ok(peer) => {
                            return AppState::Streaming {
                                user,
                                games,
                                selected,
                                filtered_indices,
                                search_query,
                                search_requested,
                                covers,
                                session,
                                handle,
                                peer,
                            };
                        }
                        Err(error) => {
                            eprintln!("failed to start peer engine: {error:#}");
                            offer_sdp = Some(sdp);
                        }
                    }
                }
                Some(SignalingEvent::RemoteIce(candidate)) => {
                    self.status_note = Some(self.tr1("status-remote-ice", "candidate", &candidate.candidate));
                }
                Some(SignalingEvent::Error(message)) => {
                    eprintln!("Signaling: {message}");
                }
                Some(SignalingEvent::Disconnected(reason)) => {
                    disconnected_reason = Some(reason);
                    break;
                }
                None => break,
            }
        }

        if let Some(reason) = disconnected_reason {
            self.stop_cloudmatch_session(&session);
            return AppState::Error {
                code: None,
                message: self.tr1("error-signaling-disconnected", "reason", &reason),
                retry: ErrorRetry::BackToCatalog {
                    user,
                    games,
                    selected,
                    filtered_indices,
                    search_query,
                    search_requested,
                    covers,
                },
            };
        }

        AppState::Signaling {
            user,
            games,
            selected,
            filtered_indices,
            search_query,
            search_requested,
            covers,
            session,
            handle,
            offer_sdp,
        }
    }

    async fn advance_login_start(&self, job: PollJob<DeviceCodeChallenge>) -> AppState {
        let PollJob::Pending(handle) = job else {
            return AppState::Login;
        };
        match poll_job(handle).await {
            PollJob::Pending(handle) => AppState::StartingDeviceLogin(PollJob::Pending(handle)),
            PollJob::Done(Ok(challenge)) => AppState::WaitingForDeviceAuthorization {
                next_poll_at: Instant::now() + challenge.interval,
                challenge,
                poll_job: None,
            },
            PollJob::Done(Err(error)) => AppState::Error {
                code: None,
                message: self.tr1("error-login-start", "error", format!("{error:#}")),
                retry: ErrorRetry::RestartLogin,
            },
        }
    }

    async fn advance_login_poll(
        &mut self,
        challenge: DeviceCodeChallenge,
        pending_poll: Option<PollJob<DevicePollOutcome>>,
        next_poll_at: Instant,
    ) -> AppState {
        if challenge.is_expired() {
            return AppState::Error {
                code: None,
                message: self.tr("error-login-code-expired"),
                retry: ErrorRetry::RestartLogin,
            };
        }

        let pending_poll = match pending_poll {
            Some(job) => Some(job),
            None if Instant::now() >= next_poll_at => {
                let client = self.http_client.clone();
                let challenge_for_task = challenge.clone();
                let handle: JoinHandle<Result<DevicePollOutcome>> = tokio::spawn(async move {
                    auth::poll_device_login(&client, &challenge_for_task).await
                });
                Some(PollJob::Pending(handle))
            }
            None => None,
        };

        let Some(job) = pending_poll else {
            return AppState::WaitingForDeviceAuthorization {
                challenge,
                poll_job: None,
                next_poll_at,
            };
        };

        let PollJob::Pending(handle) = job else {
            return AppState::WaitingForDeviceAuthorization {
                challenge,
                poll_job: None,
                next_poll_at,
            };
        };

        match poll_job(handle).await {
            PollJob::Pending(handle) => AppState::WaitingForDeviceAuthorization {
                challenge,
                poll_job: Some(PollJob::Pending(handle)),
                next_poll_at,
            },
            PollJob::Done(Ok(DevicePollOutcome::Pending)) => {
                AppState::WaitingForDeviceAuthorization {
                    next_poll_at: Instant::now() + challenge.interval,
                    challenge,
                    poll_job: None,
                }
            }
            PollJob::Done(Ok(DevicePollOutcome::SlowDown)) => {
                AppState::WaitingForDeviceAuthorization {
                    next_poll_at: Instant::now() + challenge.interval * 2,
                    challenge,
                    poll_job: None,
                }
            }
            PollJob::Done(Ok(DevicePollOutcome::Authorized(tokens))) => self.finish_login(tokens),
            PollJob::Done(Ok(DevicePollOutcome::Expired)) => AppState::Error {
                code: None,
                message: self.tr("error-login-code-expired"),
                retry: ErrorRetry::RestartLogin,
            },
            PollJob::Done(Ok(DevicePollOutcome::Denied)) => AppState::Error {
                code: None,
                message: self.tr("error-login-denied"),
                retry: ErrorRetry::RestartLogin,
            },
            PollJob::Done(Err(error)) => AppState::Error {
                code: None,
                message: self.tr1("error-login-check", "error", format!("{error:#}")),
                retry: ErrorRetry::RestartLogin,
            },
        }
    }

    fn finish_login(&mut self, tokens: AuthTokens) -> AppState {
        if let Err(error) = auth::save_tokens(&tokens) {
            eprintln!("Could not persist GFN login: {error:#}");
        }
        let user = match auth::user_from_tokens(&tokens) {
            Ok(user) => user,
            Err(error) => {
                return AppState::Error {
                    code: None,
                    message: self.tr1("error-profile-read", "error", format!("{error:#}")),
                    retry: ErrorRetry::RestartLogin,
                };
            }
        };
        self.vpc_id_cache = catalog::VpcIdCache::default();
        let state = Self::start_catalog_fetch(
            &self.http_client,
            &tokens,
            &self.vpc_id_cache,
            user,
            self.catalog_filter == CatalogFilter::MyGames,
        );
        self.tokens = Some(tokens);
        state
    }

    /// Pumps the in-game keyboard and forwards whatever it detected to the game.
    ///
    /// The IME needs `update` called every frame to deliver its events at all, and its handler can
    /// only queue keystrokes - it has no route to the peer - so the hand-off happens here.
    fn pump_keyboard(&mut self) {
        // The keyboard belongs to a running session; leaving it up over the catalog would send
        // keystrokes into a game that is no longer there.
        if crate::ime::is_open() && !matches!(self.state, AppState::Streaming { .. }) {
            crate::ime::close();
            return;
        }
        crate::ime::update();
        let keys = crate::ime::take_keys();
        if keys.is_empty() {
            return;
        }
        if let AppState::Streaming { peer, .. } = &self.state {
            for key in keys {
                peer.tap_key(key);
            }
        }
    }

    /// Keeps the saved login ahead of its expiry, so a request is never the thing that discovers
    /// the token died.
    ///
    /// Rate-limited rather than attempted every tick: `tick` runs 60 times a second, and a failing
    /// refresh must not turn into a request storm against NVIDIA's token endpoint.
    async fn maintain_session(&mut self) {
        let Some(tokens) = self.tokens.as_ref() else {
            return;
        };
        if !tokens.needs_maintenance() {
            return;
        }
        if self
            .last_refresh_attempt
            .is_some_and(|at| at.elapsed() < Self::REFRESH_RETRY_INTERVAL)
        {
            return;
        }
        let Some(user_id) = self.current_user_id() else {
            return;
        };
        self.last_refresh_attempt = Some(Instant::now());

        let tokens = tokens.clone();
        // `ensure_fresh_tokens` covers both halves of maintenance: renewing the access token, and
        // acquiring a client token for a login saved before this build knew to ask for one.
        match crate::gfn::auth::ensure_fresh_tokens(&self.http_client, &tokens, &user_id).await {
            Ok(refreshed) => self.tokens = Some(refreshed),
            Err(crate::gfn::auth::RefreshError::ReauthenticationRequired(message)) => {
                eprintln!("Saved GFN login can no longer be refreshed: {message}");
                // Nothing to do here beyond dropping the dead credential; the next authenticated
                // request surfaces the sign-in prompt through the usual error path.
                crate::gfn::auth::clear_tokens();
                self.tokens = None;
            }
            Err(crate::gfn::auth::RefreshError::Temporary(error)) => {
                // Keep the saved login and try again after the backoff.
                eprintln!("Deferring GFN token refresh: {error:#}");
            }
        }
    }

    /// How long to wait before retrying a refresh that did not succeed.
    const REFRESH_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

    /// The signed-in account's subject id, read back from the saved JWT.
    fn current_user_id(&self) -> Option<String> {
        let tokens = self.tokens.as_ref()?;
        crate::gfn::auth::user_from_tokens(tokens)
            .ok()
            .map(|user| user.user_id)
    }

    /// Renews the saved login in place, replacing `self.tokens` on success.
    ///
    /// The three outcomes are deliberately distinct: only a credential NVIDIA has actually
    /// rejected justifies discarding the saved login. Treating a timeout the same way is what made
    /// sessions feel like they expired after minutes.
    async fn refresh_session(&mut self, user: &GfnUser) -> SessionRefresh {
        let Some(tokens) = self.tokens.clone() else {
            return SessionRefresh::ReauthenticationRequired;
        };
        match crate::gfn::auth::refresh_tokens(&self.http_client, &tokens, &user.user_id).await {
            Ok(refreshed) => {
                if let Err(error) = crate::gfn::auth::save_tokens(&refreshed) {
                    eprintln!("Could not persist refreshed GFN tokens: {error:#}");
                }
                self.tokens = Some(refreshed);
                SessionRefresh::Renewed
            }
            Err(crate::gfn::auth::RefreshError::ReauthenticationRequired(message)) => {
                eprintln!("Saved GFN login can no longer be refreshed: {message}");
                SessionRefresh::ReauthenticationRequired
            }
            Err(crate::gfn::auth::RefreshError::Temporary(error)) => {
                SessionRefresh::Failed(format!("{error:#}"))
            }
        }
    }

    async fn advance_catalog_load(
        &mut self,
        user: GfnUser,
        job: PollJob<catalog::CatalogPage>,
    ) -> AppState {
        let PollJob::Pending(handle) = job else {
            return AppState::LoadingCatalog { user, job };
        };
        match poll_job(handle).await {
            PollJob::Pending(handle) => AppState::LoadingCatalog {
                user,
                job: PollJob::Pending(handle),
            },
            PollJob::Done(Ok(page)) => {
                self.paging.restart(String::new(), &page);
                // Starred games the catalog did not page in are folded in here, so they have a row
                // to be sorted into. Only on the browse load: a search should return what matches.
                let games = merge_favorites(page.games, &self.favorite_games);
                let filtered_indices = filter_indices_with_favorites(
                    &games,
                    "",
                    self.catalog_sort,
                    &self.favorites,
                );
                AppState::Catalog {
                    user,
                    games,
                    selected: 0,
                    filtered_indices,
                    search_query: String::new(),
                    search_requested: false,
                    covers: CoverStore::new(),
                }
            }
            PollJob::Done(Err(error)) => {
                let err_str = format!("{error:#}");
                // check code first, text fallback is only for the graphql endpoint which
                // doesnt give us a real requestStatus
                if gfn_error_code(&error).is_some_and(|code| code.needs_reauth())
                    || err_str.contains("401 Unauthorized")
                    || err_str.contains("Invalid or expired token")
                {
                    // An expired access token is the common case here and it is recoverable, so
                    // try to renew it before falling back to making the player sign in again.
                    match self.refresh_session(&user).await {
                        SessionRefresh::Renewed => {
                            let tokens = self
                                .tokens
                                .as_ref()
                                .expect("refresh stored the renewed tokens");
                            self.vpc_id_cache = catalog::VpcIdCache::default();
                            return Self::start_catalog_fetch(
                                &self.http_client,
                                tokens,
                                &self.vpc_id_cache,
                                user,
                                self.catalog_filter == CatalogFilter::MyGames,
                            );
                        }
                        SessionRefresh::ReauthenticationRequired => {
                            crate::gfn::auth::clear_tokens();
                            self.tokens = None;
                            self.vpc_id_cache = catalog::VpcIdCache::default();
                            return AppState::Error {
                                code: None,
                                message: self.tr("error-session-expired"),
                                retry: ErrorRetry::RestartLogin,
                            };
                        }
                        // Keep the saved login: a flaky connection must not cost a re-scan.
                        SessionRefresh::Failed(message) => {
                            return AppState::Error {
                                code: None,
                                message: self.tr1("error-catalog-load", "error", &message),
                                retry: ErrorRetry::ReloadCatalog(user),
                            };
                        }
                    }
                } else {
                    AppState::Error {
                        code: None,
                        message: self.tr1("error-catalog-load", "error", &err_str),
                        retry: ErrorRetry::ReloadCatalog(user),
                    }
                }
            }
        }
    }

    async fn advance_session_creation(
        locale: Locale,
        user: GfnUser,
        games: Vec<GameSummary>,
        selected: usize,
        filtered_indices: Vec<usize>,
        search_query: String,
        search_requested: bool,
        covers: CoverStore,
        job: PollJob<SessionInfo>,
        queue_tracker: cloudmatch::QueueProgressTracker,
    ) -> AppState {
        let PollJob::Pending(handle) = job else {
            return AppState::CreatingSession {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested,
                covers,
                job,
                queue_tracker,
            };
        };
        match poll_job(handle).await {
            PollJob::Pending(handle) => AppState::CreatingSession {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested,
                covers,
                job: PollJob::Pending(handle),
                queue_tracker,
            },
            // Straight into signaling rather than parking on a "press X to start" screen. The
            // player already chose this game and sat through the queue; asking again is a second
            // confirmation for a decision nobody changed their mind about. `SessionReady` is still
            // reachable - it is where a failed connect lands, so the prompt becomes a retry.
            PollJob::Done(Ok(session)) => {
                match signaling::connect(&session.signaling_url, &session.session_id) {
                    Ok(handle) => AppState::Signaling {
                        user,
                        games,
                        selected,
                        filtered_indices,
                        search_query,
                        search_requested,
                        covers,
                        session,
                        handle,
                        offer_sdp: None,
                    },
                    Err(_) => AppState::SessionReady {
                        user,
                        games,
                        selected,
                        filtered_indices,
                        search_query,
                        search_requested,
                        covers,
                        session,
                    },
                }
            }
            PollJob::Done(Err(error)) => AppState::Error {
                // grab the code before it gets flattened into a string
                code: gfn_error_code(&error),
                message: tr(locale, "error-session-create", "error", format!("{error:#}")),
                retry: ErrorRetry::BackToCatalog {
                    user,
                    games,
                    selected,
                    filtered_indices,
                    search_query,
                    search_requested,
                    covers,
                },
            },
        }
    }
}

#[cfg(test)]
mod catalog_order_tests {
    use super::*;
    use std::collections::BTreeSet;

    fn game(app_id: &str, title: &str, played: bool) -> GameSummary {
        GameSummary {
            app_id: app_id.to_owned(),
            title: title.to_owned(),
            cover_url: None,
            store: None,
            last_played: played.then(|| "2026-07-30T00:00:00Z".to_owned()),
            search_key: title.to_lowercase(),
        }
    }

    fn ids(games: &[GameSummary], indices: &[usize]) -> Vec<String> {
        indices.iter().map(|&i| games[i].app_id.clone()).collect()
    }

    /// The whole point of the change: starred first, then played recently, then the rest.
    #[test]
    fn browsing_groups_favorites_then_recent_then_the_rest() {
        let games = vec![
            game("plain", "Plain", false),
            game("recent", "Recent", true),
            game("starred", "Starred", false),
        ];
        let favorites: BTreeSet<String> = ["starred".to_owned()].into_iter().collect();

        let indices =
            filter_indices_with_favorites(&games, "", CatalogSort::TitleAsc, &favorites);
        assert_eq!(ids(&games, &indices), ["starred", "recent", "plain"]);
    }

    /// A starred game that was also played recently belongs in the starred band, not both.
    #[test]
    fn a_starred_recent_game_sorts_as_starred() {
        let games = vec![game("other", "Other", true), game("both", "Both", true)];
        let favorites: BTreeSet<String> = ["both".to_owned()].into_iter().collect();
        let indices =
            filter_indices_with_favorites(&games, "", CatalogSort::TitleAsc, &favorites);
        assert_eq!(ids(&games, &indices), ["both", "other"]);
    }

    /// Grouping must not disturb the order the chosen sort produced inside each band.
    #[test]
    fn grouping_is_stable_within_a_band() {
        let games = vec![
            game("c", "C", false),
            game("a", "A", false),
            game("b", "B", false),
        ];
        let indices =
            filter_indices_with_favorites(&games, "", CatalogSort::TitleAsc, &BTreeSet::new());
        assert_eq!(ids(&games, &indices), ["a", "b", "c"]);
    }

    /// The complaint this replaces: favourites were being floated into search results, putting
    /// games at the top that the player had not asked for.
    #[test]
    fn searching_does_not_pin_favorites() {
        let games = vec![
            game("alpha", "Alpha Quest", false),
            game("beta", "Beta Quest", false),
        ];
        let favorites: BTreeSet<String> = ["beta".to_owned()].into_iter().collect();

        let indices =
            filter_indices_with_favorites(&games, "quest", CatalogSort::TitleAsc, &favorites);
        assert_eq!(
            ids(&games, &indices),
            ["alpha", "beta"],
            "search results should stay in their own order"
        );
    }

    /// A favourite past the catalog's 1000-title cut-off has no row until it is merged in.
    #[test]
    fn merging_adds_favorites_the_catalog_never_paged_in() {
        let games = vec![game("loaded", "Loaded", false)];
        let stored = vec![crate::gfn::favorites::FavoriteGame {
            app_id: "unpaged".to_owned(),
            title: "Unpaged".to_owned(),
            cover_url: None,
            store: None,
        }];
        let merged = merge_favorites(games, &stored);
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|game| game.app_id == "unpaged"));
    }

    #[test]
    fn merging_does_not_duplicate_a_favorite_already_present() {
        let games = vec![game("loaded", "Loaded", false)];
        let stored = vec![crate::gfn::favorites::FavoriteGame {
            app_id: "loaded".to_owned(),
            title: "Loaded".to_owned(),
            cover_url: None,
            store: None,
        }];
        assert_eq!(merge_favorites(games, &stored).len(), 1);
    }
}
