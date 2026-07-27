//! Game catalog fetch - needed before a streaming session can exist at all, since CloudMatch's
//! `POST /v2/session` requires a numeric `appId` (see docs/protocol-notes.md §2). Fase 3 (actual
//! session creation/streaming) builds on top of `GameSummary::app_id` from here.
//!
//! Uses the same plain (non-persisted-query) GraphQL endpoint the desktop GFN client calls for
//! its own catalog browsing (`games.geforce.com/graphql`), not the LCARS CDN's persisted-query
//! endpoint used for marketing/marquee panels - see
//! `opennow-stable/src/main/gfn/games.ts::fetchPaginatedLibraryApps`/`browseCatalogUncached`.
//!
//! First version of this module filtered `apps()` down to the account's own "added to my GFN
//! library" list (`variants.gfn.library.status.notEquals: "NOT_OWNED"`), matching the reference
//! client's own `fetchLibraryGames`. On a real account that turned out to return zero results:
//! "added to library" is a separate, opt-in concept from "launchable on GFN" - most people never
//! explicitly add anything and just search/launch directly. `browseCatalogUncached` in the
//! reference client passes an **empty** `filters: {}` when the user hasn't picked any browse
//! filter, which browses the whole live catalog instead - that's what this fetches now.
//!
//! Still simplified vs. that reference: no genre/filter UI and no persisted-query transport (we
//! always POST the literal document). Cursor pagination *is* implemented - see
//! [`fetch_catalog_page`] - but the caller decides how many pages to walk
//! (`app::MAX_CATALOG_PAGES`) rather than exhausting the catalog.

use super::headers::{self, error_for_status_with_body};
use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::OnceCell;

const GRAPHQL_ENDPOINT: &str = "https://games.geforce.com/graphql";
const CLOUDMATCH_BASE_URL: &str = "https://prod.cloudmatchbeta.nvidiagrid.net/";
const LOCALE: &str = "en_US";
/// Same default sort `browseCatalogUncached` falls back to when nothing more specific applies.
const CATALOG_SORT: &str = "itemMetadata.relevance:DESC,sortName:ASC";
/// Titles per page. 200 matches the reference client's own `LIBRARY_FETCH_COUNT` and is a hard
/// server ceiling, not a tuning knob: a larger value (2000 was tried) gets the request rejected
/// outright with HTTP 400, because the server validates `first` against some undocumented
/// maximum.
const CATALOG_PAGE_SIZE: u32 = 200;

#[derive(Debug, Clone)]
pub struct GameSummary {
    pub app_id: String,
    pub title: String,
    /// Best-effort poster-style cover URL (portrait box art). `None` if the catalog response
    /// carried no image fields at all - the grid just draws a placeholder tile then.
    pub cover_url: Option<String>,
    /// Storefront the launchable variant belongs to (`"STEAM"`, `"EPIC"`, `"EA_APP"`, ...),
    /// straight from GFN's `variant.appStore` - mirrors OpenNOW's `appToVariants` (`games.ts`).
    /// `None` if the matched variant didn't report one (some first-party/"GFN native" titles
    /// don't have a storefront at all).
    pub store: Option<String>,
    /// ISO-8601 timestamp of this account's last session for the title, from
    /// `variant.gfn.library.lastPlayedDate` - `None` for anything never launched from this
    /// account (i.e. most of the catalog). Powers the "recently played" sort.
    pub last_played: Option<String>,
    /// Lowercased `title`, computed once here so the per-keystroke filter and the title sorts in
    /// `app::filter_indices` never allocate. Before this existed, both lowercased on the fly -
    /// including *inside the sort comparator*, i.e. O(n log n) `String` allocations (~44k for a
    /// 2000-title catalog) on every single keystroke.
    ///
    /// Currently just the title. OpenNOW folds publisher/store/genre into the same haystack
    /// (`buildSearchText`, `games.ts`); this is where that would go.
    pub search_key: String,
}

#[derive(Debug, Deserialize)]
struct ServerInfoResponse {
    #[serde(rename = "requestStatus")]
    request_status: ServerInfoRequestStatus,
}

#[derive(Debug, Deserialize)]
struct ServerInfoRequestStatus {
    #[serde(rename = "serverId")]
    server_id: Option<String>,
}

