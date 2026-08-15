use vita_newlib_shims as _;

mod i18n;
mod locale;
mod power;
mod app;
mod gfn;
mod input;
mod jobs;
mod logger;
mod safe_memory;
mod shell;
mod streaming;
mod thread_affinity;

use app::App;

#[used]
#[unsafe(export_name = "sceUserMainThreadStackSize")]
pub static SCE_USER_MAIN_THREAD_STACK_SIZE: u32 = 4 * 1024 * 1024;

#[used]
#[unsafe(export_name = "sceLibcHeapSize")]
pub static SCE_LIBC_HEAP_SIZE: u32 = 40 * 1024 * 1024;

#[used]
#[unsafe(export_name = "_newlib_heap_size_user")]
pub static NEWLIB_HEAP_SIZE_USER: u32 = 192 * 1024 * 1024;

fn main() -> anyhow::Result<()> {
    if let Err(e) = logger::init() {
        eprintln!("Failed to initialize logger: {e}");
    }
    log_info!("OpenNOW-vita starting up");

    let _performance = power::PerformanceMode::engage();
    thread_affinity::pin_current_thread(thread_affinity::VitaCore::Render, "shell");
    let _app_util = safe_memory::AppUtil::initialize()?;
    #[cfg(target_os = "vita")]
    streaming::video::reserve_decoder_cdram();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let app = App::new()?;
        shell::run(app).await
    })
}
