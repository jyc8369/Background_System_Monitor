//! Cross-platform system monitoring for the API server.

mod system_monitor;

pub use system_monitor::{
    SystemMonitor, SystemSnapshot, control_shutdown, get_system_snapshot, health,
};