/// The "VPC id" CloudMatch expects on catalog/session calls - not documented anywhere beyond
/// `requestStatus.serverId` showing up in `serverInfo` responses (see protocol notes §2).
pub async fn fetch_vpc_id(client: &Client, token: &str) -> Result<String> {
    let response = headers::apply_lcars_headers(
        client.get(format!("{CLOUDMATCH_BASE_URL}v2/serverInfo")),
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
    payload
        .request_status
        .server_id
        .context("serverInfo response did not include a VPC id")
}

#[derive(Debug, Deserialize)]
struct GraphQlEnvelope<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct CatalogData {
    apps: CatalogApps,
}

#[derive(Debug, Deserialize)]
struct CatalogApps {
    items: Vec<CatalogAppItem>,
    #[serde(default, rename = "pageInfo")]
    page_info: Option<CatalogPageInfo>,
}

#[derive(Debug, Deserialize, Default)]
struct CatalogPageInfo {
    #[serde(default, rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(default, rename = "endCursor")]
    end_cursor: Option<String>,
    #[serde(default, rename = "totalCount")]
    total_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct CatalogAppItem {
    id: String,
    title: String,
    #[serde(default)]
    variants: Vec<CatalogAppVariant>,
    /// Mirrors the field shape the official client requests (`games.ts` line 1014:
    /// `images { ... KEY_ART KEY_IMAGE GAME_BOX_ART ... }`). Each value is either a single
    /// URL string or an array of URL strings (depending on the image kind); we capture both
    /// shapes and pick the first non-empty entry. Missing entirely if the catalog entry has
    /// no artwork published.
    #[serde(default)]
    images: Option<CatalogAppImages>,
}

#[derive(Debug, Deserialize)]
struct CatalogAppVariant {
    id: String,
    #[serde(default, rename = "appStore")]
    app_store: Option<String>,
    #[serde(default)]
    gfn: Option<CatalogAppVariantGfn>,
}

/// Only the `library.lastPlayedDate` leaf of `variant.gfn` - mirrors OpenNOW's
/// `variant.gfn?.library?.lastPlayedDate` (`games.ts:585`). Populated only for variants the
/// account has actually launched before; most catalog entries won't have one.
#[derive(Debug, Deserialize)]
struct CatalogAppVariantGfn {
    #[serde(default)]
    library: Option<CatalogAppVariantLibrary>,
}

#[derive(Debug, Deserialize)]
struct CatalogAppVariantLibrary {
    #[serde(default, rename = "lastPlayedDate")]
    last_played_date: Option<String>,
}

impl CatalogAppVariant {
    fn last_played_date(&self) -> Option<&str> {
        self.gfn.as_ref()?.library.as_ref()?.last_played_date.as_deref()
    }
}

#[derive(Debug, Deserialize, Default)]
struct CatalogAppImages {
    /// Box art (portrait poster) - preferred for grid covers.
    #[serde(default, rename = "GAME_BOX_ART")]
    game_box_art: ImageField,
    /// Square key image - second preference (some titles ship only this).
    #[serde(default, rename = "KEY_IMAGE")]
    key_image: ImageField,
    /// Wide key art - third preference (latest fallback).
    #[serde(default, rename = "KEY_ART")]
    key_art: ImageField,
}

/// Catalog image values arrive as either a single URL (`"..."`) or an array (`["...", ...]`);
/// `ImageField` accepts both generously and exposes a `first()` accessor.
#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum ImageField {
    #[default]
    Empty,
    Single(String),
    Many(Vec<String>),
}

impl ImageField {
    fn first(&self) -> Option<&str> {
        match self {
            ImageField::Empty => None,
            ImageField::Single(s) => Some(s.as_str()),
            ImageField::Many(list) => list.first().map(|s| s.as_str()),
        }
    }
}

impl CatalogAppImages {
    /// Same preference order as OpenNOW's `POSTER_IMAGE_KEYS` (`games.ts` line 382):
    /// GAME_BOX_ART > KEY_IMAGE > KEY_ART.
    fn poster_url(&self) -> Option<String> {
        self.game_box_art
            .first()
            .or_else(|| self.key_image.first())
            .or_else(|| self.key_art.first())
            .map(|url| optimize_image(url))
    }
}

/// NVIDIA's `img.nvidiagrid.net` CDN accepts URL-fragment suffixes like `;f=jpeg;w=300` to
/// transcode/resize on the fly (see `optimizeImage` in `games.ts` line 384). The official
/// client asks for `f=webp`, which our decoder doesn't support - we ask for `jpeg` instead,
/// which the same CDN happily serves. Non-nvidiagrid URLs (rare for catalog covers) are
/// returned as-is.
fn optimize_image(url: &str) -> String {
    if url.contains("img.nvidiagrid.net") {
        format!("{url};f=jpeg;w=256")
    } else {
        url.to_owned()
    }
}

/// Field selection shared by both catalog queries, including the cursor-pagination metadata.
/// Mirrors the reference client's `appFields` fragment (`games.ts`).
const CATALOG_PAGE_FIELDS: &str = r#"
    items {
      id
      title
      variants { id appStore gfn { library { lastPlayedDate } } }
      images { GAME_BOX_ART KEY_IMAGE KEY_ART }
    }
    pageInfo { hasNextPage endCursor totalCount }
"#;

/// Browse one page of the catalog. `$cursor` is **non-nullable** (`String!`) and the first page
/// passes the empty string, matching the reference client - sending `null` is a GraphQL
/// validation error.
fn catalog_query() -> String {
    format!(
        r#"
query GetCatalogApps(
  $vpcId: String!,
  $locale: String!,
  $sortString: String!,
  $fetchCount: Int!,
  $cursor: String!,
  $filters: AppFilterFields!
) {{
  apps(vpcId: $vpcId, language: $locale, orderBy: $sortString, first: $fetchCount, after: $cursor, filters: $filters) {{
{CATALOG_PAGE_FIELDS}
  }}
}}
"#
    )
}

/// Same shape as [`catalog_query`] plus the `searchQuery` argument - matches the reference
/// client's `GetSearchFilterResults` (`games.ts`). Passing the search term to the server instead
/// of filtering browse results locally is what lets search reach the *entire* live catalog
/// rather than only the pages we happen to have fetched.
fn catalog_search_query() -> String {
    format!(
        r#"
query GetCatalogSearchApps(
  $vpcId: String!,
  $locale: String!,
  $sortString: String!,
  $fetchCount: Int!,
  $cursor: String!,
  $searchString: String!,
  $filters: AppFilterFields!
) {{
  apps(vpcId: $vpcId, language: $locale, orderBy: $sortString, first: $fetchCount, after: $cursor, searchQuery: $searchString, filters: $filters) {{
{CATALOG_PAGE_FIELDS}
  }}
}}
"#
    )
}

/// One page of catalog results plus the cursor needed to ask for the next one.
#[derive(Debug, Clone)]
pub struct CatalogPage {
    pub games: Vec<GameSummary>,
    /// `pageInfo.endCursor`, `Some` only when `hasNextPage` was true *and* the cursor is
    /// non-empty. The reference client treats an empty cursor as "stop" even when the server
    /// claims another page exists, and so do we - otherwise a server quirk becomes an infinite
    /// request loop.
    pub next_cursor: Option<String>,
    /// `pageInfo.totalCount` - how many titles match in total, which is generally far more than
    /// we will ever page in. Shown in the catalog header so a truncated list is explicable.
    pub total_count: Option<usize>,
}

/// Fetches one page. `query` is `None` to browse, `Some(q)` to run a server-side search;
/// `cursor` is `""` for the first page and a previous page's `next_cursor` thereafter.
pub async fn fetch_catalog_page(
    client: &Client,
    token: &str,
    vpc_id: &str,
    query: Option<&str>,
    cursor: &str,
) -> Result<CatalogPage> {
    let (document, label) = match query {
        Some(_) => (catalog_search_query(), "catalog search"),
        None => (catalog_query(), "catalog"),
    };
    let mut variables = json!({
        "vpcId": vpc_id,
        "locale": LOCALE,
        "sortString": CATALOG_SORT,
        "fetchCount": CATALOG_PAGE_SIZE,
        "cursor": cursor,
        // Empty on purpose - see module docs. A non-empty filter here narrows to a specific
        // genre/store/etc, which is what the reference client's filter UI builds up; we have
        // no such UI yet.
        "filters": {},
    });
    if let Some(query) = query {
        variables["searchString"] = json!(query);
    }
    run_catalog_query(client, token, json!({ "query": document, "variables": variables }), label)
        .await
}

async fn run_catalog_query(
    client: &Client,
    token: &str,
    body: serde_json::Value,
    context_label: &str,
) -> Result<CatalogPage> {
    let response = headers::apply_graphql_headers(client.post(GRAPHQL_ENDPOINT), token)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("{context_label} GraphQL request failed"))?;
    let response = error_for_status_with_body(response).await?;

    let envelope: GraphQlEnvelope<CatalogData> = response
        .json()
        .await
        .with_context(|| format!("failed to decode {context_label} GraphQL response"))?;

    if let Some(errors) = envelope.errors.filter(|errors| !errors.is_empty()) {
        bail!(
            "{context_label} GraphQL errors: {}",
            errors
                .into_iter()
                .map(|error| error.message)
                .collect::<Vec<_>>()
                .join("; ")
        );
    }

    let data = envelope
        .data
        .with_context(|| format!("{context_label} GraphQL response had no data"))?;
    let page_info = data.apps.page_info.unwrap_or_default();
    let next_cursor = page_info
        .end_cursor
        .filter(|cursor| page_info.has_next_page && !cursor.is_empty());

    Ok(CatalogPage {
        games: data.apps.items.into_iter().map(to_game_summary).collect(),
        next_cursor,
        total_count: page_info.total_count,
    })
}

