
use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

const QUEUE_API_URL: &str = "https://api.printedwaste.com/gfn/queue";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);

const MAX_AGE_SECONDS: u64 = 30 * 60;

#[derive(Debug, Clone, Deserialize)]
struct QueueEntry {
    #[serde(rename = "QueuePosition")]
    queue_position: Option<u32>,
    #[serde(rename = "eta")]
    eta_ms: Option<u64>,
    #[serde(rename = "Last Updated")]
    last_updated: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct QueueResponse {
    #[serde(default)]
    status: bool,
    #[serde(default)]
    data: HashMap<String, QueueEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueReading {
    pub queue_position: u32,
    pub eta_seconds: Option<u64>,
}

pub type QueueMap = HashMap<String, QueueReading>;

pub fn server_code_from_url(url: &str) -> Option<String> {
    let authority = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .split('/')
        .next()?;
    let label = authority.split('.').next()?;
    if !label.contains('-') {
        return None;
    }
    if !label
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        return None;
    }
    let code = label.to_ascii_uppercase();
    code.starts_with("NP").then_some(code)
}

pub async fn fetch_queue(client: &Client) -> Result<QueueMap> {
    let response = client
        .get(QUEUE_API_URL)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .context("queue API request failed")?
        .error_for_status()
        .context("queue API returned an error status")?;

    let payload: QueueResponse = response
        .json()
        .await
        .context("failed to decode the queue API response")?;
    if !payload.status {
        anyhow::bail!("queue API reported failure");
    }

    let now = unix_time_seconds();
    let mut readings = QueueMap::new();
    let mut stale = 0usize;
    for (code, entry) in payload.data {
        let Some(queue_position) = entry.queue_position else {
            continue;
        };
        if let (Some(updated), Some(now)) = (entry.last_updated, now) {
            if now.saturating_sub(updated) > MAX_AGE_SECONDS {
                stale += 1;
                continue;
            }
        }
        readings.insert(
            code.to_ascii_uppercase(),
            QueueReading {
                queue_position,
                eta_seconds: entry.eta_ms.map(|ms| ms / 1000),
            },
        );
    }

    crate::log_info!(
        "Queue stats: {} live server(s), {stale} stale reading(s) ignored",
        readings.len()
    );
    Ok(readings)
}

fn unix_time_seconds() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zone_endpoint_yields_its_server_code() {
        assert_eq!(
            server_code_from_url("https://np-ams-02.cloudmatchbeta.nvidiagrid.net/"),
            Some("NP-AMS-02".to_owned())
        );
        assert_eq!(
            server_code_from_url("https://npa-gkr-sel-01.cloudmatchbeta.nvidiagrid.net/v2/"),
            Some("NPA-GKR-SEL-01".to_owned())
        );
    }

    #[test]
    fn non_server_hosts_yield_nothing() {
        assert_eq!(
            server_code_from_url("https://prod.cloudmatchbeta.nvidiagrid.net/"),
            None
        );
        assert_eq!(server_code_from_url("https://example.com/"), None);
        assert_eq!(server_code_from_url(""), None);
    }

    #[test]
    fn stale_readings_are_dropped() {
        let body = r#"{"status":true,"errors":[],"data":{
            "NP-FRESH-01":{"QueuePosition":4,"eta":144000,"Last Updated":4000000000},
            "NP-STALE-01":{"QueuePosition":3,"eta":138000,"Last Updated":1000}
        }}"#;
        let payload: QueueResponse = serde_json::from_str(body).expect("body should decode");
        let now = 4_000_000_100u64;
        let live: Vec<&String> = payload
            .data
            .iter()
            .filter(|(_, entry)| {
                entry
                    .last_updated
                    .is_some_and(|updated| now.saturating_sub(updated) <= MAX_AGE_SECONDS)
            })
            .map(|(code, _)| code)
            .collect();
        assert_eq!(live, vec![&"NP-FRESH-01".to_owned()]);
    }
}
