//! CloudMatch REST API — creates, polls and stops a GFN streaming session.
#![allow(dead_code)]
//! Reference: `opennow-stable/src/main/gfn/cloudmatch.ts` and `protocol.rs` in the OpenNOW native
//! streamer.

use super::active_session;
use super::error_codes::{GfnError, GfnErrorCode};
use super::headers::{self, error_for_status_with_body};
use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

const DEFAULT_CLOUDMATCH_BASE_URL: &str = "https://prod.cloudmatchbeta.nvidiagrid.net/";
const DEFAULT_LOCALE: &str = "en_US";
const DEFAULT_KEYBOARD_LAYOUT: &str = "us";

/// Per-session client identity.
#[derive(Debug, Clone)]
pub struct SessionIdentity {
    pub client_id: String,
    pub device_id: String,
}

/// Settings chosen for the Vita's hardware limits.
#[derive(Debug, Clone)]
pub struct StreamSettings {
    pub resolution: String,
    pub fps: u32,
    pub max_bitrate_mbps: u32,
}

impl StreamSettings {
    /// Vita profile: 960x544 native, H.264 (forced later via `codec` field). Frame rate is the
    /// player's choice (`gfn::stream_prefs`) and the bitrate ceiling comes from what the last
    /// measured session actually achieved (`gfn::link_estimate`).
    pub fn for_vita() -> Self {
        Self {
            resolution: "960x544".to_owned(),
            fps: super::stream_prefs::fps().value(),
            max_bitrate_mbps: super::link_estimate::ceiling_mbps(),
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        let mut parts = self.resolution.split('x');
        let width = parts.next().and_then(|s| s.parse().ok()).unwrap_or(960);
        let height = parts.next().and_then(|s| s.parse().ok()).unwrap_or(544);
        (width, height)
    }
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub app_id: String,
    pub status: u32,
    pub server_ip: String,
    pub signaling_server: String,
    pub signaling_url: String,
    pub client_id: String,
    pub device_id: String,
    pub streaming_base_url: String,
    pub identity: SessionIdentity,
    pub media_connection_info: Option<MediaConnectionInfo>,
    pub ice_servers: Vec<IceServer>,
    pub negotiated_stream_profile: Option<NegotiatedStreamProfile>,
}

#[derive(Debug, Clone)]
pub struct MediaConnectionInfo {
    pub ip: String,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NegotiatedStreamProfile {
    pub resolution: Option<String>,
    pub fps: Option<u32>,
    pub codec: Option<String>,
}

/// CloudMatch session creation request.
pub struct CreateSessionRequest<'a> {
    pub token: &'a str,
    pub app_id: &'a str,
    pub vpc_id: &'a str,
    pub settings: &'a StreamSettings,
}

/// CloudMatch session poll request.
pub struct PollSessionRequest<'a> {
    pub token: &'a str,
    pub session_id: &'a str,
    pub session: &'a SessionInfo,
}

/// Creates a CloudMatch session and returns the initial `SessionInfo`.
pub async fn create_session(
    client: &Client,
    request: CreateSessionRequest<'_>,
) -> Result<SessionInfo> {
    // device id is the per-install one from sign-in now, used to be a UUIDv5 of a fixed
    // string which was the same on every vita so nvidia refused to DELETE stale sessions.
    // same fix as OpenNOW-Switch's GenerateDeviceId. client id stays per-launch tho
    let identity = SessionIdentity {
        client_id: uuid::Uuid::new_v4().to_string(),
        device_id: super::auth::device_id(),
    };
    let base_url = DEFAULT_CLOUDMATCH_BASE_URL.trim_end_matches('/');
    let (width, height) = request.settings.dimensions();

    let body = build_session_request_body(
        request.app_id,
        &identity.device_id,
        width,
        height,
        request.settings.fps,
    );
    let url = format!(
        "{base_url}/v2/session?keyboardLayout={DEFAULT_KEYBOARD_LAYOUT}&languageCode={DEFAULT_LOCALE}"
    );

    // Clear the decks first. Anything still open would reject this launch anyway, and finding that
    // out from a 403 costs the player a failed launch and two cleanup rounds.
    //
    // our own note goes first, its the only thing that survives a crash and knows the zone
    stop_remembered_session(client, request.token, &identity).await;
    stop_active_sessions_before_launch(client, request.token, &identity, base_url).await;

    let send_request = || async {
        let mut last_err = None;
        let mut throttled = 0u32;
        for _retry in 0..6 {
            let response = headers::apply_cloudmatch_headers(
                client.post(&url),
                request.token,
                &identity.client_id,
                &identity.device_id,
            )
            .header("Connection", "close")
            .json(&body)
            .send()
            .await;

            match response {
                Ok(resp) => {
                    let status = resp.status();
                    let headers = resp.headers().clone();
                    let body_text = resp.text().await.unwrap_or_default();

                    if status.is_success() {
                        let payload: CloudMatchResponse = serde_json::from_str(&body_text)
                            .context("failed to decode CloudMatch create session response")?;
                        if payload.request_status.status_code == 1 {
                            return Ok((payload, false));
                        }
                        if payload.request_status.is_session_limit()
                            && stop_conflicting_sessions(
                                client,
                                request.token,
                                Some(&payload),
                                &identity,
                                base_url,
                            )
                            .await
                        {
                            return Ok((payload, true));
                        }
                        return Err(payload.request_status.to_error(format!(
                            "CloudMatch create session error {} ({}): {body_text}",
                            payload.request_status.status_code,
                            payload.request_status.describe()
                        ))
                        .with_http_status(status.as_u16())
                        .into());
                    }

                    if status == reqwest::StatusCode::FORBIDDEN
                        || body_text.to_ascii_uppercase().contains("SESSION_LIMIT")
                    {
                        // still try cleanup even if body doesnt decode, falls back to
                        // asking cloudmatch whats open
                        let limit_payload =
                            serde_json::from_str::<CloudMatchResponse>(&body_text).ok();
                        if stop_conflicting_sessions(
                            client,
                            request.token,
                            limit_payload.as_ref(),
                            &identity,
                            base_url,
                        )
                        .await
                            && let Some(limit_payload) = limit_payload
                        {
                            return Ok((limit_payload, true));
                        }
                        if let Some(limit_payload) = limit_payload {
                            return Err(limit_payload.request_status.to_error(format!(
                                "CloudMatch create session error {} ({}): {body_text}",
                                limit_payload.request_status.status_code,
                                limit_payload.request_status.describe()
                            ))
                            .with_http_status(status.as_u16())
                            .into());
                        }
                    }

                    // CloudMatch rate-limits bursts of launch attempts with 429
                    // (`REQUEST_LIMIT_EXCEEDED`), and sheds load with 5xx while a zone is busy.
                    // Both clear on their own, so they get honoured with a backoff instead of
                    // failing the launch on the first reply.
                    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
                    {
                        throttled += 1;
                        let wait = retry_after(&headers)
                            .unwrap_or_else(|| Duration::from_secs(2 << throttled.min(3)));
                        eprintln!(
                            "CloudMatch replied {status}, waiting {:?} before retry {throttled}",
                            wait
                        );
                        sleep(wait).await;
                        continue;
                    }

                    bail!("HTTP {status}: {body_text}");
                }
                Err(err) => {
                    last_err = Some(err);
                    sleep(Duration::from_millis(500)).await;
                }
            }
        }
        // Reached only by exhausting the retries: either transport errors (`last_err`) or a
        // sustained 429/5xx, which leaves `last_err` empty.
        match last_err {
            Some(err) => Err(anyhow::Error::new(err)
                .context("CloudMatch create session request failed")),
            None => bail!(
                "CloudMatch kept rejecting the launch after {throttled} throttled attempts - \
                 NVIDIA is rate limiting this account, try again in a few minutes"
            ),
        }
    };

    // Each limit-exceeded reply stops the zombie sessions it names and earns one retry; a
    // second one can surface when several zombies were squatting on the device id at once.
    let mut cleanups = 0;
    let payload = loop {
        let (payload, was_limit_exceeded) = send_request().await?;
        if !was_limit_exceeded {
            break payload;
        }
        if cleanups >= 2 {
            // still hitting the limit after cleanup means its a session this device cant
            // delete, report it as the per-device limit and let the error screen explain
            return Err(GfnError::new(
                GfnErrorCode::SESSION_LIMIT_PER_DEVICE_REACHED,
                "CloudMatch still reported the session limit after cleanup",
            )
            .into());
        }
        cleanups += 1;
        // No sleep here: `stop_conflicting_sessions` already waited for CloudMatch to confirm the
        // slot was released before returning.
    };

    parse_session_info(payload, base_url, identity)
}

