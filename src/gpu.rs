use serde::Serialize;

/// Best-effort GPU information. Unsupported metrics are represented as null.
#[derive(Clone, Debug, Serialize)]
pub struct GpuInfo {
    pub index: usize,
    pub name: Option<String>,
    pub vendor: Option<String>,
    pub memory_total_bytes: Option<u64>,
    pub memory_used_bytes: Option<u64>,
    pub utilization_percent: Option<f32>,
    pub temperature_celsius: Option<f32>,
    pub power_watts: Option<f32>,
}

pub fn collect() -> Vec<GpuInfo> {
    platform::collect()
}

#[cfg(target_os = "macos")]
fn macos_vendor_from_name(name: &str) -> Option<String> {
    let name = name.to_ascii_lowercase();
    let vendor = if name.contains("intel") {
        "Intel"
    } else if name.contains("amd") || name.contains("radeon") {
        "AMD"
    } else if name.contains("nvidia") || name.contains("geforce") {
        "NVIDIA"
    } else if name.contains("apple") {
        "Apple"
    } else {
        return None;
    };

    Some(vendor.to_owned())
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
struct NvmlGpuInfo {
    name: String,
    memory_total_bytes: Option<u64>,
    memory_used_bytes: Option<u64>,
    utilization_percent: Option<f32>,
    temperature_celsius: Option<f32>,
    power_watts: Option<f32>,
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn collect_nvml() -> Vec<NvmlGpuInfo> {
    use nvml_wrapper::{Nvml, enum_wrappers::device::TemperatureSensor};

    let Ok(nvml) = Nvml::init() else {
        return Vec::new();
    };
    let Ok(device_count) = nvml.device_count() else {
        return Vec::new();
    };

    (0..device_count)
        .filter_map(|index| {
            let device = nvml.device_by_index(index).ok()?;
            let name = device.name().ok()?.trim().to_owned();
            if name.is_empty() {
                return None;
            }

            let memory = device.memory_info().ok();
            let utilization_percent = device
                .utilization_rates()
                .ok()
                .map(|usage| usage.gpu as f32);
            let temperature_celsius = device
                .temperature(TemperatureSensor::Gpu)
                .ok()
                .map(|temperature| temperature as f32);
            let power_watts = device
                .power_usage()
                .ok()
                .map(|power_milliwatts| power_milliwatts as f32 / 1_000.0);

            Some(NvmlGpuInfo {
                name,
                memory_total_bytes: memory.as_ref().map(|info| info.total),
                memory_used_bytes: memory.as_ref().map(|info| info.used),
                utilization_percent,
                temperature_celsius,
                power_watts,
            })
        })
        .collect()
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn merge_nvml_metrics(gpus: &mut Vec<GpuInfo>) {
    for telemetry in collect_nvml() {
        let matching_gpu = gpus.iter_mut().find(|gpu| {
            gpu.name
                .as_deref()
                .is_some_and(|name| name.trim().eq_ignore_ascii_case(&telemetry.name))
                || (gpu.vendor.as_deref() == Some("NVIDIA")
                    && gpu.memory_total_bytes == telemetry.memory_total_bytes)
        });

        if let Some(gpu) = matching_gpu {
            gpu.memory_total_bytes = telemetry.memory_total_bytes.or(gpu.memory_total_bytes);
            gpu.memory_used_bytes = telemetry.memory_used_bytes;
            gpu.utilization_percent = telemetry.utilization_percent;
            gpu.temperature_celsius = telemetry.temperature_celsius;
            gpu.power_watts = telemetry.power_watts;
        } else {
            gpus.push(GpuInfo {
                index: gpus.len(),
                name: Some(telemetry.name),
                vendor: Some("NVIDIA".to_owned()),
                memory_total_bytes: telemetry.memory_total_bytes,
                memory_used_bytes: telemetry.memory_used_bytes,
                utilization_percent: telemetry.utilization_percent,
                temperature_celsius: telemetry.temperature_celsius,
                power_watts: telemetry.power_watts,
            });
        }
    }
}

#[cfg(target_os = "windows")]
struct AdlxGpuInfo {
    name: String,
    utilization_percent: Option<f32>,
    temperature_celsius: Option<f32>,
    power_watts: Option<f32>,
}

#[cfg(target_os = "windows")]
fn collect_adlx() -> Vec<AdlxGpuInfo> {
    let Ok(helper) = adlx::AdlxHelper::new() else {
        return Vec::new();
    };
    let system = helper.system();
    let Ok(gpu_list) = system.gpus() else {
        return Vec::new();
    };
    let Ok(services) = system.performance_monitoring_services() else {
        return Vec::new();
    };

    (0..gpu_list.size())
        .filter_map(|index| {
            let gpu = gpu_list.at(index).ok()?;
            let name = gpu.name().ok()?.trim().to_owned();
            if name.is_empty() {
                return None;
            }

            let metrics = services.current_gpu_metrics(&gpu).ok();
            let utilization_percent = metrics
                .as_ref()
                .and_then(|metrics| metrics.usage().ok())
                .and_then(valid_percent);
            let temperature_celsius = metrics
                .as_ref()
                .and_then(|metrics| metrics.temperature().ok())
                .and_then(valid_temperature);
            let power_watts = metrics
                .as_ref()
                .and_then(|metrics| metrics.power().ok())
                .and_then(valid_power);

            Some(AdlxGpuInfo {
                name,
                utilization_percent,
                temperature_celsius,
                power_watts,
            })
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn merge_adlx_metrics(gpus: &mut Vec<GpuInfo>) {
    for telemetry in collect_adlx() {
        let exact_index = gpus.iter().position(|gpu| {
            gpu.vendor.as_deref() == Some("AMD")
                && gpu
                    .name
                    .as_deref()
                    .is_some_and(|name| name.trim().eq_ignore_ascii_case(&telemetry.name))
        });
        let fallback_index = gpus.iter().position(|gpu| {
            gpu.vendor.as_deref() == Some("AMD") && gpu.temperature_celsius.is_none()
        });
        let matching_index = exact_index.or(fallback_index);

        if let Some(index) = matching_index {
            let gpu = &mut gpus[index];
            gpu.utilization_percent = telemetry.utilization_percent;
            gpu.temperature_celsius = telemetry.temperature_celsius;
            gpu.power_watts = telemetry.power_watts;
        } else {
            gpus.push(GpuInfo {
                index: gpus.len(),
                name: Some(telemetry.name),
                vendor: Some("AMD".to_owned()),
                memory_total_bytes: None,
                memory_used_bytes: None,
                utilization_percent: telemetry.utilization_percent,
                temperature_celsius: telemetry.temperature_celsius,
                power_watts: telemetry.power_watts,
            });
        }
    }
}

#[cfg(target_os = "windows")]
fn valid_percent(value: f64) -> Option<f32> {
    value
        .is_finite()
        .then_some(value as f32)
        .filter(|value| (0.0..=100.0).contains(value))
}

#[cfg(target_os = "windows")]
fn valid_temperature(value: f64) -> Option<f32> {
    value
        .is_finite()
        .then_some(value as f32)
        .filter(|value| (-50.0..=200.0).contains(value))
}

#[cfg(target_os = "windows")]
fn valid_power(value: f64) -> Option<f32> {
    value
        .is_finite()
        .then_some(value as f32)
        .filter(|value| (0.0..=10_000.0).contains(value))
}

#[cfg(target_os = "windows")]
mod platform {
    use super::GpuInfo;
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
    };
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

    pub fn collect() -> Vec<GpuInfo> {
        let com_initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() };
        let mut result = collect_adapters();
        super::merge_nvml_metrics(&mut result);
        super::merge_adlx_metrics(&mut result);
        if com_initialized {
            unsafe {
                CoUninitialize();
            }
        }
        result
    }

    fn collect_adapters() -> Vec<GpuInfo> {
        let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
            return Vec::new();
        };

        let mut result = Vec::new();
        let mut index = 0;
        loop {
            let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
                break;
            };

            let Ok(description) = (unsafe { adapter.GetDesc1() }) else {
                index += 1;
                continue;
            };

            if description.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                index += 1;
                continue;
            }

            let name_end = description
                .Description
                .iter()
                .position(|character| *character == 0)
                .unwrap_or(description.Description.len());
            let name = String::from_utf16_lossy(&description.Description[..name_end]);
            let memory_total_bytes = u64::try_from(description.DedicatedVideoMemory).ok();

            result.push(GpuInfo {
                index: result.len(),
                name: (!name.is_empty()).then_some(name),
                vendor: vendor_name(description.VendorId),
                memory_total_bytes,
                memory_used_bytes: None,
                utilization_percent: None,
                temperature_celsius: None,
                power_watts: None,
            });
            index += 1;
        }

        result
    }

    fn vendor_name(vendor_id: u32) -> Option<String> {
        let name = match vendor_id {
            0x1002 => "AMD",
            0x10DE => "NVIDIA",
            0x8086 => "Intel",
            0x106B => "Apple",
            _ => return None,
        };
        Some(name.to_owned())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::{fs, path::Path};

    use super::GpuInfo;

    pub fn collect() -> Vec<GpuInfo> {
        let Ok(entries) = fs::read_dir("/sys/class/drm") else {
            return Vec::new();
        };

        let mut cards = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let index = name.strip_prefix("card")?.parse::<usize>().ok()?;
                if name.contains('-') {
                    return None;
                }
                Some((index, entry.path()))
            })
            .collect::<Vec<_>>();
        cards.sort_by_key(|(index, _)| *index);

        let mut result = cards
            .into_iter()
            .enumerate()
            .map(|(index, (_, card_path))| collect_card(index, &card_path))
            .collect::<Vec<_>>();
        super::merge_nvml_metrics(&mut result);
        result
    }

    fn collect_card(index: usize, card_path: &Path) -> GpuInfo {
        let device_path = card_path.join("device");
        let vendor_id = read_trimmed(&device_path.join("vendor"));
        let name = read_trimmed(&device_path.join("label"))
            .or_else(|| read_trimmed(&device_path.join("product_name")))
            .or_else(|| Some(format!("GPU {index}")));
        let memory_total_bytes = read_u64(&device_path.join("mem_info_vram_total"));
        let memory_used_bytes = read_u64(&device_path.join("mem_info_vram_used"));
        let utilization_percent = read_f32(&device_path.join("gpu_busy_percent"));
        let (temperature_celsius, power_watts) = read_hwmon(&device_path);

        GpuInfo {
            index,
            name,
            vendor: vendor_id.and_then(|id| vendor_name(&id)),
            memory_total_bytes,
            memory_used_bytes,
            utilization_percent,
            temperature_celsius,
            power_watts,
        }
    }

    fn read_hwmon(device_path: &Path) -> (Option<f32>, Option<f32>) {
        let Ok(entries) = fs::read_dir(device_path.join("hwmon")) else {
            return (None, None);
        };

        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let temperature = read_f32(&path.join("temp1_input")).map(|value| value / 1000.0);
            let power = read_f32(&path.join("power1_average"))
                .or_else(|| read_f32(&path.join("power1_input")))
                .map(|value| value / 1_000_000.0);
            if temperature.is_some() || power.is_some() {
                return (temperature, power);
            }
        }
        (None, None)
    }

    fn read_trimmed(path: &Path) -> Option<String> {
        fs::read_to_string(path)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    }

    fn read_u64(path: &Path) -> Option<u64> {
        read_trimmed(path)?.parse().ok()
    }

    fn read_f32(path: &Path) -> Option<f32> {
        read_trimmed(path)?.parse().ok()
    }

    fn vendor_name(vendor_id: &str) -> Option<String> {
        let name = match vendor_id
            .trim_start_matches("0x")
            .to_ascii_lowercase()
            .as_str()
        {
            "1002" => "AMD",
            "10de" => "NVIDIA",
            "8086" => "Intel",
            "106b" => "Apple",
            _ => return None,
        };
        Some(name.to_owned())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::GpuInfo;

    pub fn collect() -> Vec<GpuInfo> {
        let mut result = metal::Device::all()
            .into_iter()
            .enumerate()
            .map(|(index, device)| GpuInfo {
                index,
                name: Some(device.name().to_owned()),
                vendor: super::macos_vendor_from_name(device.name()),
                memory_total_bytes: Some(device.recommended_max_working_set_size()),
                memory_used_bytes: Some(device.current_allocated_size()),
                utilization_percent: None,
                temperature_celsius: None,
                power_watts: None,
            })
            .collect::<Vec<_>>();
        apply_smc_metrics(&mut result);
        result
    }

    fn apply_smc_metrics(gpus: &mut [GpuInfo]) {
        let (temperature, power) = crate::macos_smc::gpu_metrics();

        if let Some(gpu) = gpus.first_mut() {
            gpu.temperature_celsius = temperature;
            gpu.power_watts = power;
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod platform {
    use super::GpuInfo;

    pub fn collect() -> Vec<GpuInfo> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::GpuInfo;

    #[test]
    fn gpu_schema_uses_null_for_unsupported_metrics_and_has_no_driver_fields() {
        let gpu = GpuInfo {
            index: 0,
            name: Some("Test GPU".to_owned()),
            vendor: Some("Test Vendor".to_owned()),
            memory_total_bytes: Some(1024),
            memory_used_bytes: None,
            utilization_percent: None,
            temperature_celsius: None,
            power_watts: None,
        };
        let json = serde_json::to_value(gpu).expect("GPU information should serialize");

        assert_eq!(json["index"], 0);
        assert_eq!(json["name"], "Test GPU");
        assert!(json["memory_used_bytes"].is_null());
        assert!(json["utilization_percent"].is_null());
        assert!(json["temperature_celsius"].is_null());
        assert!(json["power_watts"].is_null());
        assert!(json.get("driver_version").is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_vendor_is_inferred_from_the_metal_device_name() {
        assert_eq!(
            super::macos_vendor_from_name("Intel Iris Plus"),
            Some("Intel".to_owned())
        );
        assert_eq!(
            super::macos_vendor_from_name("Apple M-series GPU"),
            Some("Apple".to_owned())
        );
        assert_eq!(super::macos_vendor_from_name("Unknown GPU"), None);
    }
}
