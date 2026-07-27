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
    /// played pushed to the end in whatever order they already had. Mirrors OpenNOW/GFN's own
    /// default "Last Played" library view - the default here too.
    #[default]
    LastPlayed,
    /// GFN's own server-side ranking (relevance + name) - the order `games` already arrives
    /// in, so this is a no-op past filtering.
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
}

/// How many catalog pages (`CATALOG_PAGE_SIZE` titles each) are walked before we stop. Pages
/// stream in behind the UI, so this trades background requests on the Vita's single Wi-Fi radio
/// for how much of the catalog is reachable by scrolling. ~1000 titles is about 400 KiB of
/// `GameSummary`, negligible next to one decoded cover.
///
/// Server-side search reaches the *whole* catalog regardless of this cap, so it only bounds
/// browsing. The reference client stops at 3 pages of 120.
const MAX_CATALOG_PAGES: usize = 5;

/// Which local filter to apply on top of a set of server results.
///
/// When the server answered the query that's still in the search box, its results are already
/// the answer - and it matched on more than the title (publisher, aliases), so re-applying our
/// title-only filter would *drop* legitimate hits. Once the user types further, the extra
/// characters haven't been sent yet, so narrowing locally is exactly right.
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
    /// The server query these pages belong to (`""` = plain browse). Also what
    /// `effective_local_query` compares the search box against.
    server_query: String,
    /// Cursor for the next page, `None` once the server says there are no more.
    next_cursor: Option<String>,
    pages_loaded: usize,
    total_count: Option<usize>,
    /// In-flight next-page fetch, tagged with the `generation` it was spawned under.
    job: Option<(u64, PollJob<catalog::CatalogPage>)>,
    /// Bumped whenever `games` is replaced wholesale (new search, reload). `PollJob` carries no
    /// identity of its own, so this is what stops page 2 of a superseded query being appended
    /// onto a different result set - which would silently corrupt the list rather than fail.
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
/// `sort`. An empty query keeps every index before sorting.
///
/// Runs on every keystroke, so it leans entirely on `GameSummary::search_key` (lowercased once
/// at parse time): the needle is lowercased once per call and nothing else allocates.
fn filter_indices(games: &[GameSummary], query: &str, sort: CatalogSort) -> Vec<usize> {
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
        // Unstable is fine here: `search_key` ties are titles that differ only by case, whose
        // relative order isn't meaningful.
        CatalogSort::TitleAsc => {
            indices.sort_unstable_by(|&a, &b| games[a].search_key.cmp(&games[b].search_key))
        }
        CatalogSort::TitleDesc => {
            indices.sort_unstable_by(|&a, &b| games[b].search_key.cmp(&games[a].search_key))
        }
        CatalogSort::LastPlayed => {
            // `last_played` is an ISO-8601 string, so plain string comparison already sorts
            // chronologically. `None` (never played) sorts after every `Some`, keeping its
            // relative order (stable sort) rather than being interleaved by title.
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
    /// Later pages stream in from `advance_catalog_paging` once we're already on `Catalog`.
    LoadingCatalog {
        user: GfnUser,
        job: PollJob<catalog::CatalogPage>,
    },
    /// Fase 2 stops here - Fase 3 adds actually creating a streaming session for the selected
    /// game (`games[selected].app_id`) instead of just showing a placeholder note.
    Catalog {
        user: GfnUser,
        games: Vec<GameSummary>,
        /// `selected` indexes into `filtered_indices`, not directly into `games`.
        selected: usize,
        /// Indices into `games` that match the current `search_query`. Empty query = all games.
        filtered_indices: Vec<usize>,
        search_query: String,
        /// Set to `true` when the user presses the search button; the shell uses SDL's text input
        /// API to open the system/on-screen keyboard and feed the resulting text back via
        /// `AppCommand::SetSearchQuery`.
        search_requested: bool,
        /// Shared cover-art cache: lazily filled by async download tasks spawned from the UI
        /// loop as tiles become visible (see `app::ui::catalog_screen`). The `Arc` lets the
        /// store outlive the originating `AppState::Catalog` - inflight downloads keep their
        /// last reference and complete into an orphaned map that gets GC'd when the task ends,
        /// with no risk of holding stale references in `App::state` after it transitions away
        /// from this screen.
        covers: CoverStore,
    },
    /// CloudMatch session creation + polling in progress. Spawned from `Catalog` when the
    /// user presses Confirm (or taps PLAY) to launch the selected game.
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
    /// CloudMatch session is ready. This is a debug/transition screen: it shows the resolved
    /// server IP, signaling server, and codec profile, and is the launchpad for the WebRTC
    /// signaling step.
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
    /// Connected to the session's NVST signaling WebSocket. `offer_sdp` fills in once the server
    /// sends its offer; this is as far as Fase 3's signaling step goes today - actually building
    /// a `rtc` peer connection from that offer is the next commit.
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
    },
}