/// Shared `CatalogAppItem` -> `GameSummary` mapping for both catalog queries above - they
/// request the same item shape (`id`, `title`, `variants`, `images`).
fn to_game_summary(item: CatalogAppItem) -> GameSummary {
    let numeric_variant = item
        .variants
        .iter()
        .find(|v| v.id.chars().all(|c| c.is_ascii_digit()));
    let numeric_app_id = numeric_variant
        .map(|v| v.id.clone())
        .or_else(|| {
            if item.id.chars().all(|c| c.is_ascii_digit()) {
                Some(item.id.clone())
            } else {
                item.variants.first().map(|v| v.id.clone())
            }
        })
        .unwrap_or_else(|| item.id.clone());
    // Store badge: same variant the numeric app id came from when there was one,
    // otherwise whichever variant is first - either way, "some plausible storefront"
    // beats showing nothing.
    let store = numeric_variant
        .or_else(|| item.variants.first())
        .and_then(|v| v.app_store.clone());
    // Same "find the first variant that actually has one" approach as OpenNOW's
    // `resolveAppData` (`games.ts:585`) - which specific variant reports a play date
    // doesn't matter, only whether the account has played *a* launchable form of this
    // title before.
    let last_played = item
        .variants
        .iter()
        .find_map(|v| v.last_played_date())
        .map(str::to_owned);

    GameSummary {
        cover_url: item.images.as_ref().and_then(|images| images.poster_url()),
        app_id: numeric_app_id,
        search_key: item.title.to_lowercase(),
        title: item.title,
        store,
        last_played,
    }
}

