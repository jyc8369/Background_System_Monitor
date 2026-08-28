# Background System Monitor

[한국어](#korean) | [English](#english)

<a id="english"></a>

## English

[한국어](#korean)

A minimal cross-platform system monitoring API server built with Axum, Tokio, and `sysinfo`.

### Scope

- Windows 10/11 x64
- Ubuntu 22.04+ x64
- macOS Intel and Apple Silicon
- Physical servers and virtual machines
- Docker and Kubernetes are currently out of scope
- Manual executable distribution
- No authentication in the current MVP

### Collected metrics

- Overall CPU usage and logical core count
- Overall CPU package/thermal-zone temperature when the operating system exposes one
- Per-core CPU usage and frequency
- Total, used, available, and percentage memory
- Swap usage
- System uptime
- Hostname, OS name, OS version, kernel version, and architecture
- CPU model and physical core count
- Per-disk capacity, availability, and I/O rates
- Per-network-interface RX/TX rates and totals
- Best-effort GPU adapter name, vendor, VRAM, utilization, temperature, and power

CPU, memory, swap, uptime, and per-core CPU usage are refreshed every second using one reusable `sysinfo::System` instance. The overall CPU package temperature is refreshed every five seconds from the package/thermal sensor only; individual core temperatures are not collected. CPU frequency is refreshed every ten seconds, network counters and disk I/O every five seconds, and disk capacity every thirty seconds. The collector does not scan process lists or collect the API process's own metrics. API requests return the latest in-memory snapshot instead of querying the operating system on every request.

GPU data is refreshed every sixty seconds through platform-native APIs only. NVIDIA telemetry uses the native NVML library when it is available, and AMD Windows telemetry uses the native ADLX library when the AMD driver exposes it; Windows DXGI still provides adapter inventory when vendor telemetry is unavailable. On macOS, Metal provides GPU inventory and memory metrics, while the native IOKit SMC interface is probed for common CPU/GPU temperature and power keys. SMC key availability varies by Mac model and operating-system generation, especially between Intel and Apple Silicon, so unsupported values remain `null`. A machine without a GPU returns `"gpus": []`; unsupported GPU fields are `null`. GPU driver information is intentionally excluded. On Windows, the CPU temperature fallback reads the standard thermal-zone performance counter; this may be a firmware thermal-zone value rather than an exact silicon package sensor.

If a required CPU or memory metric cannot be collected, `/api/system` returns HTTP 503 with a stable error JSON response and `/health` returns HTTP 503. Disk and network data are supplemental and do not make the entire API unhealthy when unavailable. A snapshot that has not been refreshed for more than three seconds is also considered unhealthy.

### API

#### `GET /api/system`

Example response:

```json
{
  "timestamp": 1787812301,
  "system": {
    "hostname": "server-01",
    "os": "Linux",
    "os_version": "Ubuntu 24.04",
    "kernel_version": "6.8.0",
    "architecture": "x86_64",
    "cpu_model": "AMD EPYC"
  },
  "cpu": {
    "usage_percent": 21.7,
    "logical_cores": 16,
    "physical_cores": 8,
    "package_temperature_celsius": 54.0,
    "per_core": [
      {
        "index": 0,
        "usage_percent": 18.2,
        "frequency_mhz": 3200
      }
    ]
  },
  "memory": {
    "total_bytes": 17179869184,
    "used_bytes": 7381975040,
    "available_bytes": 9797894144,
    "usage_percent": 42.97
  },
  "swap": {
    "total_bytes": 4294967296,
    "used_bytes": 524288000,
    "available_bytes": 3770679296,
    "usage_percent": 12.21
  },
  "uptime": {
    "seconds": 583921
  },
  "disks": [
    {
      "name": "/dev/nvme0n1",
      "mount_point": "/",
      "file_system": "ext4",
      "total_bytes": 512000000000,
      "available_bytes": 218000000000,
      "usage_percent": 57.42,
      "read_bytes_per_sec": 1048576.0,
      "write_bytes_per_sec": 524288.0,
      "read_only": false,
      "removable": false
    }
  ],
  "networks": [
    {
      "name": "eth0",
      "received_bytes_per_sec": 204800.0,
      "transmitted_bytes_per_sec": 102400.0,
      "total_received_bytes": 123456789,
      "total_transmitted_bytes": 98765432
    }
  ],
  "gpus": [
    {
      "index": 0,
      "name": "Example GPU",
      "vendor": "Example Vendor",
      "memory_total_bytes": 8589934592,
      "memory_used_bytes": null,
      "utilization_percent": null,
      "temperature_celsius": null,
      "power_watts": null
    }
  ]
}
```

#### `GET /health`

Healthy response:

```json
{
  "name": "Background_System_Monitor",
  "status": "OK"
}
```

Unhealthy response:

```json
{
  "name": "Background_System_Monitor",
  "status": "UNAVAILABLE"
}
```

The HTTP status is `200` when healthy and `503` when unhealthy.

#### `POST /control/shutdown`

Requests a graceful server shutdown after the current request is handled.

```bash
curl -X POST http://127.0.0.1:3001/control/shutdown
```

Authentication is not currently provided. If the server is bound to a non-loopback address, any client that can access the endpoint may shut down the server. Restrict access with the default loopback binding or a firewall.

### Run

The default binding is:

```text
127.0.0.1:3001
```

The binding address is read once at startup from:

```text
BACKGROUND_SYSTEM_MONITOR_BIND_ADDR
```

Run with the default binding:

```bash
cargo run
```

Run with a custom binding on Unix-like shells:

```bash
BACKGROUND_SYSTEM_MONITOR_BIND_ADDR=0.0.0.0:8080 cargo run
```

Run a Windows release executable with a custom binding in PowerShell:

```powershell
$env:BACKGROUND_SYSTEM_MONITOR_BIND_ADDR = "192.168.1.20:3001"
Start-Process `
  -FilePath "C:\app\background-system-monitor-windows-x64.exe" `
  -WorkingDirectory "C:\app"
```

The environment variable only affects processes started after it is set. Restart the executable after changing the value. If the variable is not set, the default loopback binding is used.

The Windows release executable is built without a console window. Debug builds and `cargo run` keep console output for development.

### Verification

```bash
cargo fmt --check
cargo test -- --test-threads=1
cargo clippy --all-targets --all-features -- -D warnings
```

### Release artifacts

The release workflow runs only when a version tag matching `v*.*.*` is pushed. Regular branch pushes do not trigger the release workflow.

Before packaging, the workflow runs native macOS tests on both Intel and Apple Silicon runners. The smoke test starts the binary, calls `/health` and `/api/system`, and verifies the CPU temperature field and GPU array schema. Hardware sensors may still legitimately return `null` on a virtualized or unsupported Mac runner.

The workflow builds and attaches these four binaries to a GitHub Release:

```text
background-system-monitor-windows-x64.exe
background-system-monitor-linux-x64
background-system-monitor-macos-x64
background-system-monitor-macos-arm64
```

The project currently uses manual executable distribution and does not register a system service.

### Future extensions

Process lists and process metrics are intentionally excluded. GPU adapter telemetry and CPU package temperature are best-effort and do not affect API health; GPU driver information is intentionally excluded. Additional hardware sensor data and long-term history can be added later.

<a id="korean"></a>

## 한국어

[English](#english)

Axum, Tokio, `sysinfo`를 사용한 최소 크로스플랫폼 시스템 모니터링 API 서버입니다.

### 범위

- Windows 10/11 x64
- Ubuntu 22.04+ x64
- macOS Intel 및 Apple Silicon
- 물리 서버 및 가상 머신
- Docker와 Kubernetes는 현재 범위에서 제외
- 수동 실행 파일 배포
- 현재 MVP에는 인증 없음

### 수집 지표

- 전체 CPU 사용률 및 논리 코어 수
- 운영체제가 package/thermal-zone 센서를 제공하는 경우 전체 CPU 온도
- CPU 코어별 사용률 및 주파수
- 전체·사용·가용 메모리와 메모리 사용률
- Swap 사용량
- 시스템 uptime
- hostname, OS 이름, OS 버전, kernel version, architecture
- CPU 모델명 및 물리 코어 수
- 디스크별 용량·여유 공간·I/O rate
- 네트워크 인터페이스별 RX/TX rate와 누적량
- GPU별 이름·제조사·VRAM·사용률·온도·전력 정보(best-effort)

CPU·메모리·swap·uptime·코어별 CPU 사용률은 재사용되는 `sysinfo::System` 객체 하나를 사용해 1초마다 갱신합니다. 전체 CPU package 온도는 코어별 온도를 섞지 않고 package/thermal sensor만 선택해 5초마다 갱신합니다. CPU 주파수는 10초, 네트워크 counter와 디스크 I/O는 5초, 디스크 용량은 30초마다 갱신합니다. 프로세스 목록을 스캔하지 않으며 API 프로세스 자신의 지표도 수집하지 않습니다. API 요청이 들어올 때마다 운영체제를 조회하지 않고 메모리에 저장된 최신 snapshot을 반환합니다.

GPU 정보는 OS 네이티브 API만 사용해 60초마다 best-effort 방식으로 갱신합니다. NVIDIA는 NVML 네이티브 라이브러리가 제공되는 경우, AMD Windows GPU는 AMD 드라이버가 ADLX를 제공하는 경우 온도·사용률·전력 등을 수집합니다. Windows DXGI는 vendor telemetry가 없어도 GPU 기본 정보를 제공합니다. macOS에서는 Metal로 GPU 기본 정보와 메모리를 수집하고, 네이티브 IOKit SMC에서 일반적인 CPU/GPU 온도·전력 키를 조회합니다. SMC 키 제공 여부는 Mac 모델과 운영체제 세대에 따라 다르고 Intel과 Apple Silicon 사이에도 차이가 있으므로 지원되지 않는 값은 `null`로 둡니다. GPU가 없는 장비는 `"gpus": []`를 반환하고, 지원되지 않는 GPU 필드는 `null`입니다. GPU 드라이버 정보는 의도적으로 제외합니다. Windows CPU 온도 fallback은 표준 thermal-zone 성능 counter를 사용하므로, 펌웨어 thermal-zone 값일 수 있으며 실리콘 package 센서와 정확히 같다고 보장하지 않습니다.

필수 CPU 또는 메모리 지표를 수집할 수 없으면 `/api/system`은 고정된 오류 JSON과 HTTP 503을 반환하고 `/health`도 HTTP 503을 반환합니다. 디스크·네트워크 데이터는 보조 지표이므로 수집할 수 없어도 전체 API를 비정상으로 만들지 않습니다. snapshot이 3초 넘게 갱신되지 않은 경우에도 비정상 상태로 처리합니다.

### API

#### `GET /api/system`

응답 예시:

```json
{
  "timestamp": 1787812301,
  "system": {
    "hostname": "server-01",
    "os": "Linux",
    "os_version": "Ubuntu 24.04",
    "kernel_version": "6.8.0",
    "architecture": "x86_64",
    "cpu_model": "AMD EPYC"
  },
  "cpu": {
    "usage_percent": 21.7,
    "logical_cores": 16,
    "physical_cores": 8,
    "package_temperature_celsius": 54.0,
    "per_core": [
      {
        "index": 0,
        "usage_percent": 18.2,
        "frequency_mhz": 3200
      }
    ]
  },
  "memory": {
    "total_bytes": 17179869184,
    "used_bytes": 7381975040,
    "available_bytes": 9797894144,
    "usage_percent": 42.97
  },
  "swap": {
    "total_bytes": 4294967296,
    "used_bytes": 524288000,
    "available_bytes": 3770679296,
    "usage_percent": 12.21
  },
  "uptime": {
    "seconds": 583921
  },
  "disks": [
    {
      "name": "/dev/nvme0n1",
      "mount_point": "/",
      "file_system": "ext4",
      "total_bytes": 512000000000,
      "available_bytes": 218000000000,
      "usage_percent": 57.42,
      "read_bytes_per_sec": 1048576.0,
      "write_bytes_per_sec": 524288.0,
      "read_only": false,
      "removable": false
    }
  ],
  "networks": [
    {
      "name": "eth0",
      "received_bytes_per_sec": 204800.0,
      "transmitted_bytes_per_sec": 102400.0,
      "total_received_bytes": 123456789,
      "total_transmitted_bytes": 98765432
    }
  ],
  "gpus": [
    {
      "index": 0,
      "name": "Example GPU",
      "vendor": "Example Vendor",
      "memory_total_bytes": 8589934592,
      "memory_used_bytes": null,
      "utilization_percent": null,
      "temperature_celsius": null,
      "power_watts": null
    }
  ]
}
```

#### `GET /health`

정상 응답:

```json
{
  "name": "Background_System_Monitor",
  "status": "OK"
}
```

비정상 응답:

```json
{
  "name": "Background_System_Monitor",
  "status": "UNAVAILABLE"
}
```

정상일 때 HTTP 상태 코드는 `200`, 비정상일 때는 `503`입니다.

#### `POST /control/shutdown`

현재 요청을 처리한 뒤 서버를 graceful shutdown하도록 요청합니다.

```bash
curl -X POST http://127.0.0.1:3001/control/shutdown
```

현재 인증은 제공하지 않습니다. loopback이 아닌 주소에 바인딩하면 endpoint에 접근할 수 있는 클라이언트가 서버를 종료할 수 있습니다. 기본 loopback 바인딩을 유지하거나 방화벽으로 접근을 제한해야 합니다.

### 실행

기본 바인딩 주소는 다음과 같습니다.

```text
127.0.0.1:3001
```

바인딩 주소는 시작 시 다음 환경변수에서 한 번 읽습니다.

```text
BACKGROUND_SYSTEM_MONITOR_BIND_ADDR
```

기본 주소로 실행:

```bash
cargo run
```

Unix 계열 shell에서 다른 주소로 실행:

```bash
BACKGROUND_SYSTEM_MONITOR_BIND_ADDR=0.0.0.0:8080 cargo run
```

PowerShell에서 Windows release 실행 파일을 다른 주소로 실행:

```powershell
$env:BACKGROUND_SYSTEM_MONITOR_BIND_ADDR = "192.168.1.20:3001"
Start-Process `
  -FilePath "C:\app\background-system-monitor-windows-x64.exe" `
  -WorkingDirectory "C:\app"
```

환경변수는 설정한 이후에 시작되는 프로세스에만 적용됩니다. 값을 변경하면 실행 파일을 다시 시작해야 합니다. 환경변수가 없으면 기본 loopback 주소를 사용합니다.

Windows release 실행 파일은 콘솔 창이 뜨지 않도록 빌드됩니다. debug 빌드와 `cargo run`은 개발 편의를 위해 콘솔 출력을 사용합니다.

### 검증

```bash
cargo fmt --check
cargo test -- --test-threads=1
cargo clippy --all-targets --all-features -- -D warnings
```

### 릴리즈 산출물

릴리즈 workflow는 `v*.*.*` 형식의 버전 태그를 push할 때만 실행됩니다. 일반 브랜치 push에서는 릴리즈 workflow가 실행되지 않습니다.

패키징 전에 workflow가 Intel 및 Apple Silicon 네이티브 macOS runner에서 테스트를 실행합니다. smoke test는 바이너리를 시작하고 `/health`와 `/api/system`을 호출한 뒤 CPU 온도 필드와 GPU 배열 스키마를 확인합니다. 가상화되었거나 지원되지 않는 Mac runner에서는 하드웨어 센서가 정상적으로 `null`을 반환할 수 있습니다.

workflow는 다음 4개 바이너리를 빌드해 GitHub Release에 첨부합니다.

```text
background-system-monitor-windows-x64.exe
background-system-monitor-linux-x64
background-system-monitor-macos-x64
background-system-monitor-macos-arm64
```

현재 프로젝트는 수동 실행 파일 방식으로 배포하며 시스템 서비스 등록은 하지 않습니다.

### 향후 확장

프로세스 목록과 프로세스 지표는 의도적으로 제외합니다. GPU adapter telemetry와 CPU package 온도는 best-effort이며 API health에 영향을 주지 않습니다. GPU 드라이버 정보는 의도적으로 제외합니다. 추가 하드웨어 센서 데이터와 장기 history는 나중에 확장할 수 있습니다.