pub struct App {
    pub(crate) state: AppState,
    /// Used both for GFN REST/GraphQL calls (from the async `AppState` tasks below) and - via
    /// `app::ui::build_ui`, which also borrows `&App` - for the per-frame lazy cover-art
    /// download requests kicked off from the catalog grid renderer.
    pub(crate) http_client: Client,
    /// Set on every successful (or restored) login, cleared on sign-out. Fase 3's session
    /// creation will need this too, so it lives on `App` rather than threaded through every
    /// `AppState` variant that happens to run after login.
    tokens: Option<AuthTokens>,
    /// Debug readout of the last navigation command received, shown on the placeholder screen
    /// so input mapping can be sanity-checked on real hardware before there is anything else to
    /// look at.
    pub(crate) last_input: Option<InputCommand>,
    /// Transient one-line status message (e.g. "press Confirm on a game does X once Fase 3
    /// lands"), shown under the game list until the next input event replaces or clears it.
    pub(crate) status_note: Option<String>,
    /// Debounce/dispatch state for server-side catalog search. Deliberately kept on `App`
    /// instead of inside `AppState::Catalog` - the query text and its instant local pre-filter
    /// already live there (see `apply_search_query`), but threading this through would mean
    /// touching every one of that variant's many match arms just to move fields they don't care
    /// about. Cleared (and any in-flight job left to finish orphaned) whenever the current state
    /// isn't `AppState::Catalog` - see `advance_catalog_search`.
    /// The in-flight server search paired with the query it was dispatched for, so a result that
    /// arrives after the user has typed further can be recognized as stale and dropped instead of
    /// clobbering `games` and resetting the selection.
    search_job: Option<(String, PollJob<catalog::CatalogPage>)>,
    /// Set when the query changed and a debounced server search hasn't fired for it yet.
    search_pending_since: Option<Instant>,
    /// The last query a server search was actually dispatched for - avoids re-firing once the
    /// debounce elapses if the user hasn't typed anything new since.
    last_dispatched_search_query: Option<String>,
    pub(crate) confirm_exit: bool,
    /// UI display language, changed via the gear icon next to the avatar in the catalog
    /// screen. Currently only the language picker itself reads/writes this - most of the UI
    /// still has its Spanish strings hardcoded (see `src/i18n.rs` for the (unwired) fluent
    /// setup that would translate the rest).
    pub(crate) locale: Locale,
    /// Library sort order, changed via the sort dropdown next to the library header.
    pub(crate) catalog_sort: CatalogSort,
    /// Cached account VPC id, shared with every spawned catalog task so the id is fetched once
    /// per session instead of before every catalog/search request.
    vpc_id_cache: catalog::VpcIdCache,
    /// Background cursor-pagination state for the catalog list.
    paging: CatalogPaging,
}

