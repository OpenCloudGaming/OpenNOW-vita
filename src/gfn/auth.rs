//! NVIDIA GFN device-code login (the "Steam Deck" OAuth flow) and encrypted-at-rest token
//! storage.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Client;
use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// "Steam Deck" OAuth client id - the only one observed to support the device-code grant.
const CLIENT_ID: &str = "q61ddeJrVt7O90Nl-P-N7I36yctih4Ml6FyXLrb6j-U";
/// Default NVIDIA login provider id (as opposed to an Alliance-partner idp).
const IDP_ID: &str = "PDiAhv2kJTFeQ7WOPqiQ2tRZ7lGhR2X11dXvM4TZSxg";
const SCOPE: &str = "openid consent email tk_client age";
const DEVICE_AUTHORIZE_ENDPOINT: &str = "https://login.nvidia.com/device/authorize";
const TOKEN_ENDPOINT: &str = "https://login.nvidia.com/token";
/// Issues the long-lived device credential that outlives the OAuth refresh token.
const CLIENT_TOKEN_ENDPOINT: &str = "https://login.nvidia.com/client_token";
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; Steam Deck) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";
const DISPLAY_NAME: &str = "OpenNOW Vita";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Fallback poll interval if NVIDIA's response omits `interval` (should not happen in practice).
const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
/// Fallback device-code validity if NVIDIA's response omits `expires_in`.
const DEFAULT_CHALLENGE_TTL_SECS: u64 = 600;

const TOKEN_STORE_DIR: &str = "ux0:data/opennow-vita";
const TOKEN_STORE_PATH: &str = "ux0:data/opennow-vita/gfn-auth.json";
const TOKEN_STORE_VERSION: u8 = 1;
const TOKEN_KEY_MAGIC: &[u8; 8] = b"JVATKY01";
const TOKEN_KEY_SIZE: usize = 32;
const TOKEN_KEY_RECORD_SIZE: usize = TOKEN_KEY_MAGIC.len() + TOKEN_KEY_SIZE;
/// Safe Memory offset for the token encryption key.
const TOKEN_KEY_OFFSET: i64 = 0;
const TOKEN_NONCE_SIZE: usize = 12;
const TOKEN_AAD: &[u8] = b"opennow-vita/gfn-refresh-token/v1";

pub fn client() -> Client {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .pool_max_idle_per_host(0)
        .build()
        .unwrap_or_default()
}

/// A device code challenge in progress: what the UI shows, plus what `poll` needs to check on it.
#[derive(Debug, Clone)]
pub struct DeviceCodeChallenge {
    pub user_code: String,
    /// Already includes the user code as a query param - what the QR/link points at.
    pub verification_uri_complete: String,
    device_code: String,
    pub interval: Duration,
    deadline: Instant,
}

impl DeviceCodeChallenge {
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.deadline
    }
}

#[derive(Debug, Deserialize)]
struct DeviceAuthorizationResponse {
    device_code: String,
    user_code: String,
    verification_uri_complete: String,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    interval: Option<u64>,
}

