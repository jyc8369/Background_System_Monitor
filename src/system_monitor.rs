use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::{
    sync::RwLock,
    time::{MissedTickBehavior, interval},
};

const COLLECTION_INTERVAL: Duration = Duration::from_secs(1);
const MAX_SNAPSHOT_AGE: Duration = Duration::from_secs(3);

/// A point-in-time view of the metrics exposed by `/api/system`.
#[derive(Clone, Debug, Serialize)]
pub struct SystemSnapshot {
    pub timestamp: u64,
    pub system: SystemInfo,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub uptime: UptimeInfo,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemInfo {
    pub hostname: String,
    pub os: String,
    pub os_version: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CpuInfo {
    pub usage_percent: f32,
    pub logical_cores: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct UptimeInfo {
    pub seconds: u64,
}

/// Stable error response returned when the collector cannot provide a valid
/// snapshot.
#[derive(Clone, Debug, Serialize)]
pub struct SystemErrorResponse {
    pub error: &'static str,
    pub message: &'static str,
    pub last_updated_at: Option<u64>,
}

/// Shared monitor state held by Axum and updated by one background collector.
pub struct SystemMonitor {
    state: Arc<RwLock<MonitorState>>,
}

impl SystemMonitor {
    /// Creates the initial snapshot and starts the one-second collector task.
    ///
    /// The returned monitor is cheap to clone through `Arc` and is intended to
    /// be installed as Axum router state.
    pub fn start() -> Arc<Self> {
        let mut collector = Collector::new();
        let state = Arc::new(RwLock::new(MonitorState::from_result(collector.collect())));
        let monitor = Arc::new(Self {
            state: Arc::clone(&state),
        });

        tokio::spawn(async move {
            collector.run(state).await;
        });

        monitor
    }

    /// Returns a clone so the read lock is released before serialization.
    async fn snapshot(&self) -> Result<SystemSnapshot, Option<u64>> {
        let state = self.state.read().await;

        if state.is_healthy() {
            Ok(state
                .snapshot
                .clone()
                .expect("healthy monitor state must contain a snapshot"))
        } else {
            Err(state.last_updated_at())
        }
    }

    async fn is_healthy(&self) -> bool {
        self.state.read().await.is_healthy()
    }
}

/// Axum handler for `GET /api/system`.
pub async fn get_system_snapshot(
    State(monitor): State<Arc<SystemMonitor>>,
) -> Result<Json<SystemSnapshot>, (StatusCode, Json<SystemErrorResponse>)> {
    match monitor.snapshot().await {
        Ok(snapshot) => Ok(Json(snapshot)),
        Err(last_updated_at) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SystemErrorResponse {
                error: "system_metrics_unavailable",
                message: "System metrics are temporarily unavailable",
                last_updated_at,
            }),
        )),
    }
}

/// Axum handler for `GET /health`.
pub async fn health(State(monitor): State<Arc<SystemMonitor>>) -> (StatusCode, &'static str) {
    if monitor.is_healthy().await {
        (StatusCode::OK, "OK")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "UNAVAILABLE")
    }
}

struct Collector {
    system: System,
    system_info: SystemInfo,
    logical_cores: usize,
}

impl Collector {
    fn new() -> Self {
        // Only CPU and memory are initialized here. In particular, this does
        // not enumerate every process on startup.
        let system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );

        let logical_cores = system.cpus().len();

