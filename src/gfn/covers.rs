//! Cover-art cache + background download.
//!
//! Downloaded bytes -> decoded RGBA -> cached `TitleImage` with a lazily-initialized
//! `egui::TextureHandle` (decoupled from any one egui context so it can be reused across
//! `build_ui` calls).
//!
//! No dedicated worker thread and no job channels: jade-vita's UI loop IS the single-threaded
//! tokio runtime, so spawned cover-fetch tasks naturally advance on the same runtime as the
//! rest of the app.
//!
//! Lazy loading: covers are only requested when a tile becomes visible in the grid (see
//! `app::ui::catalog_screen`). A bounded semaphore caps concurrent downloads so the image CDN
//! isn't hammered if the user flicks through the catalog quickly.

use anyhow::{Context, Result};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// Maximum simultaneous cover downloads. Conservative: the Vita has limited memory and a
/// single Wi-Fi radio - one in-flight HTTP download per visible tile (typically 12-20 in a
/// grid view) would still be fine cap-wise, but parallelizing at ~8 is plenty of
/// throughput for an image CDN and keeps TLS/HTTP state machine memory bounded.
const MAX_CONCURRENT_COVER_DOWNLOADS: usize = 8;

/// How many decoded covers stay resident. One entry costs a 256x256 RGBA `Vec<u8>` (256 KiB,
/// see `MAX_COVER_DIM`) plus the power-of-two SDL texture the painter uploads it into (another
/// 256 KiB of VRAM), so ~512 KiB each and ~4 MiB at this cap.
///
/// The catalog UI only ever requests the *selected* row's cover, so this cap exists purely to
/// keep back-and-forth d-pad navigation instant without letting a long browse session
/// accumulate every cover the user ever passed over - which is unbounded growth on a console
/// that also needs its CDRAM for the video decoder. Raising this trades stream-start headroom
/// for scroll-back smoothness; 8 is deliberately conservative.
pub const MAX_CACHED_COVERS: usize = 8;

/// How many decoded row thumbnails stay resident. Each is a 64x64 RGBA buffer (16 KiB) plus a
/// 64x64 texture, so this is ~32 KiB apiece.
///
/// Deliberately conservative at 24 (~768 KiB total). The list shows ~12 rows at once on the
/// Vita's 418pt-tall screen, so this holds two full screenfuls: eviction can only ever reach
/// rows that are already off-screen, and scrolling back a page stays instant.
///
/// The floor that matters is "comfortably more than one screenful" - dropping below ~12 would
/// evict thumbnails that are still visible and thrash the decoder on every scroll.
pub const MAX_CACHED_ICONS: usize = 24;

