//! Cross-platform system monitoring for the API server.

mod gpu;
#[cfg(target_os = "macos")]
mod macos_smc;
mod system_monitor;

pub use system_monitor::{
    CpuCoreInfo, CpuInfo, DiskInfo, HealthResponse, MemoryInfo, NetworkInfo, SwapInfo, SystemInfo,
    SystemMonitor, SystemSnapshot, UptimeInfo, control_shutdown, get_system_snapshot, health,
};

pub use gpu::GpuInfo;