pub async fn start_device_login(client: &Client) -> Result<DeviceCodeChallenge> {
    let response = client
        .post(DEVICE_AUTHORIZE_ENDPOINT)
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", "https://play.geforcenow.com")
        .header("Referer", "https://play.geforcenow.com/")
        .header("x-device-id", device_id())
        .header("nv-client-id", CLIENT_ID)
        .header("nv-client-streamer", "WEBRTC")
        .header("nv-client-type", "BROWSER")
        .header("nv-client-platform-name", "browser")
        .header("nv-browser-type", "CHROME")
        .header("nv-device-os", "STEAMOS")
        .header("nv-device-type", "CONSOLE")
        .header("nv-device-model", "STEAMDECK")
        .header("nv-device-make", "VALVE")
        .form(&[
            ("client_id", CLIENT_ID),
            ("scope", SCOPE),
            ("device_id", &device_id()),
            ("display_name", DISPLAY_NAME),
            ("idp_id", IDP_ID),
        ])
        .send()
        .await
        .context("device authorization request failed")?;

    let response = response
        .error_for_status()
        .context("device authorization request rejected")?;
    let payload: DeviceAuthorizationResponse = response
        .json()
        .await
        .context("failed to decode device authorization response")?;

    Ok(DeviceCodeChallenge {
        user_code: payload.user_code,
        verification_uri_complete: payload.verification_uri_complete,
        device_code: payload.device_code,
        interval: Duration::from_secs(
            payload
                .interval
                .unwrap_or(DEFAULT_POLL_INTERVAL_SECS)
                .max(1),
        ),
        deadline: Instant::now()
            + Duration::from_secs(payload.expires_in.unwrap_or(DEFAULT_CHALLENGE_TTL_SECS)),
    })
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    client_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClientTokenResponse {
    #[serde(default)]
    client_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

pub enum DevicePollOutcome {
    /// Keep polling; the user has not finished logging in on their other device yet.
    Pending,
    /// NVIDIA asked us to slow down - the caller should widen its poll interval.
    SlowDown,
    Authorized(AuthTokens),
    /// The device code expired before the user completed login.
    Expired,
    /// The user explicitly declined the login request.
    Denied,
}

pub async fn poll_device_login(
    client: &Client,
    challenge: &DeviceCodeChallenge,
) -> Result<DevicePollOutcome> {
    let response = client
        .post(TOKEN_ENDPOINT)
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", "https://play.geforcenow.com")
        .header("Referer", "https://play.geforcenow.com/")
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", &challenge.device_code),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await
        .context("device token poll request failed")?;

    if response.status().is_success() {
        let payload: TokenResponse = response
            .json()
            .await
            .context("failed to decode device token response")?;
        let mut tokens = AuthTokens {
            access_token: payload.access_token,
            refresh_token: payload.refresh_token,
            id_token: payload.id_token,
            expires_at_unix: expires_at_unix(payload.expires_in),
            client_token: payload.client_token,
            client_token_expires_at_unix: 0,
        };
        // Grab the long-lived credential right away, while the access token is certainly valid.
        if tokens.client_token.is_none() {
            request_client_token(client, &mut tokens).await;
        }
        return Ok(DevicePollOutcome::Authorized(tokens));
    }

    let error: TokenErrorResponse = response
        .json()
        .await
        .context("failed to decode device token error response")?;
    Ok(match error.error.as_str() {
        "authorization_pending" => DevicePollOutcome::Pending,
        "slow_down" => DevicePollOutcome::SlowDown,
        "expired_token" => DevicePollOutcome::Expired,
        "access_denied" => DevicePollOutcome::Denied,
        other => bail!(
            "device token poll rejected: {other} ({})",
            error.error_description.unwrap_or_default()
        ),
    })
}

fn expires_at_unix(expires_in: Option<u64>) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    now + expires_in.unwrap_or(86_400)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expires_at_unix: u64,
    /// NVIDIA's long-lived device credential. Defaulted so token stores written before this field
    /// existed still deserialize - they simply refresh via `refresh_token` until a client token is
    /// fetched.
    #[serde(default)]
    pub client_token: Option<String>,
    #[serde(default)]
    pub client_token_expires_at_unix: u64,
}

impl AuthTokens {
    /// The token GFN's own REST/GraphQL endpoints expect in `Authorization: GFNJWT <token>` -
    /// prefers `id_token` with `access_token` as a fallback, matching OpenNOW's own
    /// `session.tokens.idToken ?? session.tokens.accessToken` pattern.
    pub fn bearer(&self) -> &str {
        self.id_token.as_deref().unwrap_or(&self.access_token)
    }

    /// Whether the access token is close enough to expiry to be worth replacing before use.
    pub fn needs_refresh(&self) -> bool {
        should_refresh(self.expires_at_unix, now_unix())
    }

    /// Whether anything about the saved login is worth renewing before the next request.
    ///
    /// Deliberately no "is it expired" counterpart driven by the local clock: the Vita's RTC can be
    /// wrong by hours, and declaring a working credential dead is far more costly than sending one
    /// request that comes back 401. NVIDIA decides when a token is finished.
    pub fn needs_maintenance(&self) -> bool {
        self.needs_refresh() || self.client_token_needs_refresh()
    }

    fn client_token_needs_refresh(&self) -> bool {
        self.client_token.is_none() || should_refresh(self.client_token_expires_at_unix, now_unix())
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Replace a token this long before it actually expires, so a request never races the deadline.
const REFRESH_WINDOW_SECS: u64 = 10 * 60;

fn should_refresh(expires_at_unix: u64, now: u64) -> bool {
    expires_at_unix == 0 || expires_at_unix.saturating_sub(now) < REFRESH_WINDOW_SECS
}

/// Statuses worth retrying: a transient network failure or a server-side hiccup says nothing about
/// whether the credential is still good.
fn is_temporary_status(status: Option<u16>) -> bool {
    match status {
        None => true,
        Some(status) => status == 408 || status == 429 || status >= 500,
    }
}

fn refresh_retry_delay(completed_attempts: u32) -> Duration {
    if completed_attempts <= 1 {
        Duration::from_millis(500)
    } else {
        Duration::from_millis(1_500)
    }
}

/// Why a refresh failed, which decides whether the saved login survives.
#[derive(Debug)]
pub enum RefreshError {
    /// The credential itself is no longer accepted - the only cure is a fresh device-code login.
    ReauthenticationRequired(String),
    /// Something transient. The saved login must be kept: discarding it here would turn a dropped
    /// packet into a QR-code re-scan.
    Temporary(anyhow::Error),
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReauthenticationRequired(message) => write!(formatter, "{message}"),
            Self::Temporary(error) => write!(formatter, "{error:#}"),
        }
    }
}

/// Carries the fields NVIDIA leaves out of a refresh response over from the previous tokens.
///
/// This is not defensive tidiness - it is required. NVIDIA routinely omits `id_token` when
/// refreshing, and `bearer()` prefers `id_token`, so blindly taking the response would swap the
/// signed JWT that CloudMatch needs for an access token it rejects.
fn merge_refreshed(previous: &AuthTokens, response: TokenResponse) -> AuthTokens {
    let client_token_rotated = response
        .client_token
        .as_ref()
        .is_some_and(|token| Some(token) != previous.client_token.as_ref());
    AuthTokens {
        access_token: response.access_token,
        refresh_token: response.refresh_token.or_else(|| previous.refresh_token.clone()),
        id_token: response.id_token.or_else(|| previous.id_token.clone()),
        expires_at_unix: expires_at_unix(response.expires_in),
        // A rotated client token needs its own lifetime from /client_token, so mark it unknown (0)
        // rather than inheriting the old one's deadline.
        client_token_expires_at_unix: if client_token_rotated {
            0
        } else if response.client_token.is_some() {
            expires_at_unix(response.expires_in)
        } else {
            previous.client_token_expires_at_unix
        },
        client_token: response.client_token.or_else(|| previous.client_token.clone()),
    }
}

async fn post_token_form(
    client: &Client,
    form: &[(&str, &str)],
) -> Result<TokenResponse, RefreshError> {
    let mut last_error = None;
    for attempt in 1..=3u32 {
        let response = client
            .post(TOKEN_ENDPOINT)
            .header("Accept", "application/json, text/plain, */*")
            .header("Origin", "https://play.geforcenow.com")
            .header("Referer", "https://play.geforcenow.com/")
            .form(form)
            .send()
            .await;

        let status = match &response {
            Ok(response) => Some(response.status().as_u16()),
            Err(_) => None,
        };

        match response {
            Ok(response) if response.status().is_success() => {
                return response
                    .json::<TokenResponse>()
                    .await
                    .map_err(|error| RefreshError::Temporary(error.into()));
            }
            // 400 and 401 are NVIDIA saying the credential is dead, not that we should try again.
            Ok(response) if matches!(response.status().as_u16(), 400 | 401) => {
                return Err(RefreshError::ReauthenticationRequired(format!(
                    "saved GFN login is no longer valid (HTTP {})",
                    response.status().as_u16()
                )));
            }
            Ok(response) => {
                last_error = Some(anyhow::anyhow!(
                    "token refresh rejected with HTTP {}",
                    response.status().as_u16()
                ));
            }
            Err(error) => last_error = Some(anyhow::Error::from(error)),
        }

        if !is_temporary_status(status) || attempt == 3 {
            break;
        }
        tokio::time::sleep(refresh_retry_delay(attempt)).await;
    }

    Err(RefreshError::Temporary(last_error.unwrap_or_else(|| {
        anyhow::anyhow!("token refresh failed for an unknown reason")
    })))
}

/// Exchanges the long-lived client token for a fresh access token.
async fn refresh_with_client_token(
    client: &Client,
    tokens: &AuthTokens,
    user_id: &str,
) -> Result<AuthTokens, RefreshError> {
    let Some(client_token) = tokens.client_token.as_deref() else {
        return Err(RefreshError::Temporary(anyhow::anyhow!(
            "no client token saved"
        )));
    };
    if user_id.is_empty() {
        return Err(RefreshError::Temporary(anyhow::anyhow!(
            "client-token refresh needs the account subject id"
        )));
    }

    let response = post_token_form(
        client,
        &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:client_token"),
            ("client_token", client_token),
            ("client_id", CLIENT_ID),
            ("sub", user_id),
        ],
    )
    .await?;
    Ok(merge_refreshed(tokens, response))
}

