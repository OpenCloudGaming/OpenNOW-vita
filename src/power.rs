//! CPU/GPU clock configuration.
//!
//! The Vita boots homebrew at conservative clocks. Nothing in this app raised them, so the shell
//! loop - egui tessellation, the SDL painter, SRTP/DTLS decrypt - ran slower than the hardware is
//! capable of, and that loop is what paces the whole video pipeline.
//!
//! Everything here is a no-op off-Vita, so the host typecheck build sees these as unused.
#![cfg_attr(not(target_os = "vita"), allow(dead_code))]

/// The clocks a session runs at, in MHz. Matches the profile RetroArch's Vita frontend ships
/// (`frontend/drivers/platform_psp.c`), which is the platform's established maximum-performance
/// setting and what most Vita users already run for hours at a time.
const PERFORMANCE_ARM_MHZ: i32 = 444;
const PERFORMANCE_BUS_MHZ: i32 = 222;
const PERFORMANCE_GPU_MHZ: i32 = 222;
const PERFORMANCE_GPU_XBAR_MHZ: i32 = 166;

/// Clock speeds captured before this app changed them.
#[derive(Debug, Clone, Copy)]
struct Clocks {
    arm: i32,
    bus: i32,
    gpu: i32,
    gpu_xbar: i32,
}

/// Raises the console to its performance clocks for as long as this value is alive.
///
/// Note this is best-effort by design: a clock that refuses to change costs frames, not
/// correctness, so every failure is logged and stepped over rather than propagated.
pub struct PerformanceMode {
    restore: Option<Clocks>,
}

impl PerformanceMode {
    #[cfg(target_os = "vita")]
    pub fn engage() -> Self {
        let restore = Some(Clocks {
            arm: unsafe { vitasdk_sys::scePowerGetArmClockFrequency() },
            bus: unsafe { vitasdk_sys::scePowerGetBusClockFrequency() },
            gpu: unsafe { vitasdk_sys::scePowerGetGpuClockFrequency() },
            gpu_xbar: unsafe { vitasdk_sys::scePowerGetGpuXbarClockFrequency() },
        });
        eprintln!("Clocks before: {restore:?}");

        set_clocks(Clocks {
            arm: PERFORMANCE_ARM_MHZ,
            bus: PERFORMANCE_BUS_MHZ,
            gpu: PERFORMANCE_GPU_MHZ,
            gpu_xbar: PERFORMANCE_GPU_XBAR_MHZ,
        });

        let result = unsafe { vitasdk_sys::scePowerSetUsingWireless(1) };
        if result < 0 {
            eprintln!("scePowerSetUsingWireless failed: {result:#x}");
        }

        Self { restore }
    }

    #[cfg(not(target_os = "vita"))]
    pub fn engage() -> Self {
        Self { restore: None }
    }
}

impl Drop for PerformanceMode {
    fn drop(&mut self) {
        // In practice this rarely runs: `shell::run` loops until the user quits via the PS button,
        // which tears the process down without unwinding. Kept because it is correct if a clean
        // exit path is ever added - it is not what protects the console.
        #[cfg(target_os = "vita")]
        if let Some(restore) = self.restore {
            set_clocks(restore);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryStatus {
    pub percent: u8,
    pub charging: bool,
    pub low: bool,
}

impl BatteryStatus {
    pub const WARN_PERCENT: u8 = 20;
    pub const CRITICAL_PERCENT: u8 = 8;

    pub fn should_warn(self) -> bool {
        !self.charging && (self.low || self.percent <= Self::WARN_PERCENT)
    }

    pub fn is_critical(self) -> bool {
        !self.charging && self.percent <= Self::CRITICAL_PERCENT
    }
}

#[cfg(target_os = "vita")]
pub fn battery_status() -> Option<BatteryStatus> {
    let percent = unsafe { vitasdk_sys::scePowerGetBatteryLifePercent() };
    if percent < 0 {
        return None;
    }
    Some(BatteryStatus {
        percent: percent.clamp(0, 100) as u8,
        charging: unsafe { vitasdk_sys::scePowerIsBatteryCharging() } > 0
            || unsafe { vitasdk_sys::scePowerIsPowerOnline() } > 0,
        low: unsafe { vitasdk_sys::scePowerIsLowBattery() } > 0,
    })
}

#[cfg(not(target_os = "vita"))]
pub fn battery_status() -> Option<BatteryStatus> {
    None
}

#[cfg(target_os = "vita")]
pub fn suspend_required() -> bool {
    unsafe { vitasdk_sys::scePowerIsSuspendRequired() > 0 }
}

#[cfg(not(target_os = "vita"))]
pub fn suspend_required() -> bool {
    false
}

/// Restoring reads the previous values back rather than hardcoding a default, so launching from a
/// shell that already changed clocks puts them back where they actually were.
#[cfg(target_os = "vita")]
fn set_clocks(clocks: Clocks) {
    let apply = |name: &str, mhz: i32, setter: unsafe extern "C" fn(i32) -> i32| {
        let result = unsafe { setter(mhz) };
        if result < 0 {
            eprintln!("{name}({mhz}) failed: {result:#x}");
        }
    };
    apply(
        "scePowerSetArmClockFrequency",
        clocks.arm,
        vitasdk_sys::scePowerSetArmClockFrequency,
    );
    apply(
        "scePowerSetBusClockFrequency",
        clocks.bus,
        vitasdk_sys::scePowerSetBusClockFrequency,
    );
    apply(
        "scePowerSetGpuClockFrequency",
        clocks.gpu,
        vitasdk_sys::scePowerSetGpuClockFrequency,
    );
    apply(
        "scePowerSetGpuXbarClockFrequency",
        clocks.gpu_xbar,
        vitasdk_sys::scePowerSetGpuXbarClockFrequency,
    );
}