        Self {
            system,
            system_info: SystemInfo {
                hostname: System::host_name().unwrap_or_else(|| "unknown".to_owned()),
                os: System::name().unwrap_or_else(platform_name),
                os_version: System::long_os_version()
                    .or_else(System::os_version)
                    .unwrap_or_else(|| "unknown".to_owned()),
            },
            logical_cores,
        }
    }

    async fn run(mut self, state: Arc<RwLock<MonitorState>>) {
        let mut ticker = interval(COLLECTION_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        // `interval` ticks immediately once. Consume that tick so the first
        // published refresh is made after the first one-second sample window.
        ticker.tick().await;

        loop {
            ticker.tick().await;
            let result = self.collect();
            let mut monitor_state = state.write().await;
            let was_healthy = monitor_state.is_healthy();

            match result {
                Ok(next_snapshot) => monitor_state.set_success(next_snapshot),
                Err(error) => {
                    monitor_state.set_failure();
                    if was_healthy {
                        tracing::warn!(?error, "system metrics collection failed");
                    }
                }
            }
        }
    }

    fn collect(&mut self) -> Result<SystemSnapshot, CollectionError> {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();

        if self.logical_cores == 0 {
            return Err(CollectionError::Cpu);
        }

        let usage_percent = self.system.global_cpu_usage();
        if !usage_percent.is_finite() {
            return Err(CollectionError::Cpu);
        }

        let total_memory = self.system.total_memory();
        if total_memory == 0 {
            return Err(CollectionError::Memory);
        }

        let used_memory = self.system.used_memory();

        Ok(SystemSnapshot {
            timestamp: unix_timestamp(),
            system: self.system_info.clone(),
            cpu: CpuInfo {
                usage_percent,
                logical_cores: self.logical_cores,
            },
            memory: MemoryInfo {
                total_bytes: total_memory,
                used_bytes: used_memory,
                available_bytes: self.system.available_memory(),
                usage_percent: percentage(used_memory, total_memory),
            },
            uptime: UptimeInfo {
                seconds: System::uptime(),
            },
        })
    }
}

#[derive(Debug)]
enum CollectionError {
    Cpu,
    Memory,
}

struct MonitorState {
    snapshot: Option<SystemSnapshot>,
    last_success_at: Option<Instant>,
    collection_failed: bool,
}

impl MonitorState {
    fn from_result(result: Result<SystemSnapshot, CollectionError>) -> Self {
        let mut state = Self {
            snapshot: None,
            last_success_at: None,
            collection_failed: true,
        };

        if let Ok(snapshot) = result {
            state.set_success(snapshot);
        }

        state
    }

    fn set_success(&mut self, snapshot: SystemSnapshot) {
        self.snapshot = Some(snapshot);
        self.last_success_at = Some(Instant::now());
        self.collection_failed = false;
    }

    fn set_failure(&mut self) {
        self.collection_failed = true;
    }

    fn is_healthy(&self) -> bool {
        !self.collection_failed
            && self.snapshot.is_some()
            && self
                .last_success_at
                .is_some_and(|last_success| last_success.elapsed() <= MAX_SNAPSHOT_AGE)
    }

    fn last_updated_at(&self) -> Option<u64> {
        self.snapshot.as_ref().map(|snapshot| snapshot.timestamp)
    }
}

fn percentage(value: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (value as f64 / total as f64) * 100.0
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn platform_name() -> String {
    match std::env::consts::OS {
        "macos" => "macOS".to_owned(),
        "windows" => "Windows".to_owned(),
        "linux" => "Linux".to_owned(),
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Collector, percentage};

    #[test]
    fn percentage_returns_zero_for_unknown_total() {
        assert_eq!(percentage(100, 0), 0.0);
    }

    #[test]
    fn percentage_is_calculated_as_a_ratio() {
        assert_eq!(percentage(25, 100), 25.0);
    }

    #[test]
    fn initial_snapshot_is_serializable_and_has_platform_info() {
        let mut collector = Collector::new();
        let snapshot = collector.collect().expect("metrics should be available");
        let json = serde_json::to_value(snapshot).expect("snapshot should serialize");

        assert!(json.get("timestamp").is_some());
        assert!(json["system"]["os"].as_str().is_some());
        assert!(json["cpu"]["logical_cores"].is_u64());
        assert!(json.get("process").is_none());
    }

    #[test]
    fn failed_collection_is_not_healthy() {
        let state = super::MonitorState::from_result(Err(super::CollectionError::Cpu));

        assert!(!state.is_healthy());
        assert_eq!(state.last_updated_at(), None);
    }
}