/// The standard OAuth refresh, used when there is no client token or it was rejected.
async fn refresh_with_refresh_token(
    client: &Client,
    tokens: &AuthTokens,
) -> Result<AuthTokens, RefreshError> {
    let Some(refresh_token) = tokens.refresh_token.as_deref() else {
        return Err(RefreshError::ReauthenticationRequired(
            "saved GFN login cannot be refreshed".to_owned(),
        ));
    };

    let response = post_token_form(
        client,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ],
    )
    .await?;
    Ok(merge_refreshed(tokens, response))
}

/// Fetches a client token for the current access token, if NVIDIA will issue one.
///
/// Best-effort: the OAuth refresh token still works without it, this just outlives it.
async fn request_client_token(client: &Client, tokens: &mut AuthTokens) {
    let response = client
        .get(CLIENT_TOKEN_ENDPOINT)
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", "https://play.geforcenow.com")
        .header("Referer", "https://play.geforcenow.com/")
        .header("Authorization", format!("Bearer {}", tokens.access_token))
        .send()
        .await;

    let Ok(response) = response else { return };
    if !response.status().is_success() {
        return;
    }
    let Ok(payload) = response.json::<ClientTokenResponse>().await else {
        return;
    };
    if let Some(client_token) = payload.client_token {
        tokens.client_token = Some(client_token);
        tokens.client_token_expires_at_unix = expires_at_unix(payload.expires_in);
    }
}

