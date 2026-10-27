//! Shared GFN request headers/identity constants.

use anyhow::{Result, bail};
use reqwest::Response;

/// `Response::error_for_status()` discards the response body, which for GraphQL/REST error
/// responses is usually where the actually-useful message lives (e.g.
pub async fn error_for_status_with_body(response: Response) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| format!("<failed to read response body: {error}>"));
    bail!(
        "HTTP {status}: {}",
        body.chars().take(500).collect::<String>()
    );
}

pub const CLIENT_ID: &str = "ec7e38d4-03af-4b58-b131-cfb0495903ab";
pub const CLIENT_VERSION: &str = "2.0.89.154";
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36";
pub const PLAY_ORIGIN: &str = "https://play.geforcenow.com";
pub const PLAY_REFERER: &str = "https://play.geforcenow.com/";

pub fn jwt_authorization(token: &str) -> String {
    format!("GFNJWT {token}")
}

/// Headers for the CloudMatch/"LCARS" REST endpoints (`serverInfo`, catalog, session
/// create/poll/stop).
pub fn apply_lcars_headers(
    builder: reqwest::RequestBuilder,
    token: &str,
    client_streamer: &str,
) -> reqwest::RequestBuilder {
    builder
        .header("Accept", "application/json")
        .header("Authorization", jwt_authorization(token))
        .header("nv-client-id", CLIENT_ID)
        .header("nv-client-type", "BROWSER")
        .header("nv-client-version", CLIENT_VERSION)
        .header("nv-client-streamer", client_streamer)
        .header("nv-device-os", "WINDOWS")
        .header("nv-device-type", "DESKTOP")
        .header("User-Agent", USER_AGENT)
}

/// Headers for the CloudMatch session create/poll/stop endpoints.
pub fn apply_cloudmatch_headers(
    builder: reqwest::RequestBuilder,
    token: &str,
    client_id: &str,
    device_id: &str,
) -> reqwest::RequestBuilder {
    builder
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("Authorization", jwt_authorization(token))
        .header("Origin", PLAY_ORIGIN)
        .header("Referer", PLAY_REFERER)
        .header("nv-browser-type", "CHROME")
        .header("nv-client-id", client_id)
        .header("nv-client-streamer", "WEBRTC")
        .header("nv-client-type", "BROWSER")
        .header("nv-client-version", CLIENT_VERSION)
        .header("nv-device-make", "UNKNOWN")
        .header("nv-device-model", "UNKNOWN")
        .header("nv-device-os", "WINDOWS")
        .header("nv-device-type", "DESKTOP")
        .header("x-device-id", device_id)
        .header("User-Agent", USER_AGENT)
}

/// Headers for the plain (non-persisted-query) `games.geforce.com/graphql` endpoint used for
/// library/catalog lookups.
pub fn apply_graphql_headers(
    builder: reqwest::RequestBuilder,
    token: &str,
) -> reqwest::RequestBuilder {
    builder
        .header("Accept", "application/json, text/plain, */*")
        .header("Content-Type", "application/json")
        .header("Origin", PLAY_ORIGIN)
        .header("Referer", PLAY_REFERER)
        .header("Authorization", jwt_authorization(token))
        .header("nv-client-id", CLIENT_ID)
        .header("nv-client-type", "NATIVE")
        .header("nv-client-version", CLIENT_VERSION)
        .header("nv-client-streamer", "NVIDIA-CLASSIC")
        .header("nv-device-os", "WINDOWS")
        .header("nv-device-type", "DESKTOP")
        .header("nv-device-make", "UNKNOWN")
        .header("nv-device-model", "UNKNOWN")
        .header("nv-browser-type", "CHROME")
        .header("User-Agent", USER_AGENT)
}