/// The server's own `Retry-After` (seconds form), clamped so a hostile or mistaken value can't
/// park a launch for minutes.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let seconds: u64 = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds.clamp(1, 30)))
}

#[derive(Debug, Clone, Default)]
pub struct QueueStatus {
    pub queue_position: u32,
    pub eta_ms: u32,
    pub attempt: usize,
    /// Consecutive 5xx replies from CloudMatch. Non-zero means the poll loop is backing off
    /// rather than stalled, which is worth telling the player before it eventually gives up.
    pub server_errors: usize,
    /// Latches once CloudMatch has actually reported a queue position. `queue_position` only
    /// describes right now, so it drops back to 0 the moment the wait ends - this remembers that
    /// the wait happened at all, so the launch stepper can tell "queued, then done" apart from
    /// "never queued".
    pub was_queued: bool,
    // rig is patching the game, can take a while so we tell the player instead of looking stuck
    pub app_patching: bool,
}

pub type QueueProgressTracker = Arc<std::sync::Mutex<QueueStatus>>;

/// Polls a session until the server reports it is ready, or a reasonable timeout expires.
pub async fn poll_session(
    client: &Client,
    request: PollSessionRequest<'_>,
    tracker: Option<QueueProgressTracker>,
) -> Result<SessionInfo> {
    let base_url = request.session.streaming_base_url.trim_end_matches('/');
    let url = format!("{base_url}/v2/session/{}", request.session_id);
    let identity = &request.session.identity;
    const MAX_ATTEMPTS: usize = 1800;
    const POLL_INTERVAL: Duration = Duration::from_secs(2);
    // CloudMatch answers 5xx while a rig is being provisioned or its zone is loaded, and those
    // stretches routinely outlast a handful of polls. Retrying on the flat 2s interval burned the
    // whole allowance in ~22 seconds and killed launches that would have succeeded, so server
    // errors back off instead - 12 retries now span a couple of minutes.
    const MAX_CONSECUTIVE_SERVER_ERRORS: usize = 12;
    const SERVER_ERROR_BACKOFF_CAP: Duration = Duration::from_secs(15);
    let mut consecutive_server_errors = 0usize;

    for attempt in 0..MAX_ATTEMPTS {
        let response = match headers::apply_cloudmatch_headers(
            client.get(&url),
            request.token,
            &identity.client_id,
            &identity.device_id,
        )
        .send()
        .await
        {
            Ok(response) => response,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("CloudMatch poll attempt {attempt} failed"));
            }
        };

        if response.status().is_server_error() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            // check for patching before it counts against the 5xx budget, its not really an error
            let patching = serde_json::from_str::<CloudMatchResponse>(&body_text)
                .map(|payload| payload.request_status.is_app_patching())
                .unwrap_or(false);
            if patching {
                consecutive_server_errors = 0;
                if let Some(tr) = &tracker
                    && let Ok(mut st) = tr.lock()
                {
                    st.attempt = attempt + 1;
                    st.server_errors = 0;
                    st.app_patching = true;
                }
                sleep(POLL_INTERVAL).await;
                continue;
            }

            // a code marked non-retryable (banned region, membership etc) skips the retry
            // budget entirely, no point burning 2.5 min re-asking about something final
            let failure = serde_json::from_str::<CloudMatchResponse>(&body_text)
                .ok()
                .map(|payload| {
                    payload.request_status.to_error(format!(
                        "CloudMatch poll attempt {attempt} rejected: {}: {body_text}",
                        describe_status(status, &body_text)
                    ))
                })
                .map(|error| error.with_http_status(status.as_u16()));
            if let Some(failure) = &failure
                && !failure.code.is_retryable()
            {
                return Err(failure.clone().into());
            }

            consecutive_server_errors += 1;
            if let Some(tr) = &tracker
                && let Ok(mut st) = tr.lock()
            {
                st.attempt = attempt + 1;
                st.server_errors = consecutive_server_errors;
                st.app_patching = false;
            }
            if consecutive_server_errors > MAX_CONSECUTIVE_SERVER_ERRORS {
                return Err(failure.map_or_else(
                    || {
                        // no payload, gotta go off the http status alone
                        GfnError::new(
                            GfnErrorCode::from_http_status(status.as_u16())
                                .unwrap_or(GfnErrorCode::SERVER_INTERNAL_ERROR),
                            format!(
                                "CloudMatch poll attempt {attempt} rejected: {}: {body_text}",
                                describe_status(status, &body_text)
                            ),
                        )
                        .with_http_status(status.as_u16())
                    },
                    |failure| failure,
                )
                .into());
            }
            let backoff = POLL_INTERVAL
                .saturating_mul(1 << (consecutive_server_errors - 1).min(3) as u32)
                .min(SERVER_ERROR_BACKOFF_CAP);
            sleep(backoff).await;
            continue;
        }
        consecutive_server_errors = 0;

        let response = error_for_status_with_body(response)
            .await
            .with_context(|| format!("CloudMatch poll attempt {attempt} rejected"))?;

        let body_text = response
            .text()
            .await
            .context("failed to read CloudMatch poll response body")?;
        let payload: CloudMatchResponse = serde_json::from_str(&body_text)
            .context("failed to decode CloudMatch poll response")?;

        if payload.request_status.status_code != 1 {
            return Err(payload.request_status.to_error(format!(
                "CloudMatch poll error: {} ({}): {body_text}",
                payload.request_status.status_code,
                payload.request_status.describe()
            ))
            .into());
        }

        let session = payload
            .session
            .as_ref()
            .context("CloudMatch poll response had no session")?;

        if let Some(tr) = &tracker {
            if let Ok(mut st) = tr.lock() {
                st.attempt = attempt + 1;
                st.server_errors = 0;
                if let Some(seat) = &session.seat_setup_info {
                    st.queue_position = seat.queue_position;
                    st.eta_ms = seat.seat_setup_eta;
                    st.was_queued |= seat.queue_position > 0;
                }
            }
        }

        if is_ready_status(session.status) {
            let base_host = base_url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .split('/')
                .next()
                .and_then(|authority| authority.split(':').next())
                .unwrap_or("");
            if let Some(real_ip) = streaming_server_ip(session) {
                if is_zone_hostname(base_host) && !is_zone_hostname(&real_ip) {
                    let direct_base = format!("https://{real_ip}");
                    let direct_url = format!("{direct_base}/v2/session/{}", request.session_id);
                    if let Some(direct_payload) =
                        fetch_session_payload(client, &direct_url, request.token, identity).await
                    {
                        return parse_session_info(direct_payload, &direct_base, identity.clone());
                    }
                }
            }
            return parse_session_info(payload, base_url, identity.clone());
        }

        sleep(POLL_INTERVAL).await;
    }

    bail!("CloudMatch session did not become ready within the poll timeout")
}

