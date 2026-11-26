//! Name lookups through the Vita's own resolver instead of newlib's getaddrinfo, which asks once
//! with no retry through shared static buffers: parallel lookups (login, covers, region pings,
//! signaling) failed with "authorization request failed" / timeouts. Same fix Kasumi ships.
//! Each lookup gets its own resolver, a few tries, and recent answers are cached (a stale answer
//! beats none). IPv4 only, as the Vita is.

use std::collections::HashMap;
use std::ffi::CString;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const RESOLVE_TIMEOUT_US: i32 = 2_000_000;
const RESOLVE_RETRIES: i32 = 3;
const ATTEMPTS: usize = 2;
const CACHE_FRESH: Duration = Duration::from_secs(5 * 60);

fn cache() -> &'static Mutex<HashMap<String, (Ipv4Addr, Instant)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Ipv4Addr, Instant)>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn ask_system(host: &str) -> Option<Ipv4Addr> {
    let name = CString::new(host).ok()?;
    for _ in 0..ATTEMPTS {
        // SAFETY: plain FFI calls; `address` outlives the call and the resolver is destroyed once.
        unsafe {
            let rid = vitasdk_sys::sceNetResolverCreate(c"opennow".as_ptr(), std::ptr::null_mut(), 0);
            if rid < 0 {
                continue;
            }
            let mut address: vitasdk_sys::SceNetInAddr = std::mem::zeroed();
            let result = vitasdk_sys::sceNetResolverStartNtoa(
                rid,
                name.as_ptr(),
                &mut address,
                RESOLVE_TIMEOUT_US,
                RESOLVE_RETRIES,
                0,
            );
            vitasdk_sys::sceNetResolverDestroy(rid);
            if result >= 0 && address.s_addr != 0 {
                return Some(Ipv4Addr::from(u32::from_be(address.s_addr)));
            }
        }
    }
    None
}

/// Blocking lookup. Literal IPs pass straight through.
pub fn resolve_blocking(host: &str) -> Option<Ipv4Addr> {
    if let Ok(ip) = host.parse() {
        return Some(ip);
    }
    if let Some((ip, at)) = cache().lock().ok()?.get(host).copied() {
        if at.elapsed() < CACHE_FRESH {
            return Some(ip);
        }
    }
    match ask_system(host) {
        Some(ip) => {
            if let Ok(mut map) = cache().lock() {
                map.insert(host.to_owned(), (ip, Instant::now()));
            }
            Some(ip)
        }
        None => {
            let stale = cache().lock().ok()?.get(host).map(|(ip, _)| *ip);
            eprintln!("dns: {host}: lookup failed{}", if stale.is_some() { ", using the last answer" } else { "" });
            stale
        }
    }
}

pub async fn resolve(host: &str, port: u16) -> std::io::Result<SocketAddr> {
    let owned = host.to_owned();
    tokio::task::spawn_blocking(move || resolve_blocking(&owned))
        .await
        .ok()
        .flatten()
        .map(|ip| SocketAddr::from((ip, port)))
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("could not resolve {host}")))
}

/// Plugs the resolver above into reqwest.
pub struct VitaResolver;

impl reqwest::dns::Resolve for VitaResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addr = resolve(name.as_str(), 0).await?;
            Ok(Box::new(std::iter::once(addr)) as reqwest::dns::Addrs)
        })
    }
}