impl App {
    /// Returns the current Bearer token if the user is logged in.
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
        let http_client = auth::client();
        let tokens = auth::load_tokens();
        let vpc_id_cache = catalog::VpcIdCache::default();
        let state = match &tokens {
            Some(tokens) => match auth::user_from_tokens(tokens) {
                Ok(user) => Self::start_catalog_fetch(&http_client, tokens, &vpc_id_cache, user),
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
            last_input: None,
            status_note: None,
            search_job: None,
            search_pending_since: None,
            last_dispatched_search_query: None,
            confirm_exit: false,
            locale: Locale::default(),
            catalog_sort: CatalogSort::default(),
            vpc_id_cache,
            paging: CatalogPaging::default(),
        })
    }

    pub async fn handle_command(&mut self, command: AppCommand) -> Result<()> {
        // Snapshot these up front so the match arms can move `self` references freely without
        // holding a borrow across the state reassignment.
        let bearer_token = self.bearer_token().map(|s| s.to_owned());
        let http_client = self.http_client.clone();

        // Takes ownership of the current state up front rather than matching on `&mut
        // self.state` directly - some arms below need to both read out of the matched state
        // (e.g. `ReloadCatalog(user)`) and reassign `self.state`, which the borrow checker
        // won't allow through a live reference into the same field.
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
            AppCommand::SelectGame(index) => {
                // `current_state` (not `self.state`) is the live state here - it was moved out
                // by the `mem::replace` above.
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
                        // Must go through `effective_local_query`: passing `search_query`
                        // unconditionally re-narrowed a *server* result set to title matches
                        // only, silently dropping rows the server matched on publisher/alias.
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
                // Reset selection to the first matching result whenever the query changes.
                let selected = 0;
                // Arms the debounce timer for a server-side search - see `advance_catalog_search`.
                // Cleared once that search actually dispatches, not here, so rapid keystrokes
                // keep pushing the timer back instead of firing one request per character.
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
    /// method (SDL IME / on-screen keyboard). Reset to false once the query actually arrives via
    /// `SetSearchQuery`.
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

    /// Tells CloudMatch to release the session, fired off as a background task so the caller
    /// (an exit button press, or a disconnect being turned into an error screen) never blocks
    /// on it. Best-effort: `cloudmatch::stop_session` itself swallows and logs failures rather
    /// than surfacing them, since there is no user-facing action left to retry from here.
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
                ..
            } => AppState::Catalog {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested,
                covers,
            },
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
                // Order matters: close the signaling socket (and, for Streaming, drop `peer` -
                // implicit here since it's matched away by `..` - which stops its background
                // thread and releases the direct-video textures/CDRAM) before telling CloudMatch
                // the session is over, so nothing keeps writing to a session we've just told the
                // server to tear down.
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
                    // While the system keyboard is open let the platform handle d-pad/stick input.
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
                    // Close the system keyboard and return to list navigation.
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
                    // Launch the selected game: kick off the CloudMatch session creation flow.
                    // This hits NVIDIA's REST API and polls until the session reports ready.
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
                            let handle: JoinHandle<Result<SessionInfo>> =
                                tokio::spawn(async move {
                                    let settings = cloudmatch::StreamSettings::for_vita();
                                    let session = cloudmatch::create_session(
                                        &http_client,
                                        cloudmatch::CreateSessionRequest {
                                            token: token.as_str(),
                                            app_id: &app_id,
                                            vpc_id: "", // VPC id is not required by the v2/session endpoint; serverInfo is optional for MVP.
                                            settings: &settings,
                                        },
                                    )
                                    .await?;
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
                                    // `create_session` already seated a session on NVIDIA's side;
                                    // if polling then fails we must hand it back. Without this,
                                    // every failed launch leaks a live session against the
                                    // account, and since GFN caps concurrent sessions the next
                                    // attempt is *more* likely to fail - which shows up as
                                    // escalating HTTP 503s that look like an NVIDIA outage.
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
                                Some("No se pudo iniciar sesion: falta login o juego.".to_owned());
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
                if search_requested {
                    // Close the system keyboard without leaving the catalog.
                    AppState::Catalog {
                        user,
                        games,
                        selected,
                        filtered_indices,
                        search_query,
                        search_requested: false,
                        covers,
                    }
                } else if !search_query.is_empty() {
                    // Clear the search query and restore full list
                    let new_query = String::new();
                    let new_filtered = filter_indices(&games, "", self.catalog_sort);
                    // Crucial: trigger a server search for the empty string to restore full catalog
                    self.search_pending_since = Some(std::time::Instant::now());
                    AppState::Catalog {
                        user,
                        games,
                        selected: 0,
                        filtered_indices: new_filtered,
                        search_query: new_query,
                        search_requested: false,
                        covers,
                    }
                } else {
                    // Back in the catalog no longer signs out immediately - too easy to hit by
                    // accident. Sign-out will live in a dedicated menu later.
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
                        Some("Conectando a la señalización de NVIDIA...".to_owned());
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
                        Some(format!("No se pudo conectar la señalización: {error:#}"));
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
                    retry: ErrorRetry::RestartLogin,
                    ..
                },
                InputCommand::Confirm,
            ) => self.start_login_state(),
            (
                AppState::Error {
                    retry: ErrorRetry::ReloadCatalog(user),
                    ..
                },
                InputCommand::Confirm,
            ) => Self::start_catalog_fetch(
                &self.http_client,
                self.tokens.as_ref().expect("retry requires a saved login"),
                &self.vpc_id_cache,
                user,
            ),
            (
                AppState::Error {
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
            (AppState::Error { .. }, InputCommand::Back) => AppState::Login,
            (other, _) => other,
        })
    }

    fn start_login_state(&self) -> AppState {
        let client = self.http_client.clone();
        let handle: JoinHandle<Result<DeviceCodeChallenge>> =
            tokio::spawn(async move { auth::start_device_login(&client).await });
        AppState::StartingDeviceLogin(PollJob::Pending(handle))
    }

    /// Kicks off the catalog load. Only **page 1** is fetched here, so the catalog screen appears
    /// as soon as the first 200 titles land; `advance_catalog_paging` streams the rest in behind
    /// the UI.
    fn start_catalog_fetch(
        client: &Client,
        tokens: &AuthTokens,
        vpc_id_cache: &catalog::VpcIdCache,
        user: GfnUser,
    ) -> AppState {
        let client = client.clone();
        let bearer = tokens.bearer().to_owned();
        let cache = vpc_id_cache.clone();
        let handle: JoinHandle<Result<catalog::CatalogPage>> = tokio::spawn(async move {
            catalog::fetch_catalog_page_for_account(&client, &bearer, &cache, None, "").await
        });
        AppState::LoadingCatalog {
            user,
            job: PollJob::Pending(handle),
        }
    }

    /// How long to wait after the last keystroke before actually hitting the network. Long enough
    /// that typing doesn't fire one request per character, short enough that results still feel
    /// immediate. The reference client uses 220ms for a desktop keyboard; 250ms lands on a clean
    /// ~16-frame boundary given the loop only samples this timer once per frame, and the Vita's
    /// on-screen keyboard is slower between keystrokes anyway.
    ///
    /// No minimum query length: real titles are short ("R6", "GTA"), and
    /// `last_dispatched_search_query` already suppresses redundant fires.
    const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);

    /// Drives server-side catalog search: polls any in-flight search job to completion, then -
    /// once the debounce timer has elapsed for the current query and that query hasn't already
    /// been dispatched - fires a new one. A no-op whenever the current screen isn't
    /// `AppState::Catalog`.
    ///
    /// Complements (does not replace) the instant local `filter_indices` pre-filter in
    /// `apply_search_query`/`search_backspace`: those give immediate feedback within whatever
    /// page is already loaded, while this widens `games`/`filtered_indices` in place to the
    /// server's full-catalog match once it comes back - see `docs/protocol-notes.md`-adjacent
    /// comments on `catalog::search_catalog` for why a bigger local page isn't a substitute.
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
                // A result for a query the user has since typed past is worse than no result: it
                // would replace `games` with the wrong set and reset the selection. Drop it and
                // let the debounce dispatch the current query.
                PollJob::Done(_) if job_query != query => {}
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
                        // Server results are already the answer for this query - see
                        // `effective_local_query`.
                        *filtered_indices = filter_indices(&page.games, "", self.catalog_sort);
                        *games = page.games;
                        *selected = 0;
                    }
                    self.status_note = Some(format!("{result_count} resultado(s) para \"{query}\""));
                }
                PollJob::Done(Err(error)) => {
                    self.status_note = Some(format!("Búsqueda falló: {error:#}"));
                }
            }
            // Only ever one search in flight at a time - don't also consider dispatching a new
            // one in the same tick we just resolved this one.
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
        let Some(token) = self.bearer_token().map(str::to_owned) else {
            return;
        };

        // A pending page belongs to the query we're about to replace - stop it before it can
        // append onto a different result set.
        self.paging.abort_job();
        self.search_pending_since = None;
        self.last_dispatched_search_query = Some(query.clone());
        let client = self.http_client.clone();
        let cache = self.vpc_id_cache.clone();
        let dispatched = query.clone();
        let handle: JoinHandle<Result<catalog::CatalogPage>> = tokio::spawn(async move {
            let trimmed = dispatched.trim();
            let server_query = (!trimmed.is_empty()).then_some(trimmed);
            catalog::fetch_catalog_page_for_account(&client, &token, &cache, server_query, "").await
        });
        self.search_job = Some((query, PollJob::Pending(handle)));
    }

    /// Streams the remaining catalog pages in behind the UI, appending each to `games` as it
    /// lands so the list grows while the user browses.
    ///
    /// Deliberately yields to search: no page is dispatched while a search is in flight or its
    /// debounce is armed, because on a console with one Wi-Fi radio a background page competing
    /// with the query the user is actively typing feels worse than the shorter list did.
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
                // Superseded by a newer query - `games` is a different set now, so appending
                // this page would corrupt the list.
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
                    // Non-fatal: the user keeps whatever pages already landed.
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
        let handle: JoinHandle<Result<catalog::CatalogPage>> = tokio::spawn(async move {
            let trimmed = server_query.trim();
            let query = (!trimmed.is_empty()).then_some(trimmed);
            catalog::fetch_catalog_page_for_account(&client, &token, &cache, query, &cursor).await
        });
        self.paging.job = Some((generation, PollJob::Pending(handle)));
    }

    /// Appends a freshly fetched page to the catalog, keeping the highlighted title highlighted.
    ///
    /// `games` is append-only within a paging generation, which makes a `games` index stable
    /// identity for the selected title - whereas its position in `filtered_indices` can move as
    /// soon as new titles sort in above it. So we record the selection as a `games` index first
    /// and look it back up afterwards.
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
        // Cursor pagination shouldn't overlap, but a duplicate would be more than cosmetic:
        // `app_id` is the egui texture key for covers, so two rows sharing one id would look
        // like a rendering bug.
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

    /// Bounds how much decoded cover art stays resident, pruned on every tick.
    ///
    /// On the catalog screen the selected title's cover is pinned and a small LRU of recently
    /// visited ones is kept, so stepping back up the list is instant. Everywhere else - i.e.
    /// once a streaming session is being set up - every cover is released: that frees the RGBA
    /// buffers *and* their VRAM textures before `PeerEngine` claims the direct-video textures
    /// and the decoder's CDRAM, which is the app's peak memory pressure.
    ///
    /// Must run before the `mem::replace` in `tick`, since it reads the real `self.state`.
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
            AppState::CreatingSession { covers, .. }
            | AppState::SessionReady { covers, .. }
            | AppState::Signaling { covers, .. }
            | AppState::Streaming { covers, .. } => covers.prune(None, 0),
            _ => {}
        }
    }

    /// Per-frame housekeeping: advances whatever async step is in flight. Kept out of the render
    /// closure so `build_ui` stays a pure function of the
    /// current state.
    pub async fn tick(&mut self) -> Result<()> {
        self.prune_covers();
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
                self.state = Self::advance_session_creation(
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

                // Signaling stays alive during streaming: it still trickles NVIDIA's ICE
                // candidates (forwarded into the peer) and carries our answer out. Losing it
                // mid-stream means no more renegotiation is possible, so treat it the same as
                // the peer itself dying.
                while let Some(event) = handle.try_recv() {
                    match event {
                        SignalingEvent::RemoteIce(candidate) => {
                            peer.add_remote_ice(candidate);
                        }
                        SignalingEvent::Disconnected(reason) => {
                            fatal_reason.get_or_insert(format!("Señalización perdida: {reason}"));
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
                            self.status_note = Some("Transmisión de vídeo en directo activa".to_owned());
                        }
                        crate::gfn::peer::PeerEvent::Error(err) => {
                            // Non-fatal (e.g. a rejected trickled ICE candidate, or the
                            // hardware decoder being unavailable): surfaced for diagnostics,
                            // but the session may still recover on its own.
                            eprintln!("Streaming peer error: {err}");
                            self.status_note = Some(format!("Peer: {err}"));
                        }
                        crate::gfn::peer::PeerEvent::Disconnected(reason) => {
                            eprintln!("Streaming peer disconnected: {reason}");
                            fatal_reason
                                .get_or_insert(format!("Conexión de streaming perdida: {reason}"));
                            break;
                        }
                    }
                }

                if let Some(message) = fatal_reason {
                    // Order matters, same as a user-initiated exit: stop the signaling socket
                    // (`peer` is dropped right here along with it, since this match arm doesn't
                    // bind it further - that runs `PeerEngine::drop`, which stops its thread and
                    // releases the direct-video textures/CDRAM) before telling CloudMatch the
                    // session is over.
                    handle.close();
                    self.stop_cloudmatch_session(&session);
                    self.state = AppState::Error {
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
                        Some("Señalización conectada, esperando offer SDP...".to_owned());
                }
                Some(SignalingEvent::Offer(sdp)) => {
                    self.status_note = Some(format!(
                        "Offer SDP recibido ({} bytes). Negociando WebRTC...",
                        sdp.len()
                    ));
                    // The peer thread generates the real answer (and its NVST blob) and emits
                    // it as `PeerEvent::LocalAnswer`; `advance_streaming` forwards it through
                    // this same signaling handle. Any ICE candidates still queued behind the
                    // offer are drained next tick by the Streaming arm.
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
                    self.status_note = Some(format!(
                        "Candidato ICE remoto recibido de NVIDIA: {}",
                        candidate.candidate
                    ));
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
                message: format!("Señalización desconectada: {reason}"),
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
                message: format!("No se pudo iniciar sesión: {error:#}"),
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
                message: "El código expiró antes de completar el login. Inténtalo de nuevo."
                    .to_owned(),
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
                message: "El código expiró antes de completar el login. Inténtalo de nuevo."
                    .to_owned(),
                retry: ErrorRetry::RestartLogin,
            },
            PollJob::Done(Ok(DevicePollOutcome::Denied)) => AppState::Error {
                message: "Inicio de sesión rechazado.".to_owned(),
                retry: ErrorRetry::RestartLogin,
            },
            PollJob::Done(Err(error)) => AppState::Error {
                message: format!("Fallo comprobando el login: {error:#}"),
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
                    message: format!("Login correcto pero no se pudo leer el perfil: {error:#}"),
                    retry: ErrorRetry::RestartLogin,
                };
            }
        };
        // The VPC id is per-account: a different login must not inherit the previous one.
        self.vpc_id_cache = catalog::VpcIdCache::default();
        let state =
            Self::start_catalog_fetch(&self.http_client, &tokens, &self.vpc_id_cache, user);
        self.tokens = Some(tokens);
        state
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
                // Seed paging from page 1 so `advance_catalog_paging` can stream the rest.
                self.paging.restart(String::new(), &page);
                let filtered_indices = filter_indices(&page.games, "", self.catalog_sort);
                AppState::Catalog {
                    user,
                    games: page.games,
                    selected: 0,
                    filtered_indices,
                    search_query: String::new(),
                    search_requested: false,
                    covers: CoverStore::new(),
                }
            }
            PollJob::Done(Err(error)) => {
                let err_str = format!("{error:#}");
                if err_str.contains("401 Unauthorized") || err_str.contains("Invalid or expired token") {
                    crate::gfn::auth::clear_tokens();
                    // Tokens are gone, so the cached id no longer belongs to anyone.
                    self.vpc_id_cache = catalog::VpcIdCache::default();
                    AppState::Error {
                        message: "Tu sesion ha expirado. Por favor, vuelve a iniciar sesion.".to_owned(),
                        retry: ErrorRetry::RestartLogin,
                    }
                } else {
                    AppState::Error {
                        message: format!("No se pudo cargar tu biblioteca de juegos: {err_str}"),
                        retry: ErrorRetry::ReloadCatalog(user),
                    }
                }
            }
        }
    }

    async fn advance_session_creation(
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
            PollJob::Done(Ok(session)) => AppState::SessionReady {
                user,
                games,
                selected,
                filtered_indices,
                search_query,
                search_requested,
                covers,
                session,
            },
            PollJob::Done(Err(error)) => AppState::Error {
                message: format!("No se pudo crear la sesión de streaming: {error:#}"),
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