/// Replaces the access token, preferring the long-lived client token and falling back to OAuth.
pub async fn refresh_tokens(
    client: &Client,
    tokens: &AuthTokens,
    user_id: &str,
) -> Result<AuthTokens, RefreshError> {
    if tokens.client_token.is_some() {
        match refresh_with_client_token(client, tokens, user_id).await {
            Ok(refreshed) => return Ok(refreshed),
            Err(RefreshError::ReauthenticationRequired(message)) => {
                // The client token is dead, but the OAuth refresh token may well not be.
                eprintln!("Client-token refresh rejected ({message}); trying OAuth refresh");
            }
            Err(RefreshError::Temporary(error)) => {
                eprintln!("Client-token refresh failed ({error:#}); trying OAuth refresh");
            }
        }
    }
    refresh_with_refresh_token(client, tokens).await
}

/// Returns tokens good for immediate use, refreshing and re-saving them only when needed.
///
/// Called before work that authenticates, so an idle Vita picks up where it left off instead of
/// sending the player back to the QR code.
pub async fn ensure_fresh_tokens(
    client: &Client,
    tokens: &AuthTokens,
    user_id: &str,
) -> Result<AuthTokens, RefreshError> {
    if !tokens.needs_refresh() && !tokens.client_token_needs_refresh() {
        return Ok(tokens.clone());
    }

    let mut refreshed = if tokens.needs_refresh() {
        refresh_tokens(client, tokens, user_id).await?
    } else {
        tokens.clone()
    };
    if refreshed.client_token_needs_refresh() {
        request_client_token(client, &mut refreshed).await;
    }
    if let Err(error) = save_tokens(&refreshed) {
        // Losing the write is survivable - the tokens in memory are still good for this session.
        eprintln!("Could not persist refreshed GFN tokens: {error:#}");
    }
    Ok(refreshed)
}

