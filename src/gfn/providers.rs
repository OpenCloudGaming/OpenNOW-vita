
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const SERVICE_URLS_ENDPOINT: &str = "https://pcs.geforcenow.com/v1/serviceUrls";
pub const DEFAULT_NVIDIA_IDP_ID: &str = "PDiAhv2kJTFeQ7WOPqiQ2tRZ7lGhR2X11dXvM4TZSxg";
pub const DEFAULT_NVIDIA_STREAMING_URL: &str = "https://prod.cloudmatchbeta.nvidiagrid.net/";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; Steam Deck) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GfnProvider {
    pub code: String,
    pub display_name: String,
    pub idp_id: String,
    pub streaming_service_url: String,
    #[serde(default)]
    pub priority: i32,
}

impl Default for GfnProvider {
    fn default() -> Self {
        Self {
            code: "NVIDIA".to_owned(),
            display_name: "NVIDIA".to_owned(),
            idp_id: DEFAULT_NVIDIA_IDP_ID.to_owned(),
            streaming_service_url: DEFAULT_NVIDIA_STREAMING_URL.to_owned(),
            priority: 1,
        }
    }
}

impl GfnProvider {
    pub fn is_nvidia(&self) -> bool {
        self.code.trim().eq_ignore_ascii_case("NVIDIA")
    }

    pub fn normalized_streaming_url(&self) -> String {
        if self.streaming_service_url.ends_with('/') {
            self.streaming_service_url.clone()
        } else {
            format!("{}/", self.streaming_service_url)
        }
    }
}

#[derive(Debug, Deserialize)]
struct ServiceUrlsResponse {
    #[serde(default, rename = "gfnServiceInfo")]
    gfn_service_info: Option<GfnServiceInfo>,
}

#[derive(Debug, Deserialize)]
struct GfnServiceInfo {
    #[serde(default, rename = "defaultProvider")]
    default_provider: Option<String>,
    #[serde(default, rename = "clientCountryCode")]
    client_country_code: Option<String>,
    #[serde(default, rename = "loginPreferredProviders")]
    login_preferred_providers: Vec<String>,
    #[serde(default, rename = "gfnServiceEndpoints")]
    gfn_service_endpoints: Vec<GfnServiceEndpoint>,
}

#[derive(Debug, Deserialize)]
struct GfnServiceEndpoint {
    #[serde(rename = "loginProviderCode")]
    login_provider_code: String,
    #[serde(rename = "loginProviderDisplayName")]
    login_provider_display_name: String,
    #[serde(rename = "idpId")]
    idp_id: String,
    #[serde(rename = "streamingServiceUrl")]
    streaming_service_url: String,
    #[serde(default, rename = "loginProviderPriority")]
    login_provider_priority: Option<i32>,
}

pub async fn discover_providers(client: &Client) -> Result<(GfnProvider, Vec<GfnProvider>)> {
    let response = client
        .get(SERVICE_URLS_ENDPOINT)
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .context("failed to request service URLs")?;

    let payload: ServiceUrlsResponse = response
        .json()
        .await
        .context("failed to parse service URLs JSON")?;

    let Some(service_info) = payload.gfn_service_info else {
        return Ok((GfnProvider::default(), vec![GfnProvider::default()]));
    };

    let mut providers: Vec<GfnProvider> = service_info
        .gfn_service_endpoints
        .into_iter()
        .map(|ep| GfnProvider {
            display_name: friendly_name(&ep.login_provider_code, ep.login_provider_display_name),
            code: ep.login_provider_code,
            idp_id: ep.idp_id,
            streaming_service_url: if ep.streaming_service_url.ends_with('/') {
                ep.streaming_service_url
            } else {
                format!("{}/", ep.streaming_service_url)
            },
            priority: ep.login_provider_priority.unwrap_or(100),
        })
        .filter(is_valid)
        .collect();

    providers.sort_by_key(|p| p.priority);

    if providers.is_empty() {
        return Ok((GfnProvider::default(), vec![GfnProvider::default()]));
    }

    let preferred = if let Some(pref_name) = service_info.login_preferred_providers.first() {
        providers
            .iter()
            .find(|p| p.display_name.eq_ignore_ascii_case(pref_name) || p.code.eq_ignore_ascii_case(pref_name))
            .cloned()
    } else if let Some(default_code) = &service_info.default_provider {
        providers
            .iter()
            .find(|p| p.code.eq_ignore_ascii_case(default_code) || p.display_name.eq_ignore_ascii_case(default_code))
            .cloned()
    } else {
        None
    }
    .unwrap_or_else(|| providers[0].clone());

    save_cached(&preferred, &providers);
    Ok((preferred, providers))
}

// the same display fixes OpenNOW and Kasumi apply: a partner's own name hides where it serves,
// so "au" read as Australia to players there while it is KDDI in Japan
fn friendly_name(code: &str, name: String) -> String {
    match code {
        "BPC" => "bro.game",
        "KDD" => "au (KDDI, Japan)",
        "PNT" => "Cloud.GG (Australia)",
        "TWM" => "Taiwan Mobile (Taiwan)",
        "STR" => "StarHub (Singapore)",
        "GKR" => "GFN Korea",
        "RAN" => "rain (South Africa)",
        "DIG" => "Digevo (Latin America)",
        _ => return name,
    }
    .to_owned()
}

// these values come from the network and end up in a form body and a request URL. same policy as
// OpenNOW's trusted_cloudmatch_base: https on 443, no credentials, a public host. Kasumi's
// "*.nvidiagrid.net only" is stricter and would hide a partner hosted elsewhere
fn is_valid(provider: &GfnProvider) -> bool {
    let Some(rest) = provider.streaming_service_url.trim().strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let (host, port) = authority.rsplit_once(':').unwrap_or((authority, "443"));
    let public_host = match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => {
            !(ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified())
        }
        Err(_) => host.contains('.') && !host.ends_with(".local") && !host.contains(['[', ']']),
    };
    !provider.code.is_empty()
        && port == "443"
        && !authority.contains('@')
        && public_host
        && !provider.idp_id.is_empty()
        && provider
            .idp_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

const CACHE_PATH: &str = "ux0:data/opennow-vita/providers.json";

#[derive(Serialize, Deserialize)]
struct ProviderCache {
    recommended: String,
    providers: Vec<GfnProvider>,
}

fn save_cached(recommended: &GfnProvider, providers: &[GfnProvider]) {
    let cache = ProviderCache {
        recommended: recommended.code.clone(),
        providers: providers.to_vec(),
    };
    if let Ok(json) = serde_json::to_string(&cache) {
        let _ = std::fs::write(CACHE_PATH, json);
    }
}

/// Last provider list fetched, so the login screen has a picker without waiting on the network.
/// Returns the list and the code NVIDIA recommends for this country.
pub fn load_cached() -> Option<(Vec<GfnProvider>, String)> {
    let cache: ProviderCache =
        serde_json::from_str(&std::fs::read_to_string(CACHE_PATH).ok()?).ok()?;
    let providers: Vec<_> = cache.providers.into_iter().filter(is_valid).collect();
    (!providers.is_empty()).then_some((providers, cache.recommended))
}
