
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
            display_name: if ep.login_provider_code == "BPC" {
                "bro.game".to_owned()
            } else {
                ep.login_provider_display_name
            },
            code: ep.login_provider_code,
            idp_id: ep.idp_id,
            streaming_service_url: if ep.streaming_service_url.ends_with('/') {
                ep.streaming_service_url
            } else {
                format!("{}/", ep.streaming_service_url)
            },
            priority: ep.login_provider_priority.unwrap_or(100),
        })
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

    Ok((preferred, providers))
}
