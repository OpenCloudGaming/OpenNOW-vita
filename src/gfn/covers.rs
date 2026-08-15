//! Cover-art cache + background download.

use anyhow::{Context, Result};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// Maximum simultaneous cover downloads.
const MAX_CONCURRENT_COVER_DOWNLOADS: usize = 8;

/// How many decoded covers stay resident. At the 256px decode cap a cover is roughly 200 KB, so
/// this budget is a few MB - cheap next to re-downloading one over the Vita's wifi.
pub const MAX_CACHED_COVERS: usize = 24;

/// How many decoded row thumbnails stay resident. Must comfortably exceed a full library's worth
/// of rows: at 24 a list of 29 titles evicted thumbnails faster than scrolling could redraw them,
/// so every scroll re-downloaded what had just been shown. 64px RGBA is 16 KB apiece.
pub const MAX_CACHED_ICONS: usize = 256;

/// Where downloaded cover bytes are kept between runs, alongside the token store.
const COVER_DISK_CACHE_DIR: &str = "ux0:data/opennow-vita/covers";

/// How long a failed cover download is remembered before another attempt is allowed.
const COVER_RETRY_AFTER: Duration = Duration::from_secs(30);

/// Decoded RGBA cover image, with a lazily-initialized egui texture.
pub struct TitleImage {
    rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    texture: OnceLock<egui::TextureHandle>,
}

impl TitleImage {
    fn new(rgba: Vec<u8>, width: u32, height: u32) -> Self {
        Self {
            rgba,
            width,
            height,
            texture: OnceLock::new(),
        }
    }

    /// Lazily uploads the RGBA to the egui context.
    pub fn texture(&self, ctx: &egui::Context, key: impl FnOnce() -> String) -> &egui::TextureHandle {
        self.texture.get_or_init(|| {
            ctx.load_texture(
                key(),
                egui::ColorImage::from_rgba_unmultiplied(
                    [self.width as usize, self.height as usize],
                    &self.rgba,
                ),
                egui::TextureOptions::LINEAR,
            )
        })
    }
}

#[derive(Clone)]
enum CoverState {
    /// A `request` has fired but no terminal state yet.
    Loading,
    /// Decode succeeded (or recovered from cache).
    Ready(Arc<TitleImage>),
    /// Download/decode failed.
    Failed { at: Instant },
}

struct CoverEntry {
    state: CoverState,
    generation: u64,
}

#[derive(Default)]
struct CoverCache {
    entries: HashMap<String, CoverEntry>,
    next_generation: u64,
    ready_count: usize,
}

impl CoverCache {
    fn get(&self, app_id: &str) -> Option<&CoverState> {
        self.entries.get(app_id).map(|entry| &entry.state)
    }

    fn insert(&mut self, app_id: String, state: CoverState) {
        let is_ready = matches!(state, CoverState::Ready(_));
        self.next_generation += 1;
        let generation = self.next_generation;
        if let Some(previous) = self.entries.insert(
            app_id,
            CoverEntry {
                state,
                generation,
            },
        ) {
            if matches!(previous.state, CoverState::Ready(_)) {
                self.ready_count -= 1;
            }
        }
        if is_ready {
            self.ready_count += 1;
        }
    }

    fn touch(&mut self, app_id: &str) {
        self.next_generation += 1;
        let generation = self.next_generation;
        if let Some(entry) = self.entries.get_mut(app_id) {
            entry.generation = generation;
        }
    }

    fn forget(&mut self, app_id: &str) {
        if let Some(entry) = self.entries.remove(app_id)
            && matches!(entry.state, CoverState::Ready(_))
        {
            self.ready_count -= 1;
        }
    }

    /// Drops `Ready` covers until at most `max_ready` remain, oldest-touched first, never
    /// touching `keep`.
    fn evict_to(&mut self, keep: Option<&str>, max_ready: usize) {
        while self.ready_count > max_ready {
            let victim = self
                .entries
                .iter()
                .filter(|(id, entry)| {
                    keep != Some(id.as_str()) && matches!(entry.state, CoverState::Ready(_))
                })
                .min_by_key(|(_, entry)| entry.generation)
                .map(|(id, _)| id.clone());
            let Some(victim) = victim else { break };
            self.forget(&victim);
        }
    }
}

/// Shared, lazily-populated cache of cover art.
#[derive(Clone)]
pub struct CoverStore {
    inner: Arc<Mutex<CoverCache>>,
    /// Row thumbnails, cached separately from `inner` because they are decoded at a fraction of
    /// the size - see [`CoverSize`].
    icons: Arc<Mutex<CoverCache>>,
    download_permits: Arc<Semaphore>,
}

