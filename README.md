# Background System Monitor

Axum API 서버 안에서 `sysinfo` 기반의 최신 시스템 스냅샷을 백그라운드에서 수집하는 최소 구현입니다.

## 현재 범위

- Windows, Linux, macOS 공통 코드
- 전체 CPU 사용률과 논리 코어 수
- 전체 메모리(total/used/available/usage)
- 시스템 uptime
- `GET /api/system`
- `GET /health`
- `POST /control/shutdown`

CPU·메모리·uptime은 하나의 재사용되는 `sysinfo::System` 인스턴스로 1초마다 갱신합니다. 프로세스 목록이나 자기 프로세스 정보를 수집하지 않으며, API 요청 시 OS를 직접 조회하지 않고 `Arc<RwLock<SystemSnapshot>>`의 최신 값을 반환합니다.

필수 지표 중 하나라도 수집에 실패하면 `/api/system`은 HTTP 503과 고정된 오류 JSON을 반환하고, `/health`는 HTTP 503과 `UNAVAILABLE`을 반환합니다. 정상 상태의 `/health` 응답 본문은 `OK`입니다. collector가 중단되어 3초 이상 snapshot이 갱신되지 않은 경우에도 같은 비정상 상태로 처리합니다.

## 실행

```text
cargo run
```

기본 주소는 `127.0.0.1:3001`이며 `BIND_ADDR`로 변경할 수 있습니다.

```text
BIND_ADDR=0.0.0.0:8080 cargo run
```

```text
curl http://127.0.0.1:3001/api/system
```

```text
curl http://127.0.0.1:3001/health
```

정상적인 `/health` 응답:

```json
{
  "name": "Background_System_Monitor",
  "status": "OK"
}
```

```text
curl -X POST http://127.0.0.1:3001/control/shutdown
```

`/health`는 정상일 때 위 JSON과 HTTP 200을 반환합니다. 필수 지표 수집에 실패하면 `/api/system`은 HTTP 503과 고정된 오류 JSON을 반환하고, `/health`는 `{"name":"Background_System_Monitor","status":"UNAVAILABLE"}`와 HTTP 503을 반환합니다. collector가 중단되어 3초 이상 snapshot이 갱신되지 않은 경우에도 비정상 상태로 처리합니다.

`POST /control/shutdown`은 현재 요청을 처리한 뒤 서버를 graceful shutdown합니다. 인증은 현재 제공하지 않으므로 `BIND_ADDR`로 외부에 바인딩할 경우 접근 가능한 클라이언트가 서버를 종료할 수 있습니다. 기본 loopback을 유지하거나 방화벽으로 접근을 제한해야 합니다.

기본 바인딩은 안전한 loopback 주소인 `127.0.0.1:3001`입니다. 내부망에서 원격 접근이 필요할 때만 `BIND_ADDR`를 명시하고, 서버 방화벽으로 허용된 내부 네트워크만 접근하도록 제한해야 합니다. 인증은 현재 범위에서 제공하지 않습니다.

디스크, 네트워크, 전체 프로세스 목록, 장기 히스토리는 다음 단계에서 별도 주기로 확장할 수 있도록 현재 collector와 snapshot 모델을 분리해 두었습니다.

## 검증

```text
cargo fmt --check
cargo test -- --test-threads=1
cargo clippy --all-targets --all-features -- -D warnings
```

## 릴리즈 산출물

`v*.*.*` 태그를 push하면 GitHub Actions가 다음 4개 바이너리를 빌드하고 GitHub Release에 첨부합니다.

```text
background-system-monitor-windows-x64.exe
background-system-monitor-linux-x64
background-system-monitor-macos-x64
background-system-monitor-macos-arm64
```

수동 실행 방식이므로 서비스 등록은 하지 않습니다. Windows release 바이너리는 콘솔 창이 뜨지 않는 GUI subsystem으로 빌드되며, debug 빌드와 `cargo run`은 개발 편의를 위해 콘솔을 사용합니다. 운영 환경에서는 기본 loopback을 유지하거나, 원격 내부망 접근이 필요할 때만 `BIND_ADDR`를 명시하고 방화벽에서 허용된 내부 네트워크만 열어야 합니다.