/// How long a failed cover download is remembered before another attempt is allowed.
///
/// Without this the UI melts down: the detail panel calls [`CoverStore::request`] every frame
/// for the selected game, and a `Failed` entry is retryable, so a cover URL that 404s would
/// re-download at 60 Hz for as long as its row stays highlighted. Long enough to stop the
/// storm, short enough that a transient Wi-Fi drop still recovers on its own.
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

    /// Lazily uploads the RGBA to the egui context. Safe to call every frame - the first
    /// caller wins, subsequent calls return the cached handle. `key` is a per-app stable id so
    /// egui dedupes uploads across visits to the same catalog item.
    pub fn texture(&self, ctx: &egui::Context, key: &str) -> &egui::TextureHandle {
        self.texture.get_or_init(|| {
            ctx.load_texture(
                key.to_owned(),
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
    /// Download/decode failed. Retryable, but only once `COVER_RETRY_AFTER` has elapsed since
    /// `at` - see that constant for why an immediately-retryable failure is a footgun here.
    Failed { at: Instant },
}

/// The map plus its LRU ordering, kept together so both are mutated under the one `Mutex`.
#[derive(Default)]
struct CoverCache {
    entries: HashMap<String, CoverState>,
    /// App ids ordered oldest-touched first. Only `Ready` entries are eviction candidates, but
    /// every id is tracked so promotion on `get` is a single operation.
    lru: Vec<String>,
}

impl CoverCache {
    /// Moves `app_id` to the most-recently-used end.
    fn touch(&mut self, app_id: &str) {
        if let Some(index) = self.lru.iter().position(|id| id == app_id) {
            let id = self.lru.remove(index);
            self.lru.push(id);
        } else {
            self.lru.push(app_id.to_owned());
        }
    }

    fn forget(&mut self, app_id: &str) {
        self.entries.remove(app_id);
        self.lru.retain(|id| id != app_id);
    }

    /// Drops `Ready` covers until at most `max_ready` remain, oldest-touched first, never
    /// touching `keep`.
    ///
    /// Only `Ready` entries are considered: evicting a `Loading` entry would let `request`
    /// start a second download for the same id, and `Failed` entries are a few bytes each and
    /// are what suppresses the retry storm.
    fn evict_to(&mut self, keep: Option<&str>, max_ready: usize) {
        let ready_count = |cache: &Self| {
            cache
                .entries
                .values()
                .filter(|state| matches!(state, CoverState::Ready(_)))
                .count()
        };

        while ready_count(self) > max_ready {
            let victim = self.lru.iter().find(|id| {
                if keep == Some(id.as_str()) {
                    return false;
                }
                matches!(self.entries.get(*id), Some(CoverState::Ready(_)))
            });
            let Some(victim) = victim.cloned() else {
                // Nothing left that we're allowed to drop.
                break;
            };
            // Dropping the value drops the `Arc<TitleImage>` and with it the cached
            // `TextureHandle`, which is what makes egui report the texture in
            // `textures_delta.free` so the SDL painter can release the VRAM.
            self.forget(&victim);
        }
    }
}

/// Shared, lazily-populated cache of cover art. Lives inside `AppState::Catalog`, cloned as an
/// `Arc` into the async download tasks that fill it.
///
/// `Arc<Mutex<HashMap>>` chosen over `DashMap`/`RwLock<HashMap>` for two reasons:
/// - Reads happen every frame (one cell per visible tile), writes are rare (one per finished
///   download). A plain Mutex is still cheap under contention this low and keeps the code small.
/// - The cloneable `Arc` lets a download task drop a value into the map after the originating
///   state has been replaced (e.g. user pressed Back - we'd discard the maps but inflight tasks
///   just write into an orphaned cache that gets GC'd when their last Arc drops). No
///   channel/plumbing through `App` needed.
#[derive(Clone)]
pub struct CoverStore {
    inner: Arc<Mutex<CoverCache>>,
    /// Row thumbnails, cached separately from `inner` because they are decoded at a fraction of
    /// the size - see [`CoverSize`].
    icons: Arc<Mutex<CoverCache>>,
    download_permits: Arc<Semaphore>,
}

/// Which decode size a request wants. The two live in separate caches with separate budgets:
/// the detail panel shows one big cover at a time, while the list shows a screenful of tiny ones.
///
/// Sizing them apart is what makes per-row thumbnails affordable at all. Reusing the 256px cover
/// for a 22pt row would cost ~256 KB of VRAM per visible row (the painter rounds textures up to a
/// power of two regardless of the size drawn) - roughly 3.5 MB for one screenful, growing as the
/// user scrolls. At 64px a row thumbnail is 16 KB.
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
    ///
    /// Icons get a far larger count because they are ~16x cheaper each: 48 icons is about
    /// 786 KB of RGBA plus the same again in VRAM, and covers a screenful plus a comfortable
    /// scroll-back window without any explicit cursor tracking.
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
    /// `COVER_RETRY_AFTER` has elapsed. On a fresh (or retryable) entry we record `Loading` and
    /// spawn the download task. The spawned task lives independent of the caller - its outcome
    /// is written into the shared map on completion.
    ///
    /// Safe to call every frame, which the detail panel does for the selected game.
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
        // Scoped so the guard is released before `cache` is moved into the spawned task.
        {
            let mut inner = match cache.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            match inner.entries.get(&app_id) {
                Some(CoverState::Loading | CoverState::Ready(_)) => return,
                // Back off rather than hammering a URL that just failed - see `COVER_RETRY_AFTER`.
                Some(CoverState::Failed { at }) if at.elapsed() < COVER_RETRY_AFTER => return,
                Some(CoverState::Failed { .. }) | None => {}
            }
            inner.entries.insert(app_id.clone(), CoverState::Loading);
            inner.touch(&app_id);
        }

        let permits = self.download_permits.clone();
        let http_client = http_client.clone();
        let ctx = ctx.clone();
        tokio::spawn(async move {
            // Bound concurrent downloads so a flurried scroll over many new tiles doesn't fan
            // out ~50 parallel HTTPS requests against NVIDIA's image CDN. Released on drop at
            // the end of the task.
            let _permit = match permits.acquire_owned().await {
                Ok(permit) => permit,
                Err(error) => {
                    eprintln!("Cover semaphore closed for {app_id}: {error}");
                    let mut inner = match cache.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    inner
                        .entries
                        .insert(app_id, CoverState::Failed { at: Instant::now() });
                    return;
                }
            };

            let outcome = fetch_and_decode(&http_client, &url, size.max_dimension()).await;
            let texture_key = size.texture_key(&app_id);
            let mut inner = match cache.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            match outcome {
                Ok(image) => {
                    let texture = Arc::new(image);
                    // Pre-create the egui texture from the async context so the UI thread can
                    // just blit it next frame without paying decode/upload cost. egui's context
                    // is Send+Sync and `load_texture` uses its internal lock.
                    let _ = texture.texture(&ctx, &texture_key);
                    inner
                        .entries
                        .insert(app_id.clone(), CoverState::Ready(texture));
                    inner.touch(&app_id);
                    // Make room only after inserting, so the image we just decoded is the
                    // most-recently-used entry and can never be its own eviction victim.
                    inner.evict_to(Some(&app_id), size.cache_capacity());
                }
                Err(error) => {
                    eprintln!("Cover fetch for {app_id} failed: {error:#}");
                    inner
                        .entries
                        .insert(app_id, CoverState::Failed { at: Instant::now() });
                }
            }
        });
    }

    /// Drops decoded covers that are no longer worth keeping resident, retaining at most
    /// `max_ready` of the most-recently-used plus `keep` (the currently selected title, which
    /// must survive regardless of age).
    ///
    /// Called every tick from `App::tick`. Passing `max_ready = 0` releases everything except
    /// `keep` - used when leaving the catalog for a streaming session, so the RGBA buffers and
    /// their VRAM textures are gone before the video decoder claims its CDRAM.
    /// Prunes **both** caches. `max_ready` applies to the big covers; icons are trimmed to their
    /// own capacity, or dropped entirely when `max_ready` is 0 (i.e. we are leaving the catalog).
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

    /// Returns a snapshot of the current state for `app_id` if present. The returned `Arc`
    /// can be held cheaply across the duration of one frame by the UI without locking the
    /// map again.
    ///
    /// Also promotes the entry in the LRU, so anything the UI is actively drawing is by
    /// definition the newest and safe from eviction.
    pub fn get(&self, app_id: &str) -> Option<CoverSnapshot> {
        self.get_sized(app_id, CoverSize::Cover)
    }

    /// Like [`Self::get`] for the small list-row thumbnail.
    pub fn get_icon(&self, app_id: &str) -> Option<CoverSnapshot> {
        self.get_sized(app_id, CoverSize::Icon)
    }

    fn get_sized(&self, app_id: &str, size: CoverSize) -> Option<CoverSnapshot> {
        let mut inner = self.cache_for(size).lock().ok()?;
        let snapshot = match inner.entries.get(app_id)? {
            CoverState::Loading => CoverSnapshot::Loading,
            CoverState::Ready(image) => CoverSnapshot::Ready(image.clone()),
            CoverState::Failed { .. } => CoverSnapshot::Failed,
        };
        inner.touch(app_id);
        Some(snapshot)
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

async fn fetch_and_decode(client: &Client, url: &str, max_dim: u32) -> Result<TitleImage> {
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
    // `covers::request`'s task runs on the same single-threaded tokio runtime as the render
    // loop (see module docs above) - decoding a JPEG and resizing it synchronously here would
    // stall that runtime, and with it every frame's UI/input polling, for the duration of the
    // decode. `spawn_blocking` moves that CPU-bound work onto tokio's separate blocking-thread
    // pool so the render loop keeps ticking while covers decode in the background.
    tokio::task::spawn_blocking(move || decode_rgba(&bytes, max_dim))
        .await
        .context("cover decode task panicked")?
}

/// Decodes JPEG/PNG bytes to RGBA. Resizes if larger than `MAX_COVER_DIM` along its largest
/// axis. No "pad to bounds" step: covers render at their natural aspect ratio inside a fixed
/// slot, so uniform padding is unnecessary.
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