pub struct GfnUser {
    /// The GFN account's stable subject id.
    #[allow(dead_code)]
    pub user_id: String,
    pub display_name: String,
    pub email: Option<String>,
}

/// Reads `sub`/`email`/`preferred_username` out of the `id_token` JWT payload without verifying
/// its signature - the token just came from NVIDIA's token endpoint over TLS, so there is nothing
/// to gain from also checking the signature client-side (mirrors OpenNOW's own
/// `parseJwtPayload`/`fetchUserInfo` fallback logic).
pub fn user_from_tokens(tokens: &AuthTokens) -> Result<GfnUser> {
    let jwt = tokens.id_token.as_deref().unwrap_or(&tokens.access_token);
    let payload = decode_jwt_payload(jwt)?;
    let user_id = payload
        .get("sub")
        .and_then(|value| value.as_str())
        .context("JWT payload missing 'sub'")?
        .to_owned();
    let email = payload
        .get("email")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    let display_name = payload
        .get("preferred_username")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .or_else(|| {
            email
                .as_deref()
                .and_then(|email| email.split('@').next())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "Usuario".to_owned());

    Ok(GfnUser {
        user_id,
        display_name,
        email,
    })
}

fn decode_jwt_payload(token: &str) -> Result<serde_json::Value> {
    let mut segments = token.split('.');
    segments.next().context("JWT missing header segment")?;
    let payload_segment = segments.next().context("JWT missing payload segment")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload_segment)
        .context("JWT payload is not valid base64url")?;
    serde_json::from_slice(&bytes).context("JWT payload is not valid JSON")
}

/// A random id persisted alongside the tokens, standing in for the hostname+username hash
/// OpenNOW's desktop client derives its `device_id` from - the Vita has neither concept in a way
/// that is stable and meaningful here.
fn device_id() -> String {
    if let Some(existing) = load_device_id() {
        return existing;
    }
    let mut bytes = [0u8; 16];
    let _ = SystemRandom::new().fill(&mut bytes);
    let id = encode_hex(&bytes);
    let _ = save_device_id(&id);
    id
}

const DEVICE_ID_PATH: &str = "ux0:data/opennow-vita/device-id.txt";

fn load_device_id() -> Option<String> {
    std::fs::read_to_string(DEVICE_ID_PATH)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn save_device_id(id: &str) -> Result<()> {
    ensure_token_store_dir()?;
    write_file_truncating(DEVICE_ID_PATH, id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EncryptedTokenStore {
    version: u8,
    nonce: String,
    ciphertext: String,
}

pub fn save_tokens(tokens: &AuthTokens) -> Result<()> {
    let plaintext = serde_json::to_vec(tokens).context("failed to serialize GFN tokens")?;
    let key = load_or_create_token_key()?;
    let mut nonce_bytes = [0u8; TOKEN_NONCE_SIZE];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| anyhow::anyhow!("failed to generate GFN token nonce"))?;
    let cipher = token_cipher(&key)?;
    let mut ciphertext = plaintext;
    cipher
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(TOKEN_AAD),
            &mut ciphertext,
        )
        .map_err(|_| anyhow::anyhow!("failed to encrypt GFN tokens"))?;

    let store = EncryptedTokenStore {
        version: TOKEN_STORE_VERSION,
        nonce: encode_hex(&nonce_bytes),
        ciphertext: encode_hex(&ciphertext),
    };
    ensure_token_store_dir()?;
    write_file_truncating(
        TOKEN_STORE_PATH,
        serde_json::to_string_pretty(&store).context("failed to serialize GFN token store")?,
    )
}

pub fn load_tokens() -> Option<AuthTokens> {
    load_tokens_inner().ok()
}

fn load_tokens_inner() -> Result<AuthTokens> {
    let data = std::fs::read_to_string(TOKEN_STORE_PATH).context("no saved GFN login")?;
    let store: EncryptedTokenStore =
        serde_json::from_str(&data).context("failed to parse GFN token store")?;
    if store.version != TOKEN_STORE_VERSION {
        bail!("unsupported GFN token store version {}", store.version);
    }
    let nonce = decode_hex(&store.nonce).context("invalid GFN token nonce")?;
    let nonce: [u8; TOKEN_NONCE_SIZE] = nonce
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid GFN token nonce length"))?;
    let mut ciphertext = decode_hex(&store.ciphertext).context("invalid GFN ciphertext")?;
    let key = load_token_key()?;
    let cipher = token_cipher(&key)?;
    let plaintext = cipher
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(TOKEN_AAD),
            &mut ciphertext,
        )
        .map_err(|_| anyhow::anyhow!("GFN token authentication failed"))?;
    serde_json::from_slice(plaintext).context("decrypted GFN token payload is not valid JSON")
}