/// Which decode size a request wants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoverSize {
    /// Detail-panel cover art and the panel backdrop.
    Cover,
    /// List-row thumbnail.
    Icon,
}

impl CoverSize {
    /// Longest-edge cap applied at decode time.
    fn max_dimension(self) -> u32 {
        match self {
            Self::Cover => 256,
            Self::Icon => 64,
        }
    }

    /// How many decoded images of this size stay resident.
    fn cache_capacity(self) -> usize {
        match self {
            Self::Cover => MAX_CACHED_COVERS,
            Self::Icon => MAX_CACHED_ICONS,
        }
    }

    /// Per-size egui texture key, so a title's cover and its thumbnail never collide.
    fn texture_key(self, app_id: &str) -> String {
        match self {
            Self::Cover => format!("gfn_cover_{app_id}"),
            Self::Icon => format!("gfn_icon_{app_id}"),
        }
    }
}

impl Default for CoverStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CoverStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CoverCache::default())),
            icons: Arc::new(Mutex::new(CoverCache::default())),
            download_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_COVER_DOWNLOADS)),
        }
    }

    fn cache_for(&self, size: CoverSize) -> &Arc<Mutex<CoverCache>> {
        match size {
            CoverSize::Cover => &self.inner,
            CoverSize::Icon => &self.icons,
        }
    }

    /// Idempotent: no-op if already loading or ready, and a recent failure is left alone until
    /// `COVER_RETRY_AFTER` has elapsed.
    pub fn request(&self, http_client: &Client, ctx: &egui::Context, app_id: String, url: String) {
        self.request_sized(http_client, ctx, app_id, url, CoverSize::Cover);
    }

    /// Like [`Self::request`] but for the small list-row thumbnail.
    pub fn request_icon(
        &self,
        http_client: &Client,
        ctx: &egui::Context,
        app_id: String,
        url: String,
    ) {
        self.request_sized(http_client, ctx, app_id, url, CoverSize::Icon);
    }

    fn request_sized(
        &self,
        http_client: &Client,
        ctx: &egui::Context,
        app_id: String,
        url: String,
        size: CoverSize,
    ) {
        let cache = self.cache_for(size).clone();
        {
            let mut inner = match cache.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            match inner.get(&app_id) {
                Some(CoverState::Loading | CoverState::Ready(_)) => return,
                Some(CoverState::Failed { at }) if at.elapsed() < COVER_RETRY_AFTER => return,
                Some(CoverState::Failed { .. }) | None => {}
            }
            inner.insert(app_id.clone(), CoverState::Loading);
        }

        let permits = self.download_permits.clone();
        let http_client = http_client.clone();
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let _permit = match permits.acquire_owned().await {
                Ok(permit) => permit,
                Err(error) => {
                    eprintln!("Cover semaphore closed for {app_id}: {error}");
                    let mut inner = match cache.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    inner.insert(app_id, CoverState::Failed { at: Instant::now() });
                    return;
                }
            };

            let outcome =
                fetch_and_decode(&http_client, &app_id, &url, size.max_dimension()).await;
            let mut inner = match cache.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            match outcome {
                Ok(image) => {
                    let texture = Arc::new(image);
                    let _ = texture.texture(&ctx, || size.texture_key(&app_id));
                    inner.insert(app_id.clone(), CoverState::Ready(texture));
                    inner.evict_to(Some(&app_id), size.cache_capacity());
                }
                Err(error) => {
                    eprintln!("Cover fetch for {app_id} failed: {error:#}");
                    inner.insert(app_id, CoverState::Failed { at: Instant::now() });
                }
            }
        });
    }

    /// Drops decoded covers that are no longer worth keeping resident, retaining at most
    /// `max_ready` of the most-recently-used plus `keep` (the currently selected title, which
    /// must survive regardless of age).
    pub fn prune(&self, keep: Option<&str>, max_ready: usize) {
        Self::prune_cache(&self.inner, keep, max_ready);
        let icon_budget = if max_ready == 0 {
            0
        } else {
            CoverSize::Icon.cache_capacity()
        };
        Self::prune_cache(&self.icons, keep, icon_budget);
    }

    fn prune_cache(cache: &Mutex<CoverCache>, keep: Option<&str>, max_ready: usize) {
        let mut inner = match cache.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(keep) = keep {
            inner.touch(keep);
        }
        inner.evict_to(keep, max_ready);
    }

    /// Returns a snapshot of the current state for `app_id` if present.
    pub fn get(&self, app_id: &str) -> Option<CoverSnapshot> {
        self.get_sized(app_id, CoverSize::Cover)
    }

    /// Like [`Self::get`] for the small list-row thumbnail.
    pub fn get_icon(&self, app_id: &str) -> Option<CoverSnapshot> {
        self.get_sized(app_id, CoverSize::Icon)
    }

    fn get_sized(&self, app_id: &str, size: CoverSize) -> Option<CoverSnapshot> {
        let mut inner = self.cache_for(size).lock().ok()?;
        let snapshot = match inner.get(app_id)? {
            CoverState::Loading => CoverSnapshot::Loading,
            CoverState::Ready(image) => CoverSnapshot::Ready(image.clone()),
            CoverState::Failed { .. } => CoverSnapshot::Failed,
        };
        inner.touch(app_id);
        Some(snapshot)
    }

    pub fn is_requested(&self, app_id: &str, size: CoverSize) -> bool {
        let Ok(inner) = self.cache_for(size).lock() else {
            return false;
        };
        matches!(
            inner.get(app_id),
            Some(CoverState::Loading | CoverState::Ready(_))
        )
    }

    /// egui texture key for a cached image, so callers pass the right one to
    /// [`TitleImage::texture`].
    pub fn texture_key(app_id: &str, size: CoverSize) -> String {
        size.texture_key(app_id)
    }
}