// e.g "HTTP 503 (41 APP_PATCHING_STATUS)", put first bc the error screen truncates the long
// json body and this is the part ppl actually screenshot
fn describe_status(status: reqwest::StatusCode, body_text: &str) -> String {
    match serde_json::from_str::<CloudMatchResponse>(body_text) {
        Ok(payload) => format!(
            "HTTP {status} ({} {})",
            payload.request_status.status_code,
            payload.request_status.describe()
        ),
        Err(_) => format!("HTTP {status}"),
    }
}

/// Best-effort GET of a session payload; `None` on any transport/decode/status failure so the
/// caller can fall back to the zone load balancer response it already has.
async fn fetch_session_payload(
    client: &Client,
    url: &str,
    token: &str,
    identity: &SessionIdentity,
) -> Option<CloudMatchResponse> {
    let response = headers::apply_cloudmatch_headers(
        client.get(url),
        token,
        &identity.client_id,
        &identity.device_id,
    )
    .send()
    .await
    .ok()?;
    let response = error_for_status_with_body(response).await.ok()?;
    let payload: CloudMatchResponse = response.json().await.ok()?;
    if payload.request_status.status_code == 1 && payload.session.is_some() {
        Some(payload)
    } else {
        None
    }
}

/// Whether a stop actually freed the session slot.
///
/// The distinction matters: the launch retry loop only earns another attempt when a slot was
/// genuinely released. Treating a rejected DELETE as success is what let a stuck zombie burn every
/// retry and still surface as `SESSION_LIMIT_PER_DEVICE_EXCEEDED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// The session is gone - either we deleted it, or it had already expired (404).
    Stopped,
    /// CloudMatch refused to delete it under this identity. Retrying will not help: sessions
    /// created by an older build carry a different device identity and can only age out on their
    /// own.
    Forbidden,
    /// Something transient - a retry may still work.
    Failed,
}