/// Process-lifetime cache for the account's VPC id, shared with every spawned catalog task.
///
/// The id is stable for the session but was previously re-fetched before *every* catalog call,
/// so each debounced keystroke cost two HTTPS round trips instead of one on a console with a
/// single Wi-Fi radio.
pub type VpcIdCache = Arc<OnceCell<String>>;

/// What to use when `/v2/serverInfo` can't be reached. Same literal the reference client falls
/// back to (`getVpcId`, `games.ts`).
const FALLBACK_VPC_ID: &str = "GFN-PC";

/// Returns the cached VPC id, fetching it on first use.
///
/// On failure this returns [`FALLBACK_VPC_ID`] **without caching it**, so a transient
/// `serverInfo` blip doesn't pin the whole session to the fallback - the next call retries. The
/// failure is logged loudly on purpose: if GFN ever stops accepting the fallback, the symptom is
/// an empty catalog behind a successful HTTP 200, which is miserable to diagnose from a Vita.
pub async fn resolve_vpc_id(client: &Client, token: &str, cache: &VpcIdCache) -> String {
    if let Some(cached) = cache.get() {
        return cached.clone();
    }
    match fetch_vpc_id(client, token).await {
        Ok(vpc_id) => {
            let _ = cache.set(vpc_id.clone());
            vpc_id
        }
        Err(error) => {
            eprintln!(
                "serverInfo VPC id lookup failed, falling back to {FALLBACK_VPC_ID}: {error:#}"
            );
            FALLBACK_VPC_ID.to_owned()
        }
    }
}

/// Resolves the VPC id (cached) and fetches one catalog page - the pair every caller needs
/// together.
pub async fn fetch_catalog_page_for_account(
    client: &Client,
    token: &str,
    cache: &VpcIdCache,
    query: Option<&str>,
    cursor: &str,
) -> Result<CatalogPage> {
    let vpc_id = resolve_vpc_id(client, token, cache).await;
    fetch_catalog_page(client, token, &vpc_id, query, cursor).await
}