pub fn clear_tokens() {
    let _ = std::fs::remove_file(TOKEN_STORE_PATH);
    let _ = safe_memory_save(TOKEN_KEY_OFFSET, &[0u8; TOKEN_KEY_RECORD_SIZE]);
}

fn token_cipher(key: &[u8; TOKEN_KEY_SIZE]) -> Result<LessSafeKey> {
    let key = UnboundKey::new(&aead::CHACHA20_POLY1305, key)
        .map_err(|_| anyhow::anyhow!("failed to initialize GFN token cipher"))?;
    Ok(LessSafeKey::new(key))
}

fn load_token_key() -> Result<[u8; TOKEN_KEY_SIZE]> {
    let record: [u8; TOKEN_KEY_RECORD_SIZE] = safe_memory_load(TOKEN_KEY_OFFSET)?;
    if &record[..TOKEN_KEY_MAGIC.len()] != TOKEN_KEY_MAGIC {
        bail!("GFN token key is missing from Safe Memory");
    }
    let mut key = [0u8; TOKEN_KEY_SIZE];
    key.copy_from_slice(&record[TOKEN_KEY_MAGIC.len()..]);
    Ok(key)
}

fn load_or_create_token_key() -> Result<[u8; TOKEN_KEY_SIZE]> {
    if let Ok(key) = load_token_key() {
        return Ok(key);
    }
    let mut key = [0u8; TOKEN_KEY_SIZE];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| anyhow::anyhow!("failed to generate GFN token key"))?;
    let mut record = [0u8; TOKEN_KEY_RECORD_SIZE];
    record[..TOKEN_KEY_MAGIC.len()].copy_from_slice(TOKEN_KEY_MAGIC);
    record[TOKEN_KEY_MAGIC.len()..].copy_from_slice(&key);
    safe_memory_save(TOKEN_KEY_OFFSET, &record)?;
    Ok(key)
}

fn safe_memory_load<const N: usize>(offset: i64) -> Result<[u8; N]> {
    crate::safe_memory::load::<N>(offset)
}

fn safe_memory_save(offset: i64, data: &[u8]) -> Result<()> {
    crate::safe_memory::save(offset, data)
}

fn ensure_token_store_dir() -> Result<()> {
    std::fs::create_dir_all(TOKEN_STORE_DIR).context("failed to create GFN token store directory")
}