pub async fn stop_session_by_id(
    client: &Client,
    token: &str,
    session_id: &str,
    identity: &SessionIdentity,
    base_url: &str,
) -> StopOutcome {
    let base_url = base_url.trim_end_matches('/');
    let url = format!("{base_url}/v2/session/{session_id}");

    let Ok(response) = headers::apply_cloudmatch_headers(
        client.delete(&url),
        token,
        &identity.client_id,
        &identity.device_id,
    )
    .send()
    .await
    else {
        eprintln!("CloudMatch stop for session {session_id} could not be sent");
        return StopOutcome::Failed;
    };

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    // 404 means the session is already gone, which is exactly the state we wanted.
    if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
        return StopOutcome::Stopped;
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        eprintln!(
            "CloudMatch refused to stop session {session_id} (HTTP 403); it belongs to another \
             device identity and has to expire on its own"
        );
        return StopOutcome::Forbidden;
    }
    eprintln!("CloudMatch stop for session {session_id} failed: HTTP {status}: {body}");
    StopOutcome::Failed
}

// deletes at the session's own zone url, not the generic entrypoint, sessions only
// live on the zone that provisioned them. mirrors OpenNOW's StopSession
pub async fn stop_session(client: &Client, token: &str, session: &SessionInfo) {
    let base_url = if session.streaming_base_url.is_empty() {
        DEFAULT_CLOUDMATCH_BASE_URL
    } else {
        &session.streaming_base_url
    };
    if stop_session_by_id(
        client,
        token,
        &session.session_id,
        &session.identity,
        base_url,
    )
    .await
        != StopOutcome::Failed
    {
        // treat as gone either way, real failures keep the note around for next attempt
        active_session::forget(&session.session_id);
    }
}

/// Sessions CloudMatch still considers active for this account. Setup/queuing (1), ready (2)
/// and streaming (3) all count against the per-device session limit. Mirrors OpenNOW's
/// `getActiveSessions` (`GET /v2/session`).
pub async fn get_active_sessions(
    client: &Client,
    token: &str,
    identity: &SessionIdentity,
) -> Result<Vec<String>> {
    let base_url = DEFAULT_CLOUDMATCH_BASE_URL.trim_end_matches('/');
    let url = format!("{base_url}/v2/session");

    // Deliberately the launch's own identity rather than a fresh random one: CloudMatch scopes the
    // per-device session limit by these headers, so listing under a different client id can hide
    // the very sessions that are blocking us.
    let response = headers::apply_cloudmatch_headers(
        client.get(&url),
        token,
        &identity.client_id,
        &identity.device_id,
    )
    .send()
    .await?;
    let body_text = response.text().await.unwrap_or_default();
    let payload: GetSessionsResponse = match serde_json::from_str(&body_text) {
        Ok(payload) => payload,
        Err(error) => {
            // Worth shouting about: the caller treats a failure here as "no zombies found", so a
            // silent decode error looks exactly like a clean account while launches keep failing.
            eprintln!("Could not read CloudMatch active sessions: {error}: {body_text}");
            return Err(anyhow::Error::new(error)
                .context("failed to decode CloudMatch active sessions response"));
        }
    };

    Ok(payload
        .sessions
        .into_iter()
        .filter(|s| s.status.occupies_device_slot())
        .filter_map(|s| s.session_id.map(|id| id.as_string()))
        .filter(|id| !id.is_empty())
        .collect())
}

/// Deletes every session squatting on this device id: the ones the error payload names, or -
/// when CloudMatch names none - every active session from `get_active_sessions` (OpenNOW's
/// `stopActiveSessionsForCreate`). Returns true when at least one stop was issued, telling the
/// caller a retry is worthwhile.
async fn stop_conflicting_sessions(
    client: &Client,
    token: &str,
    payload: Option<&CloudMatchResponse>,
    identity: &SessionIdentity,
    base_url: &str,
) -> bool {
    let mut old_ids = Vec::new();
    if let Some(payload) = payload {
        if let Some(session) = &payload.session
            && let Some(id) = &session.session_id
        {
            old_ids.push(id.as_string());
        }
        for session in &payload.other_user_sessions {
            if let Some(id) = &session.session_id {
                old_ids.push(id.as_string());
            }
        }
    }
    if old_ids.is_empty() {
        old_ids = get_active_sessions(client, token, identity)
            .await
            .unwrap_or_default();
    }
    old_ids.retain(|id| !id.is_empty());
    // `dedup` only collapses *adjacent* duplicates, so the same id named by both the payload's
    // session and `otherUserSessions` would otherwise be deleted twice.
    old_ids.sort();
    old_ids.dedup();

    let mut stopped_any = false;
    for old_id in &old_ids {
        eprintln!("CloudMatch session limit hit; stopping zombie session {old_id}");
        if stop_session_by_id(client, token, old_id, identity, base_url).await
            == StopOutcome::Stopped
        {
            stopped_any = true;
        }
    }
    if !stopped_any {
        // Nothing was freed, so retrying the launch would just hit the same wall.
        return false;
    }
    // A 200 on the DELETE only means NVIDIA accepted the request. Deprovisioning a rig that was
    // mid-setup takes appreciably longer than that, and retrying the launch before the slot is
    // actually released just spends an attempt on the same limit error.
    wait_for_sessions_to_clear(client, token, identity).await
}

// cleans up a session we recorded but never confirmed closed (crash/force-quit path).
// deletes at the recorded zone since that's the only place that knows about it.
// mirrors OpenNOW-Switch's CleanupStaleCloudSession
async fn stop_remembered_session(client: &Client, token: &str, identity: &SessionIdentity) {
    let Some(stale) = active_session::load() else {
        return;
    };
    let base_url = if stale.streaming_base_url.is_empty() {
        DEFAULT_CLOUDMATCH_BASE_URL
    } else {
        &stale.streaming_base_url
    };

    eprintln!(
        "Ending the session left over from a previous run: {}",
        stale.session_id
    );
    match stop_session_by_id(client, token, &stale.session_id, identity, base_url).await {
        StopOutcome::Stopped => {
            active_session::forget(&stale.session_id);
            wait_for_sessions_to_clear(client, token, identity).await;
        }
        StopOutcome::Forbidden => {
            eprintln!(
                "Session {} belongs to an older device identity and has to expire on its own; \
                 dropping the note so it stops blocking launches",
                stale.session_id
            );
            active_session::forget(&stale.session_id);
        }
        // keep the note on failure, dont wanna lose track of it over a network blip
        StopOutcome::Failed => {}
    }
}

