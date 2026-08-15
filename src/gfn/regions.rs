
use super::headers::{self, error_for_status_with_body};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::{Duration, Instant};

const CLOUDMATCH_BASE_URL: &str = "https://prod.cloudmatchbeta.nvidiagrid.net/";


const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

const LATENCY_SAMPLES: u32 = 3;

const PARALLEL_PROBES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamRegion {

    pub name: String,

    pub url: String,
  
    pub ping_ms: Option<u32>,
}
#[derive(Debug, Deserialize)]
struct ServerInfoResponse {
    #[serde(default, rename = "metaData")]
    meta_data: Vec<MetaEntry>,
}

#[derive(Debug, Deserialize)]
struct MetaEntry {
    #[serde(default)]
    key: String,
    #[serde(default)]
    value: String,
}

pub fn normalize_base_url(value: &str) -> Option<String> {
    let value = value.trim();
    let rest = value.strip_prefix("https://")?;
    if rest.is_empty() {
        return None;
    }
    let authority = rest.split('/').next()?;
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return None;
    }
    Some(if value.ends_with('/') {
        value.to_owned()
    } else {
        format!("{value}/")
    })
}

fn host_and_port(url: &str) -> Option<(String, u16)> {
    let authority = url.strip_prefix("https://")?.split('/').next()?;
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => {
            Some((host.to_owned(), port.parse().unwrap_or(443)))
        }
        _ => Some((authority.to_owned(), 443)),
    }
}

pub async fn fetch_regions(client: &Client, token: &str) -> Result<Vec<StreamRegion>> {
    let provider_url = super::auth::load_tokens()
        .and_then(|t| t.provider)
        .map(|p| p.normalized_streaming_url());
    let base_url = provider_url.as_deref().unwrap_or(CLOUDMATCH_BASE_URL);
    let base_url = if base_url.ends_with('/') {
        base_url.to_owned()
    } else {
        format!("{base_url}/")
    };
    let response = headers::apply_lcars_headers(
        client.get(format!("{base_url}v2/serverInfo")),
        token,
        "WEBRTC",
    )
    .send()
    .await
    .context("serverInfo request failed")?;
    let response = error_for_status_with_body(response).await?;

    let payload: ServerInfoResponse = response
        .json()
        .await
        .context("failed to decode serverInfo response")?;

    let mut regions: Vec<StreamRegion> = Vec::new();
    for entry in payload.meta_data {
        let name = entry.key.trim();
        if name.is_empty() || name.starts_with("gfn-") {
            continue;
        }
        let Some(url) = normalize_base_url(&entry.value) else {
            continue;
        };
        if regions.iter().any(|existing| existing.url == url) {
            continue;
        }
        regions.push(StreamRegion {
            name: name.to_owned(),
            url,
            ping_ms: None,
        });
    }

    regions.sort_by(|left, right| left.name.cmp(&right.name));
    crate::log_info!("Regions: serverInfo advertised {} zone(s)", regions.len());
    for region in &regions {
        crate::log_info!(
            "Regions: zone {:?} -> {} (queue key {:?})",
            region.name,
            region.url,
            crate::gfn::queue_stats::server_code_from_url(&region.url)
        );
    }
    Ok(regions)
}

async fn connect_once(host: &str, port: u16) -> Option<u32> {
    let started = Instant::now();
    let stream = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    .ok()?
    .ok()?;
    drop(stream);
    Some(started.elapsed().as_millis() as u32)
}

pub async fn measure_latency(url: &str) -> Option<u32> {
    let (host, port) = host_and_port(url)?;

    let _ = connect_once(&host, port).await;

    let mut total_ms = 0u32;
    let mut answered = 0u32;
    for sample in 0..LATENCY_SAMPLES {
        if sample > 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if let Some(ms) = connect_once(&host, port).await {
            total_ms += ms;
            answered += 1;
        }
    }

    (answered > 0).then(|| (total_ms + answered / 2) / answered)
}

pub async fn measure_all(regions: Vec<StreamRegion>) -> Vec<StreamRegion> {
    let mut measured = Vec::with_capacity(regions.len());

    for chunk in regions.chunks(PARALLEL_PROBES) {
        let handles: Vec<_> = chunk
            .iter()
            .map(|region| {
                let url = region.url.clone();
                tokio::spawn(async move { measure_latency(&url).await })
            })
            .collect();

        for (region, handle) in chunk.iter().zip(handles) {
            let ping_ms = handle.await.ok().flatten();
            measured.push(StreamRegion {
                ping_ms,
                ..region.clone()
            });
        }
    }

    crate::log_info!(
        "Regions: latency sweep done - {}",
        measured
            .iter()
            .map(|region| match region.ping_ms {
                Some(ms) => format!("{}={ms}ms", region.name),
                None => format!("{}=unreachable", region.name),
            })
            .collect::<Vec<_>>()
            .join(" ")
    );
    measured
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_https_url_gains_a_trailing_slash() {
        assert_eq!(
            normalize_base_url("https://prod-eu.example.net"),
            Some("https://prod-eu.example.net/".to_owned())
        );
        assert_eq!(
            normalize_base_url("  https://prod-eu.example.net/  "),
            Some("https://prod-eu.example.net/".to_owned())
        );
    }

    #[test]
    fn anything_other_than_a_plain_https_url_is_refused() {
        assert_eq!(normalize_base_url("http://insecure.example.net"), None);
        assert_eq!(normalize_base_url("https://"), None);
        assert_eq!(normalize_base_url("https://user@evil.example.net"), None);
        assert_eq!(normalize_base_url("https://host.example\n.net"), None);
        assert_eq!(normalize_base_url("not a url"), None);
        assert_eq!(normalize_base_url(""), None);
    }

    #[test]
    fn host_and_port_defaults_to_443() {
        assert_eq!(
            host_and_port("https://prod-eu.example.net/"),
            Some(("prod-eu.example.net".to_owned(), 443))
        );
        assert_eq!(
            host_and_port("https://prod-eu.example.net:8443/v2/"),
            Some(("prod-eu.example.net".to_owned(), 8443))
        );
    }

    #[test]
    fn config_blobs_are_not_mistaken_for_zones() {
        let body = r#"{"metaData":[
            {"key":"gfn-regions","value":"https://config.example.net/"},
            {"key":"EU Central","value":"https://prod-eu.example.net"},
            {"key":"US West","value":"not-a-url"},
            {"key":"","value":"https://nameless.example.net"}
        ]}"#;
        let payload: ServerInfoResponse = serde_json::from_str(body).expect("body should decode");
        let names: Vec<String> = payload
            .meta_data
            .into_iter()
            .filter(|entry| {
                let name = entry.key.trim();
                !name.is_empty() && !name.starts_with("gfn-")
            })
            .filter(|entry| normalize_base_url(&entry.value).is_some())
            .map(|entry| entry.key)
            .collect();
        assert_eq!(names, vec!["EU Central".to_owned()]);
    }
}