fn write_file_truncating(path: &str, data: impl AsRef<[u8]>) -> Result<()> {
    let _ = std::fs::remove_file(path);
    std::fs::write(path, data).with_context(|| format!("failed to write {path}"))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn decode_hex(encoded: &str) -> Result<Vec<u8>> {
    if !encoded.len().is_multiple_of(2) {
        bail!("hex value has an odd length");
    }
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = decode_hex_digit(pair[0])?;
            let low = decode_hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn decode_hex_digit(digit: u8) -> Result<u8> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        b'A'..=b'F' => Ok(digit - b'A' + 10),
        _ => bail!("invalid hex digit"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved_tokens() -> AuthTokens {
        AuthTokens {
            access_token: "old-access".to_owned(),
            refresh_token: Some("old-refresh".to_owned()),
            id_token: Some("old-id".to_owned()),
            expires_at_unix: 1_000,
            client_token: Some("old-client".to_owned()),
            client_token_expires_at_unix: 2_000,
        }
    }

    fn response(access: &str) -> TokenResponse {
        TokenResponse {
            access_token: access.to_owned(),
            refresh_token: None,
            id_token: None,
            expires_in: Some(3_600),
            client_token: None,
        }
    }

    /// The bug this guards against is subtle and total: `bearer()` prefers `id_token`, so dropping
    /// an omitted one swaps the JWT CloudMatch requires for a token it refuses.
    #[test]
    fn refresh_keeps_the_id_token_nvidia_omitted() {
        let merged = merge_refreshed(&saved_tokens(), response("new-access"));
        assert_eq!(merged.access_token, "new-access");
        assert_eq!(merged.id_token.as_deref(), Some("old-id"));
        assert_eq!(merged.bearer(), "old-id");
    }

    #[test]
    fn refresh_keeps_the_refresh_and_client_tokens_when_omitted() {
        let merged = merge_refreshed(&saved_tokens(), response("new-access"));
        assert_eq!(merged.refresh_token.as_deref(), Some("old-refresh"));
        assert_eq!(merged.client_token.as_deref(), Some("old-client"));
        assert_eq!(merged.client_token_expires_at_unix, 2_000);
    }

    #[test]
    fn refresh_takes_new_values_when_nvidia_sends_them() {
        let mut fresh = response("new-access");
        fresh.id_token = Some("new-id".to_owned());
        fresh.refresh_token = Some("new-refresh".to_owned());
        let merged = merge_refreshed(&saved_tokens(), fresh);
        assert_eq!(merged.id_token.as_deref(), Some("new-id"));
        assert_eq!(merged.refresh_token.as_deref(), Some("new-refresh"));
    }

    #[test]
    fn a_rotated_client_token_invalidates_its_old_deadline() {
        let mut fresh = response("new-access");
        fresh.client_token = Some("rotated-client".to_owned());
        let merged = merge_refreshed(&saved_tokens(), fresh);
        assert_eq!(merged.client_token.as_deref(), Some("rotated-client"));
        assert_eq!(
            merged.client_token_expires_at_unix, 0,
            "a rotated client token needs a fresh lifetime from /client_token"
        );
    }

    #[test]
    fn refresh_window_fires_before_the_token_actually_dies() {
        let now = 10_000;
        assert!(should_refresh(now + 60, now), "inside the window");
        assert!(!should_refresh(now + REFRESH_WINDOW_SECS + 60, now));
        // An unknown expiry has to be treated as due, or it would never be renewed.
        assert!(should_refresh(0, now));
    }

    #[test]
    fn a_missing_client_token_counts_as_maintenance_due() {
        let mut tokens = saved_tokens();
        tokens.expires_at_unix = now_unix() + REFRESH_WINDOW_SECS * 10;
        tokens.client_token = None;
        assert!(
            tokens.needs_maintenance(),
            "an upgraded login with no client token must still get one"
        );
    }

    #[test]
    fn only_transient_statuses_are_retried() {
        assert!(is_temporary_status(None), "a network failure says nothing");
        assert!(is_temporary_status(Some(429)));
        assert!(is_temporary_status(Some(503)));
        assert!(!is_temporary_status(Some(400)));
        assert!(!is_temporary_status(Some(401)));
    }

    #[test]
    fn stored_tokens_without_a_client_token_still_load() {
        // Token stores written before the client-token fields existed must keep working.
        let legacy = r#"{"access_token":"a","refresh_token":"r","id_token":"i","expires_at_unix":5}"#;
        let tokens: AuthTokens = serde_json::from_str(legacy).expect("legacy store should load");
        assert_eq!(tokens.client_token, None);
        assert_eq!(tokens.client_token_expires_at_unix, 0);
        assert_eq!(tokens.bearer(), "i");
    }
}