/// Ends every session CloudMatch still reports before a new launch is attempted.
///
/// Preemptive rather than reactive: GeForce NOW only allows one session at a time, so anything
/// still open is going to reject this launch. Clearing it up front turns what used to be a failed
/// launch plus two cleanup rounds into a launch that simply works.
///
/// Costs one GET when the account is already clear, which is the common case - the stopping and
/// waiting only happen when there is genuinely something to remove.
async fn stop_active_sessions_before_launch(
    client: &Client,
    token: &str,
    identity: &SessionIdentity,
    base_url: &str,
) {
    let Ok(active) = get_active_sessions(client, token, identity).await else {
        // Can't tell, so just launch: the session-limit handler is still there as a backstop.
        return;
    };
    if active.is_empty() {
        return;
    }

    eprintln!(
        "Ending {} session(s) still open before launching: {}",
        active.len(),
        active.join(", ")
    );
    let mut stopped_any = false;
    for session_id in &active {
        if stop_session_by_id(client, token, session_id, identity, base_url).await
            == StopOutcome::Stopped
        {
            stopped_any = true;
        }
    }
    if stopped_any {
        wait_for_sessions_to_clear(client, token, identity).await;
    }
}

/// Polls until CloudMatch stops reporting sessions on this device, or gives up.
///
/// Returns whether the device looks clear. Bounded well under the launch overlay's patience, so a
/// server that never releases the slot still fails with an explanation rather than hanging.
async fn wait_for_sessions_to_clear(
    client: &Client,
    token: &str,
    identity: &SessionIdentity,
) -> bool {
    const MAX_CHECKS: usize = 8;
    const CHECK_INTERVAL: Duration = Duration::from_secs(3);

    for check in 0..MAX_CHECKS {
        sleep(CHECK_INTERVAL).await;
        match get_active_sessions(client, token, identity).await {
            Ok(remaining) if remaining.is_empty() => {
                eprintln!("CloudMatch device slot is clear after {}s", (check + 1) * 3);
                return true;
            }
            Ok(remaining) => {
                eprintln!(
                    "Waiting for CloudMatch to release {} session(s): {}",
                    remaining.len(),
                    remaining.join(", ")
                );
            }
            // Can't tell - assume it cleared rather than blocking a launch that might work.
            Err(error) => {
                eprintln!("Could not confirm CloudMatch session cleanup: {error:#}");
                return true;
            }
        }
    }
    eprintln!("CloudMatch still reports active sessions after {MAX_CHECKS} checks");
    false
}

fn is_ready_status(status: u32) -> bool {
    status == 2 || status == 3
}

/// Zone load balancer hostnames (e.g.
fn is_zone_hostname(host: &str) -> bool {
    host.contains("cloudmatchbeta.nvidiagrid.net") || host.contains("cloudmatch.nvidiagrid.net")
}

/// Host portion of a `rtsps://host:port/...`-style URL.
fn extract_host_from_url(url: &str) -> Option<&str> {
    let after_proto = ["rtsps://", "rtsp://", "wss://", "https://"]
        .iter()
        .find_map(|prefix| url.strip_prefix(prefix))?;
    let host = after_proto.split(':').next()?.split('/').next()?;
    if host.is_empty() || host.starts_with('.') {
        None
    } else {
        Some(host)
    }
}

/// The real seat host, per OpenNOW's `streamingServerIp` priority chain: the usage-14
/// connection's `ip`, then the host inside its `resourcePath`, then `sessionControlInfo.ip`.
fn streaming_server_ip(session: &CloudMatchSession) -> Option<String> {
    if let Some(conn) = session
        .connection_info
        .iter()
        .find(|conn| conn.matches_usage(14))
    {
        if let Some(ip) = conn
            .ip
            .as_ref()
            .map(|ip| ip.as_string())
            .filter(|ip| !ip.is_empty())
        {
            return Some(ip);
        }
        if let Some(host) = conn
            .resource_path
            .as_deref()
            .and_then(extract_host_from_url)
        {
            return Some(host.to_owned());
        }
    }

    session
        .session_control_info
        .as_ref()
        .and_then(|ctrl| ctrl.ip.as_ref().map(|ip| ip.as_string()))
        .filter(|ip| !ip.is_empty())
}

/// Mirrors OpenNOW's `buildSignalingUrl`: `rtsps://host:port` resourcePaths become
/// `wss://{host}/nvst/`, absolute `wss://` URLs pass through, bare paths hang off the server ip,
/// and anything else falls back to `wss://{server_ip}:443/nvst/`.
fn build_signaling_url(raw: &str, server_ip: &str) -> (String, Option<String>) {
    if raw.starts_with("rtsps://") || raw.starts_with("rtsp://") {
        if let Some(host) = extract_host_from_url(raw) {
            return (format!("wss://{host}/nvst/"), Some(host.to_owned()));
        }
        return (format!("wss://{server_ip}:443/nvst/"), None);
    }

    if raw.starts_with("wss://") {
        let host = raw["wss://".len()..].split('/').next().map(str::to_owned);
        return (raw.to_owned(), host);
    }

    if server_ip.is_empty() {
        return (String::new(), None);
    }

    if raw.starts_with('/') {
        return (format!("wss://{server_ip}:443{raw}"), None);
    }

    (format!("wss://{server_ip}:443/nvst/"), None)
}