pub enum CoverSnapshot {
    Loading,
    Ready(Arc<TitleImage>),
    Failed,
}

async fn fetch_and_decode(
    client: &Client,
    app_id: &str,
    url: &str,
    max_dim: u32,
) -> Result<TitleImage> {
    if let Some(path) = disk_cache_path(app_id) {
        let cached = tokio::task::spawn_blocking({
            let path = path.clone();
            move || std::fs::read(&path).ok()
        })
        .await
        .unwrap_or(None);

        if let Some(bytes) = cached {
            match tokio::task::spawn_blocking(move || decode_rgba(&bytes, max_dim)).await {
                Ok(Ok(image)) => return Ok(image),
                // A truncated or corrupt file should cost one re-download, not a permanent
                // failure, so fall through to the network and let the write below replace it.
                Ok(Err(error)) => eprintln!("Discarding unreadable cached cover {app_id}: {error}"),
                Err(error) => eprintln!("Cached cover decode task panicked: {error}"),
            }
        }
    }

    let bytes = client
        .get(url)
        .send()
        .await
        .context("cover request failed")?
        .error_for_status()
        .context("cover request returned an error status")?
        .bytes()
        .await
        .context("failed to read cover response body")?;

    // The encoded original is stored rather than the decoded RGBA: it is an order of magnitude
    // smaller on the memory card, and both decode sizes for a title share the one file, so a
    // thumbnail and its full cover cost a single download instead of two.
    if let Some(path) = disk_cache_path(app_id) {
        let bytes = bytes.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(error) = std::fs::create_dir_all(COVER_DISK_CACHE_DIR)
                .and_then(|()| std::fs::write(&path, &bytes))
            {
                eprintln!("Could not cache cover to {}: {error}", path.display());
            }
        });
    }

    tokio::task::spawn_blocking(move || decode_rgba(&bytes, max_dim))
        .await
        .context("cover decode task panicked")?
}

/// On-disk path for a title's cover bytes, or `None` if `app_id` has no filename-safe form.
fn disk_cache_path(app_id: &str) -> Option<std::path::PathBuf> {
    let safe: String = app_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if safe.chars().all(|c| c == '_') {
        return None;
    }
    Some(std::path::Path::new(COVER_DISK_CACHE_DIR).join(format!("{safe}.img")))
}

/// Decodes JPEG/PNG bytes to RGBA.
fn decode_rgba(bytes: &[u8], max_dim: u32) -> Result<TitleImage> {
    let image = image::load_from_memory(bytes).context("failed to decode cover image")?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let scale = (max_dim as f32 / width.max(height) as f32).min(1.0);
    let target_w = ((width as f32 * scale).round() as u32).max(1);
    let target_h = ((height as f32 * scale).round() as u32).max(1);
    if (target_w, target_h) == (width, height) {
        return Ok(TitleImage::new(rgba.into_raw(), width, height));
    }
    let resized = image::imageops::resize(
        &rgba,
        target_w,
        target_h,
        image::imageops::FilterType::Triangle,
    );
    Ok(TitleImage::new(resized.into_raw(), target_w, target_h))
}
