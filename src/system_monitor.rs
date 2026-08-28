use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use sysinfo::{
    Components, CpuRefreshKind, DiskRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind,
    System,
};
use tokio::{
    sync::{Notify, RwLock},
    time::{MissedTickBehavior, interval},
};

use crate::gpu::{self, GpuInfo};

const COLLECTION_INTERVAL: Duration = Duration::from_secs(1);
const CPU_FREQUENCY_REFRESH_INTERVAL: Duration = Duration::from_secs(10);
const DISK_IO_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const DISK_CAPACITY_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const NETWORK_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const GPU_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const TEMPERATURE_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const MAX_SNAPSHOT_AGE: Duration = Duration::from_secs(3);
const PROGRAM_NAME: &str = "Background_System_Monitor";

/// A point-in-time view of the metrics exposed by `/api/system`.
#[derive(Clone, Debug, Serialize)]
pub struct SystemSnapshot {
    pub timestamp: u64,
    pub system: SystemInfo,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub swap: SwapInfo,
    pub uptime: UptimeInfo,
    pub disks: Vec<DiskInfo>,
    pub networks: Vec<NetworkInfo>,
    pub gpus: Vec<GpuInfo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemInfo {
    pub hostname: String,
    pub os: String,
    pub os_version: String,
    pub kernel_version: String,
    pub architecture: String,
    pub cpu_model: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CpuInfo {
    pub usage_percent: f32,
    pub logical_cores: usize,
    pub physical_cores: Option<usize>,
    pub package_temperature_celsius: Option<f32>,
    pub per_core: Vec<CpuCoreInfo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CpuCoreInfo {
    pub index: usize,
    pub usage_percent: f32,
    pub frequency_mhz: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SwapInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct UptimeInfo {
    pub seconds: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f64,
    pub read_bytes_per_sec: f64,
    pub write_bytes_per_sec: f64,
    pub read_only: bool,
    pub removable: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct NetworkInfo {
    pub name: String,
    pub received_bytes_per_sec: f64,
    pub transmitted_bytes_per_sec: f64,
    pub total_received_bytes: u64,
    pub total_transmitted_bytes: u64,
}

/// Stable error response returned when the collector cannot provide a valid
/// snapshot.
#[derive(Clone, Debug, Serialize)]
pub struct SystemErrorResponse {
    pub error: &'static str,
    pub message: &'static str,
    pub last_updated_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HealthResponse {
    pub name: &'static str,
    pub status: &'static str,
}

/// Shared monitor state held by Axum and updated by one background collector.
pub struct SystemMonitor {
    state: Arc<RwLock<MonitorState>>,
    shutdown: Arc<Notify>,
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
            shutdown: Arc::new(Notify::new()),
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

    /// Requests a graceful server shutdown.
    pub fn request_shutdown(&self) {
        self.shutdown.notify_one();
    }

    /// Waits until the shutdown endpoint requests graceful termination.
    pub async fn wait_for_shutdown(&self) {
        self.shutdown.notified().await;
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
pub async fn health(
    State(monitor): State<Arc<SystemMonitor>>,
) -> (StatusCode, Json<HealthResponse>) {
    let (status_code, status) = if monitor.is_healthy().await {
        (StatusCode::OK, "OK")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "UNAVAILABLE")
    };

    (
        status_code,
        Json(HealthResponse {
            name: PROGRAM_NAME,
            status,
        }),
    )
}

/// Axum handler for `POST /control/shutdown`.
pub async fn control_shutdown(
    State(monitor): State<Arc<SystemMonitor>>,
) -> (StatusCode, &'static str) {
    monitor.request_shutdown();
    (StatusCode::OK, "OK")
}

struct Collector {
    system: System,
    components: Components,
    disks: Disks,
    networks: Networks,
    system_info: SystemInfo,
    logical_cores: usize,
    physical_cores: Option<usize>,
    disk_metrics: Vec<DiskInfo>,
    network_metrics: Vec<NetworkInfo>,
    gpu_metrics: Vec<GpuInfo>,
    package_temperature_celsius: Option<f32>,
    last_cpu_frequency_refresh: Instant,
    last_disk_io_refresh: Instant,
    last_disk_capacity_refresh: Instant,
    last_network_refresh: Instant,
    last_gpu_refresh: Instant,
    last_temperature_refresh: Instant,
}

impl Collector {
    fn new() -> Self {
        // The System object only initializes CPU and memory. In particular,
        // this does not enumerate any processes.
        let system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );

        let components = Components::new_with_refreshed_list();
        let disks = Disks::new_with_refreshed_list_specifics(DiskRefreshKind::everything());
        let networks = Networks::new_with_refreshed_list();
        let now = Instant::now();
        let logical_cores = system.cpus().len();
        let physical_cores = System::physical_core_count();
        let disk_metrics = disk_metrics(&disks, Duration::ZERO);
        let network_metrics = network_metrics(&networks, Duration::ZERO);
        let gpu_metrics = gpu::collect();
        let package_temperature_celsius = cpu_package_temperature(&components);
        let cpu_model = system
            .cpus()
            .first()
            .map(|cpu| cpu.brand().trim().to_owned())
            .filter(|brand| !brand.is_empty())
            .unwrap_or_else(|| "unknown".to_owned());

        Self {
            system,
            components,
            disks,
            networks,
            system_info: SystemInfo {
                hostname: System::host_name().unwrap_or_else(|| "unknown".to_owned()),
                os: System::name().unwrap_or_else(platform_name),
                os_version: System::long_os_version()
                    .or_else(System::os_version)
                    .unwrap_or_else(|| "unknown".to_owned()),
                kernel_version: System::kernel_version().unwrap_or_else(|| "unknown".to_owned()),
                architecture: std::env::consts::ARCH.to_owned(),
                cpu_model,
            },
            logical_cores,
            physical_cores,
            disk_metrics,
            network_metrics,
            gpu_metrics,
            package_temperature_celsius,
            last_cpu_frequency_refresh: now,
            last_disk_io_refresh: now,
            last_disk_capacity_refresh: now,
            last_network_refresh: now,
            last_gpu_refresh: now,
            last_temperature_refresh: now,
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

    fn refresh_supplemental_metrics(&mut self, now: Instant) {
        if now.duration_since(self.last_disk_capacity_refresh) >= DISK_CAPACITY_REFRESH_INTERVAL {
            // This also refreshes disk I/O and discovers mounted disks that
            // appeared or disappeared since the previous list refresh.
            self.disks.refresh(true);
            let elapsed = now.duration_since(self.last_disk_io_refresh);
            self.disk_metrics = disk_metrics(&self.disks, elapsed);
            self.last_disk_io_refresh = now;
            self.last_disk_capacity_refresh = now;
        } else if now.duration_since(self.last_disk_io_refresh) >= DISK_IO_REFRESH_INTERVAL {
            self.disks
                .refresh_specifics(false, DiskRefreshKind::nothing().with_io_usage());
            let elapsed = now.duration_since(self.last_disk_io_refresh);
            self.disk_metrics = disk_metrics(&self.disks, elapsed);
            self.last_disk_io_refresh = now;
        }

        if now.duration_since(self.last_network_refresh) >= NETWORK_REFRESH_INTERVAL {
            self.networks.refresh(true);
            let elapsed = now.duration_since(self.last_network_refresh);
            self.network_metrics = network_metrics(&self.networks, elapsed);
            self.last_network_refresh = now;
        }

        if now.duration_since(self.last_gpu_refresh) >= GPU_REFRESH_INTERVAL {
            self.gpu_metrics = gpu::collect();
            self.last_gpu_refresh = now;
        }

        if now.duration_since(self.last_temperature_refresh) >= TEMPERATURE_REFRESH_INTERVAL {
            self.components.refresh(false);
            self.package_temperature_celsius = cpu_package_temperature(&self.components);
            self.last_temperature_refresh = now;
        }
    }

    fn collect(&mut self) -> Result<SystemSnapshot, CollectionError> {
        let now = Instant::now();
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();

        if now.duration_since(self.last_cpu_frequency_refresh) >= CPU_FREQUENCY_REFRESH_INTERVAL {
            self.system
                .refresh_cpu_specifics(CpuRefreshKind::nothing().with_frequency());
            self.last_cpu_frequency_refresh = now;
        }

        self.refresh_supplemental_metrics(now);

        if self.logical_cores == 0 {
            return Err(CollectionError::Cpu);
        }

        let usage_percent = self.system.global_cpu_usage();
        if !usage_percent.is_finite() {
            return Err(CollectionError::Cpu);
        }

        let per_core = self
            .system
            .cpus()
            .iter()
            .enumerate()
            .map(|(index, cpu)| {
                let usage_percent = cpu.cpu_usage();
                if usage_percent.is_finite() {
                    Ok(CpuCoreInfo {
                        index,
                        usage_percent,
                        frequency_mhz: cpu.frequency(),
                    })
                } else {
                    Err(CollectionError::Cpu)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

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
                physical_cores: self.physical_cores,
                package_temperature_celsius: self.package_temperature_celsius,
                per_core,
            },
            memory: MemoryInfo {
                total_bytes: total_memory,
                used_bytes: used_memory,
                available_bytes: self.system.available_memory(),
                usage_percent: percentage(used_memory, total_memory),
            },
            swap: SwapInfo {
                total_bytes: self.system.total_swap(),
                used_bytes: self.system.used_swap(),
                available_bytes: self.system.free_swap(),
                usage_percent: percentage(self.system.used_swap(), self.system.total_swap()),
            },
            uptime: UptimeInfo {
                seconds: System::uptime(),
            },
            disks: self.disk_metrics.clone(),
            networks: self.network_metrics.clone(),
            gpus: self.gpu_metrics.clone(),
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

fn disk_metrics(disks: &Disks, elapsed: Duration) -> Vec<DiskInfo> {
    disks
        .list()
        .iter()
        .map(|disk| {
            let total_bytes = disk.total_space();
            let available_bytes = disk.available_space();
            let usage = disk.usage();

            DiskInfo {
                name: disk.name().to_string_lossy().into_owned(),
                mount_point: disk.mount_point().to_string_lossy().into_owned(),
                file_system: disk.file_system().to_string_lossy().into_owned(),
                total_bytes,
                available_bytes,
                usage_percent: percentage(total_bytes.saturating_sub(available_bytes), total_bytes),
                read_bytes_per_sec: bytes_per_second(usage.read_bytes, elapsed),
                write_bytes_per_sec: bytes_per_second(usage.written_bytes, elapsed),
                read_only: disk.is_read_only(),
                removable: disk.is_removable(),
            }
        })
        .collect()
}

fn network_metrics(networks: &Networks, elapsed: Duration) -> Vec<NetworkInfo> {
    networks
        .iter()
        .map(|(name, network)| NetworkInfo {
            name: name.clone(),
            received_bytes_per_sec: bytes_per_second(network.received(), elapsed),
            transmitted_bytes_per_sec: bytes_per_second(network.transmitted(), elapsed),
            total_received_bytes: network.total_received(),
            total_transmitted_bytes: network.total_transmitted(),
        })
        .collect()
}

fn cpu_package_temperature(components: &Components) -> Option<f32> {
    let component_temperature = components.list().iter().find_map(|component| {
        let temperature = component.temperature()?;
        if !temperature.is_finite() {
            return None;
        }

        let label = component.label().to_ascii_lowercase();
        let id = component.id().unwrap_or_default().to_ascii_lowercase();
        let is_package_sensor = label.contains("package")
            || id.contains("package")
            || label == "computer"
            || id == "computer"
            || (label.contains("cpu") && !label.contains("core"));

        is_package_sensor.then_some(temperature)
    });

    component_temperature
        .or_else(windows_thermal_zone_temperature)
        .or_else(macos_cpu_package_temperature)
}

#[cfg(windows)]
fn windows_thermal_zone_temperature() -> Option<f32> {
    use windows::{
        Win32::System::Performance::{
            PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE,
            PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
            PdhGetFormattedCounterValue, PdhOpenQueryW,
        },
        core::w,
    };

    let mut raw_query = PDH_HQUERY::default();
    let status = unsafe { PdhOpenQueryW(None, 0, &mut raw_query) };
    if status != 0 || raw_query.is_invalid() {
        return None;
    }

    struct QueryGuard(PDH_HQUERY);

    impl Drop for QueryGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = PdhCloseQuery(self.0);
            }
        }
    }

    let query = QueryGuard(raw_query);
    let mut counter = PDH_HCOUNTER::default();
    let status = unsafe {
        PdhAddEnglishCounterW(
            query.0,
            w!(r"\Thermal Zone Information(\_TZ.THRM)\Temperature"),
            0,
            &mut counter,
        )
    };
    if status != 0 || counter.is_invalid() {
        return None;
    }

    // The first collection initializes the PDH counter instance. Read again
    // so the formatted value is available even when the first sample is not.
    let status = unsafe { PdhCollectQueryData(query.0) };
    if status != 0 || unsafe { PdhCollectQueryData(query.0) } != 0 {
        return None;
    }

    let mut value = PDH_FMT_COUNTERVALUE::default();
    let status = unsafe { PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, None, &mut value) };
    if status != 0
        || (value.CStatus != PDH_CSTATUS_VALID_DATA && value.CStatus != PDH_CSTATUS_NEW_DATA)
    {
        return None;
    }

    let kelvin = unsafe { value.Anonymous.doubleValue };
    kelvin_to_celsius(kelvin)
}

#[cfg(not(windows))]
fn windows_thermal_zone_temperature() -> Option<f32> {
    None
}

#[cfg(target_os = "macos")]
fn macos_cpu_package_temperature() -> Option<f32> {
    crate::macos_smc::cpu_package_temperature()
}

#[cfg(not(target_os = "macos"))]
fn macos_cpu_package_temperature() -> Option<f32> {
    None
}

#[cfg(windows)]
fn kelvin_to_celsius(kelvin: f64) -> Option<f32> {
    if !kelvin.is_finite() || !(173.15..=473.15).contains(&kelvin) {
        return None;
    }

    Some((kelvin - 273.15) as f32)
}

fn bytes_per_second(bytes: u64, elapsed: Duration) -> f64 {
    if elapsed.is_zero() {
        0.0
    } else {
        bytes as f64 / elapsed.as_secs_f64()
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
    use std::time::Duration;

    use super::{Collector, HealthResponse, PROGRAM_NAME, bytes_per_second, percentage};

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
        assert!(json["system"]["kernel_version"].as_str().is_some());
        assert!(json["cpu"]["logical_cores"].is_u64());
        assert!(json["cpu"].get("package_temperature_celsius").is_some());
        assert!(json["cpu"]["per_core"].is_array());
        assert!(json["swap"].is_object());
        assert!(json["disks"].is_array());
        assert!(json["networks"].is_array());
        assert!(json["gpus"].is_array());
        assert!(json.get("process").is_none());
        assert!(
            json["gpus"]
                .as_array()
                .expect("gpus should be an array")
                .iter()
                .all(|gpu| gpu.get("driver_version").is_none())
        );
    }

    #[test]
    fn failed_collection_is_not_healthy() {
        let state = super::MonitorState::from_result(Err(super::CollectionError::Cpu));

        assert!(!state.is_healthy());
        assert_eq!(state.last_updated_at(), None);
    }

    #[test]
    fn health_response_contains_program_name_and_status() {
        let response = HealthResponse {
            name: PROGRAM_NAME,
            status: "OK",
        };
        let json = serde_json::to_value(response).expect("health response should serialize");

        assert_eq!(json["name"], "Background_System_Monitor");
        assert_eq!(json["status"], "OK");
    }

    #[test]
    fn bytes_per_second_uses_the_refresh_interval() {
        assert_eq!(bytes_per_second(1_000, Duration::from_secs(2)), 500.0);
        assert_eq!(bytes_per_second(1_000, Duration::ZERO), 0.0);
    }

    #[cfg(windows)]
    #[test]
    fn kelvin_temperature_is_converted_to_celsius() {
        let temperature = super::kelvin_to_celsius(340.0).expect("valid temperature");
        assert!((temperature - 66.85).abs() < 0.01);
    }

    #[cfg(windows)]
    #[test]
    fn implausible_kelvin_temperature_is_rejected() {
        assert!(super::kelvin_to_celsius(f64::NAN).is_none());
        assert!(super::kelvin_to_celsius(0.0).is_none());
    }
}