fn build_session_request_body(
    app_id: &str,
    device_hash_id: &str,
    width: u32,
    height: u32,
    fps: u32,
) -> serde_json::Value {
    let sub_session_id = uuid::Uuid::new_v4().to_string();
    let metadata = json!([
        { "key": "SubSessionId", "value": sub_session_id },
        { "key": "wssignaling", "value": "1" },
        { "key": "GSStreamerType", "value": "WebRTC" },
        { "key": "networkType", "value": "Unknown" },
        { "key": "ClientImeSupport", "value": "0" },
        {
            "key": "clientPhysicalResolution",
            "value": json!({ "horizontalPixels": width, "verticalPixels": height }).to_string()
        },
        { "key": "surroundAudioInfo", "value": "2" }
    ]);

    json!({
        "sessionRequestData": {
            "appId": app_id,
            "internalTitle": null,
            "availableSupportedControllers": [],
            "networkTestSessionId": null,
            "parentSessionId": null,
            "clientIdentification": "GFN-PC",
            "deviceHashId": device_hash_id,
            "clientVersion": "30.0",
            "sdkVersion": "1.0",
            "streamerVersion": 1,
            "clientPlatformName": "windows",
            "clientRequestMonitorSettings": [
                {
                    "monitorId": 0,
                    "positionX": 0,
                    "positionY": 0,
                    "widthInPixels": width,
                    "heightInPixels": height,
                    "framesPerSecond": fps,
                    "sdrHdrMode": 0,
                    "displayData": {},
                    "hdr10PlusGamingData": null,
                    "dpi": 0,
                }
            ],
            "useOps": true,
            "audioMode": 2,
            "metaData": metadata,
            "sdrHdrMode": 0,
            "clientDisplayHdrCapabilities": null,
            "surroundAudioInfo": 0,
            "remoteControllersBitmap": 0,
            "clientTimezoneOffset": 0,
            "enhancedStreamMode": 1,
            "appLaunchMode": 0,
            "secureRTSPSupported": false,
            "partnerCustomData": "",
            "accountLinked": true,
            "enablePersistingInGameSettings": false,
            "userAge": 26,
            "requestedStreamingFeatures": {
                "reflex": false,
                "bitDepth": 0,
                "cloudGsync": false,
                "enabledL4S": false,
                "supportedHidDevices": 0,
                "profile": 0,
                "fallbackToLogicalResolution": false,
                "chromaFormat": 0,
                "prefilterMode": 0,
                "prefilterSharpness": 0,
                "prefilterNoiseReduction": 0,
                "hudStreamingMode": 0,
            }
        }
    })
}

