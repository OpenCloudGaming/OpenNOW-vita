//! Which of the Vita's three user CPU cores each thread runs on.
//!
//! Cores are named by the job they do rather than by number, so the whole policy lives in one
//! `match` below instead of being spread across call sites as raw `SCE_KERNEL_CPU_MASK_USER_*`
//! constants.

/// A core, chosen by what runs there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VitaCore {
    /// The shell loop: SDL events, egui, and presenting frames.
    ///
    /// Gets a core to itself because it paces the entire video pipeline - a texture only returns
    /// to the decoder once per iteration of this loop, so jitter here becomes dropped frames
    /// everywhere else. It was previously the only thread left unpinned, free to land on top of
    /// the network thread.
    Render,
    /// The WebRTC peer: SRTP/DTLS decrypt and RTP depacketisation on every inbound packet.
    ///
    /// Exclusive, because this is the heaviest *continuous* CPU load in the app and starving it
    /// pushes jitter downstream into the access-unit stream.
    Network,
    /// Video and audio decode.
    ///
    /// These share happily: `sceAvcdecDecode` is a hardware block that the thread mostly blocks
    /// inside, and Opus decode is a small fixed cost. Audio used to sit on `Network`, competing
    /// with continuous crypto for no reason.
    Media,
}

#[cfg(target_os = "vita")]
impl VitaCore {
    fn mask(self) -> u32 {
        match self {
            // Render / UI Loop: Primary Core 0, Backup Core 2 (prevents CPU 0 100% spike while keeping L1 cache local)
            Self::Render => {
                vitasdk_sys::SCE_KERNEL_CPU_MASK_USER_0 | vitasdk_sys::SCE_KERNEL_CPU_MASK_USER_2
            }
            // WebRTC Network: Primary Core 1, Backup Core 2
            Self::Network => {
                vitasdk_sys::SCE_KERNEL_CPU_MASK_USER_1 | vitasdk_sys::SCE_KERNEL_CPU_MASK_USER_2
            }
            // Video / Audio Decode: Primary Core 2, Backup Core 1
            Self::Media => {
                vitasdk_sys::SCE_KERNEL_CPU_MASK_USER_2 | vitasdk_sys::SCE_KERNEL_CPU_MASK_USER_1
            }
        }
    }
}

/// Pins the calling thread, logging (not failing) on error - a missed pin risks scheduler jitter,
/// not correctness.
///
/// Note that SDL's internal audio callback thread and sceGxm's display callback thread are created
/// inside C and cannot be pinned from here; they inherit the process default.
#[cfg(target_os = "vita")]
pub fn pin_current_thread(core: VitaCore, label: &str) {
    let mask = core.mask();
    let thread_id = unsafe { vitasdk_sys::sceKernelGetThreadId() };
    let result =
        unsafe { vitasdk_sys::sceKernelChangeThreadCpuAffinityMask(thread_id, mask as i32) };
    if result < 0 {
        eprintln!("Failed to pin {label} thread to {core:?} (mask {mask:#x}): {result:#x}");
    }
}

#[cfg(not(target_os = "vita"))]
pub fn pin_current_thread(_core: VitaCore, _label: &str) {}