fn parse_session_info(
    payload: CloudMatchResponse,
    streaming_base_url: &str,
    identity: SessionIdentity,
) -> Result<SessionInfo> {
    if payload.request_status.status_code != 1 {
        bail!(
            "CloudMatch error: {} ({})",
            payload.request_status.status_code,
            payload.request_status.describe()
        );
    }

    let session = payload
        .session
        .as_ref()
        .context("CloudMatch response had no session")?;

    let session_id = session
        .session_id
        .as_ref()
        .map(|s| s.as_string())
        .unwrap_or_default();

    let server_ip = streaming_server_ip(session).unwrap_or_default();

    let signaling_connection = session
        .connection_info
        .iter()
        .find(|conn| conn.matches_usage(14) && conn.ip.is_some())
        .or_else(|| session.connection_info.iter().find(|conn| conn.ip.is_some()));
    let resource_path = signaling_connection
        .and_then(|conn| conn.resource_path.as_deref())
        .unwrap_or("/nvst/");

    let (signaling_url, signaling_host) = build_signaling_url(resource_path, &server_ip);
    let effective_host = signaling_host.unwrap_or_else(|| server_ip.clone());
    let signaling_server = if effective_host.contains(':') || effective_host.is_empty() {
        effective_host
    } else {
        format!("{effective_host}:443")
    };

    let media_connection_info = session
        .connection_info
        .iter()
        .find(|conn| conn.matches_usage(2) || conn.matches_usage(17) || conn.matches_usage(14))
        .and_then(|conn| match (&conn.ip, conn.port) {
            (Some(ip), Some(port)) => Some(MediaConnectionInfo {
                ip: ip.as_string(),
                port,
            }),
            (None, Some(port)) if !server_ip.is_empty() => Some(MediaConnectionInfo {
                ip: server_ip.clone(),
                port,
            }),
            _ => None,
        });

    let ice_servers = session
        .ice_server_configuration
        .as_ref()
        .map(|cfg| {
            cfg.ice_servers
                .iter()
                .map(|server| IceServer {
                    urls: server.urls.to_vec(),
                    username: server.username.clone(),
                    credential: server.credential.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    let negotiated_stream_profile = session
        .session_request_data
        .as_ref()
        .and_then(|data| data.requested_streaming_features.as_ref())
        .map(|_features| NegotiatedStreamProfile {
            resolution: Some(format!("{}x{}", session.width, session.height)),
            fps: Some(session.fps),
            codec: Some("H264".to_owned()),
        });

    // record before handing back, so a crash downstream still has something to clean up next time
    active_session::remember(&session_id, streaming_base_url);

    Ok(SessionInfo {
        session_id,
        app_id: session
            .session_request_data
            .as_ref()
            .and_then(|data| data.app_id.as_ref().map(|id| id.as_string()))
            .unwrap_or_default(),
        status: session.status,
        server_ip,
        signaling_server,
        signaling_url,
        streaming_base_url: streaming_base_url.to_owned(),
        client_id: identity.client_id.clone(),
        device_id: identity.device_id.clone(),
        identity,
        media_connection_info,
        ice_servers,
        negotiated_stream_profile,
    })
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::Deserialize<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudMatchResponse {
    #[serde(rename = "requestStatus")]
    request_status: CloudMatchRequestStatus,
    #[serde(default)]
    session: Option<CloudMatchSession>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    other_user_sessions: Vec<CloudMatchSession>,
}

/// `GET /v2/session` reply - only the fields the zombie-session cleanup needs.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetSessionsResponse {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    sessions: Vec<ActiveSessionEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActiveSessionEntry {
    #[serde(default)]
    session_id: Option<FlexibleString>,
    #[serde(default)]
    status: FlexibleStatus,
}

/// A session status that CloudMatch may send either as a number or as a name.
///
/// It genuinely sends both, and a plain `u32` field is not merely lossy here - serde fails the
/// whole document when the value is a string, so one `"queued"` made the entire active-session
/// list decode to nothing and the zombie cleanup silently found nobody to stop.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum FlexibleStatus {
    Num(i64),
    Str(String),
}

impl Default for FlexibleStatus {
    fn default() -> Self {
        // Unknown rather than "queued": an absent status must not be mistaken for a real state.
        Self::Num(-1)
    }
}

impl FlexibleStatus {
    /// Normalizes to the numeric status codes used elsewhere in this module. Mirrors OpenNOW's
    /// `ParseSessionStatus`.
    fn code(&self) -> i64 {
        let text = match self {
            Self::Num(value) => return *value,
            Self::Str(text) => text.to_ascii_lowercase(),
        };
        match text.as_str() {
            "queued" => 0,
            "provisioning" | "initializing" | "setup" | "setting_up" | "launching"
            | "launching_game" => 1,
            "active" | "ready" | "paused" => 2,
            "streaming" | "playing" | "connected" => 3,
            _ if text.contains("fail")
                || text.contains("error")
                || text.contains("closed")
                || text.contains("terminated")
                || text.contains("cancel") =>
            {
                4
            }
            _ if text.contains("ad") => 6,
            _ => -1,
        }
    }

    /// Whether a session in this state still counts against the per-device session limit.
    ///
    /// Everything except a session that has positively finished. A queued session that never
    /// reached setup holds the slot just as firmly as one that is streaming, and that is exactly
    /// the case the old `1 | 2 | 3` filter walked past.
    fn occupies_device_slot(&self) -> bool {
        self.code() != 4
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudMatchRequestStatus {
    status_code: u32,
    /// CloudMatch names this field `statusDescription`. Reading only `statusMessage` meant it was
    /// always `None`, which is why every failure reported its code with "(unknown)" instead of
    /// text like `REQUEST_LIMIT_EXCEEDED_STATUS 4A8C2024`.
    #[serde(default, alias = "statusMessage")]
    status_description: Option<String>,
    /// Opaque NVIDIA support code, worth quoting verbatim when a launch fails.
    #[serde(default)]
    unified_error_code: Option<i64>,
}

impl CloudMatchRequestStatus {
    // gfn code this reply maps to
    fn error_code(&self) -> GfnErrorCode {
        GfnErrorCode::from_cloudmatch(self.status_code, self.unified_error_code)
    }

    // typed failure, code + the text the logs used to get before
    fn to_error(&self, detail: String) -> GfnError {
        GfnError::new(self.error_code(), detail).with_description(self.status_description.clone())
    }

    /// `<description> (#<unifiedErrorCode>)`, or `unknown` when the server sent neither.
    fn describe(&self) -> String {
        match (&self.status_description, self.unified_error_code) {
            (Some(text), Some(code)) => format!("{text} (#{code})"),
            (Some(text), None) => text.clone(),
            (None, Some(code)) => format!("#{code}"),
            (None, None) => "unknown".to_owned(),
        }
    }

    // checks the code table now (covers 11, 50, 83) instead of grepping the description
    // for SESSION_LIMIT like before. description is still a fallback for unknown codes
    fn is_session_limit(&self) -> bool {
        if self.error_code().is_session_conflict() {
            return true;
        }
        self.status_description
            .as_deref()
            .and_then(GfnErrorCode::from_description)
            .is_some_and(GfnErrorCode::is_session_conflict)
    }

    // rig is patching, not actually failing. comes back as non-200 so without this check
    // it eats the 5xx retry budget and the launch gets abandoned mid patch.
    // mirrors OpenNOW-Switch's IsAppPatchingResponse
    fn is_app_patching(&self) -> bool {
        self.status_code == 41
            && self
                .status_description
                .as_deref()
                .map(|text| text.to_ascii_uppercase().contains("APP_PATCHING_STATUS"))
                .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum FlexibleString {
    Str(String),
    Num(i64),
    List(Vec<FlexibleString>),
}

impl FlexibleString {
    fn as_string(&self) -> String {
        match self {
            FlexibleString::Str(s) => s.clone(),
            FlexibleString::Num(n) => n.to_string(),
            FlexibleString::List(items) => {
                items.first().map(|item| item.as_string()).unwrap_or_default()
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudMatchSession {
    #[serde(default)]
    session_id: Option<FlexibleString>,
    #[serde(default)]
    status: u32,
    #[serde(default)]
    session_control_info: Option<CloudMatchControlInfo>,
    #[serde(default)]
    seat_setup_info: Option<SeatSetupInfo>,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    fps: u32,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    connection_info: Vec<CloudMatchConnectionInfo>,
    #[serde(default)]
    ice_server_configuration: Option<IceServerConfiguration>,
    #[serde(default)]
    session_request_data: Option<CloudMatchSessionRequestData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SeatSetupInfo {
    #[serde(default)]
    pub queue_position: u32,
    #[serde(default)]
    pub seat_setup_step: u32,
    #[serde(default)]
    pub seat_setup_eta: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudMatchControlInfo {
    #[serde(default)]
    ip: Option<FlexibleString>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudMatchConnectionInfo {
    #[serde(default)]
    ip: Option<FlexibleString>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    usage: Option<UsageValue>,
    #[serde(default)]
    resource_path: Option<String>,
}

impl CloudMatchConnectionInfo {
    fn matches_usage(&self, code: u64) -> bool {
        match &self.usage {
            Some(UsageValue::Num(n)) => *n == code,
            Some(UsageValue::Str(s)) => s.parse::<u64>().map_or(false, |n| n == code),
            None => false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum UsageValue {
    Str(String),
    Num(u64),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IceServerConfiguration {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    ice_servers: Vec<IceServerDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IceServerDto {
    #[serde(default)]
    urls: IceUrls,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    credential: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum IceUrls {
    Single(String),
    Many(Vec<String>),
}

impl Default for IceUrls {
    fn default() -> Self {
        IceUrls::Many(Vec::new())
    }
}

impl IceUrls {
    fn to_vec(&self) -> Vec<String> {
        match self {
            IceUrls::Single(s) => vec![s.clone()],
            IceUrls::Many(v) => v.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudMatchSessionRequestData {
    #[serde(default)]
    app_id: Option<FlexibleString>,
    #[serde(default)]
    requested_streaming_features: Option<CloudMatchStreamingFeatures>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudMatchStreamingFeatures {
    #[serde(default)]
    bit_depth: Option<u8>,
    #[serde(default)]
    chroma_format: Option<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_status(body: &str) -> CloudMatchRequestStatus {
        serde_json::from_str::<CloudMatchResponse>(body)
            .expect("body should decode")
            .request_status
    }

    #[test]
    fn app_patching_is_recognised_from_the_status_pair() {
        let patching = r#"{"requestStatus":{"statusCode":41,
            "statusDescription":"APP_PATCHING_STATUS 1234"}}"#;
        assert!(request_status(patching).is_app_patching());
    }

    #[test]
    fn other_failures_are_not_mistaken_for_patching() {
        // right code wrong description
        let other = r#"{"requestStatus":{"statusCode":41,"statusDescription":"SOMETHING_ELSE"}}"#;
        assert!(!request_status(other).is_app_patching());

        // right description wrong code
        let wrong_code =
            r#"{"requestStatus":{"statusCode":50,"statusDescription":"APP_PATCHING_STATUS"}}"#;
        assert!(!request_status(wrong_code).is_app_patching());

        // real server error, shouldnt be mistaken for patching
        let busy = r#"{"requestStatus":{"statusCode":3,"statusDescription":"INTERNAL_ERROR"}}"#;
        assert!(!request_status(busy).is_app_patching());
    }

    fn active_ids(body: &str) -> Vec<String> {
        let payload: GetSessionsResponse =
            serde_json::from_str(body).expect("active sessions body should decode");
        payload
            .sessions
            .into_iter()
            .filter(|s| s.status.occupies_device_slot())
            .filter_map(|s| s.session_id.map(|id| id.as_string()))
            .filter(|id| !id.is_empty())
            .collect()
    }

    /// The regression that made every zombie cleanup a no-op: one string status failed the whole
    /// document, so the account looked empty while launches kept hitting the session limit.
    #[test]
    fn a_named_status_does_not_break_the_whole_list() {
        let body = r#"{"sessions":[
            {"sessionId":"aaa","status":"paused"},
            {"sessionId":"bbb","status":3}
        ]}"#;
        assert_eq!(active_ids(body), vec!["aaa".to_owned(), "bbb".to_owned()]);
    }

    /// A queued session never reached setup but still holds the device slot.
    #[test]
    fn queued_sessions_count_against_the_limit() {
        let body = r#"{"sessions":[{"sessionId":"queued-one","status":0}]}"#;
        assert_eq!(active_ids(body), vec!["queued-one".to_owned()]);
        let named = r#"{"sessions":[{"sessionId":"queued-two","status":"QUEUED"}]}"#;
        assert_eq!(active_ids(named), vec!["queued-two".to_owned()]);
    }

    #[test]
    fn finished_sessions_are_left_alone() {
        let body = r#"{"sessions":[
            {"sessionId":"dead","status":4},
            {"sessionId":"also-dead","status":"TERMINATED"},
            {"sessionId":"failed","status":"SESSION_ERROR"},
            {"sessionId":"live","status":"streaming"}
        ]}"#;
        assert_eq!(active_ids(body), vec!["live".to_owned()]);
    }

    /// An unrecognised state is far more likely to be holding the slot than not, and guessing
    /// wrong the other way leaves the player unable to launch at all.
    #[test]
    fn unknown_states_are_treated_as_occupying() {
        let body = r#"{"sessions":[{"sessionId":"mystery","status":"something-new"}]}"#;
        assert_eq!(active_ids(body), vec!["mystery".to_owned()]);
        assert!(FlexibleStatus::default().occupies_device_slot());
    }

    #[test]
    fn named_statuses_map_to_the_numeric_codes() {
        assert_eq!(FlexibleStatus::Str("queued".into()).code(), 0);
        assert_eq!(FlexibleStatus::Str("setting_up".into()).code(), 1);
        assert_eq!(FlexibleStatus::Str("READY".into()).code(), 2);
        assert_eq!(FlexibleStatus::Str("streaming".into()).code(), 3);
        assert_eq!(FlexibleStatus::Str("cancelled".into()).code(), 4);
        assert_eq!(FlexibleStatus::Num(7).code(), 7);
    }


    #[test]
    fn parse_resolution_splits() {
        let settings = StreamSettings {
            resolution: "1280x720".to_owned(),
            fps: 30,
            max_bitrate_mbps: 5,
        };
        assert_eq!(settings.dimensions(), (1280, 720));
    }

    #[test]
    fn is_ready_status_values() {
        assert!(is_ready_status(2));
        assert!(is_ready_status(3));
        assert!(!is_ready_status(1));
        assert!(!is_ready_status(4));
    }
}
