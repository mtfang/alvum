use alvum_core::data_ref::DataRef;
use alvum_core::extension::ExtensionManifest;
use alvum_core::observation::Observation;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ExtensionClient {
    base_url: String,
    token: String,
    http: reqwest::blocking::Client,
}

impl ExtensionClient {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
            http: reqwest::blocking::Client::new(),
        }
    }

    pub fn health(&self, path: &str) -> Result<()> {
        self.get(path)?;
        Ok(())
    }

    pub fn manifest(&self) -> Result<ExtensionManifest> {
        self.get_json("/v1/manifest")
    }

    pub fn gather(&self, request: &GatherRequest) -> Result<GatherResponse> {
        self.post_json("/v1/gather", request)
    }

    pub fn process(&self, request: &ProcessRequest) -> Result<ProcessResponse> {
        self.post_json("/v1/process", request)
    }

    pub fn capture_start(&self, request: &CaptureStartRequest) -> Result<CaptureStartResponse> {
        self.post_json("/v1/capture/start", request)
    }

    pub fn capture_stop(&self, request: &CaptureStopRequest) -> Result<()> {
        let _: serde_json::Value = self.post_json("/v1/capture/stop", request)?;
        Ok(())
    }

    pub fn analyze(
        &self,
        request: &crate::analysis::AnalysisRequest,
    ) -> Result<crate::analysis::AnalysisResponse> {
        self.post_json("/v1/analyze", request)
    }

    fn get(&self, path: &str) -> Result<reqwest::blocking::Response> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .context("extension GET failed")?;
        ensure_success(response)
    }

    fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        Ok(self.get(path)?.json()?)
    }

    fn post_json<T, B>(&self, path: &str, body: &B) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
        B: Serialize,
    {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .context("extension POST failed")?;
        Ok(ensure_success(response)?.json()?)
    }
}

fn ensure_success(response: reqwest::blocking::Response) -> Result<reqwest::blocking::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().unwrap_or_default();
    bail!("extension HTTP request failed with {status}: {body}");
}

pub struct ManagedExtension {
    client: ExtensionClient,
    child: Child,
}

impl ManagedExtension {
    pub fn start(
        manifest: &ExtensionManifest,
        package_dir: &Path,
        log_dir: &Path,
        host_url: Option<&str>,
    ) -> Result<Self> {
        if manifest.server.start.is_empty() {
            bail!("extension {} has empty server.start", manifest.id);
        }
        std::fs::create_dir_all(log_dir)?;
        let port = reserve_local_port()?;
        let token = new_token(&manifest.id);
        let base_url = format!("http://127.0.0.1:{port}");
        let data_dir = package_dir.join(".alvum-data");
        std::fs::create_dir_all(&data_dir)?;

        let log_path = log_dir.join(format!("{}.log", manifest.id));
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("failed to open {}", log_path.display()))?;
        let stderr = stdout.try_clone()?;

        let mut command = Command::new(&manifest.server.start[0]);
        command
            .args(&manifest.server.start[1..])
            .current_dir(package_dir)
            .env("ALVUM_EXTENSION_PORT", port.to_string())
            .env("ALVUM_EXTENSION_TOKEN", &token)
            .env("ALVUM_EXTENSION_ID", &manifest.id)
            .env("ALVUM_EXTENSION_DATA_DIR", &data_dir)
            .env("ALVUM_HOST_URL", host_url.unwrap_or(""))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));

        let child = command
            .spawn()
            .with_context(|| format!("failed to start extension {}", manifest.id))?;
        let managed = Self {
            client: ExtensionClient::new(base_url, token),
            child,
        };
        managed.wait_for_health(
            &manifest.server.health_path,
            manifest.server.startup_timeout_ms,
        )?;
        Ok(managed)
    }

    pub fn client(&self) -> &ExtensionClient {
        &self.client
    }

    fn wait_for_health(&self, path: &str, timeout_ms: u64) -> Result<()> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut last_error = None;
        while Instant::now() < deadline {
            match self.client.health(path) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last_error = Some(e.to_string());
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        bail!(
            "extension health check timed out at {path}: {}",
            last_error.unwrap_or_else(|| "no response".into())
        );
    }
}

impl Drop for ManagedExtension {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn reserve_local_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

fn new_token(id: &str) -> String {
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    format!("alvum-{id}-{}-{nanos}", std::process::id())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatherRequest {
    pub connector: String,
    pub capture_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GatherResponse {
    #[serde(default)]
    pub data_refs: Vec<DataRef>,
    #[serde(default)]
    pub observations: Vec<Observation>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRequest {
    pub processor: String,
    pub data_refs: Vec<DataRef>,
    pub capture_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProcessResponse {
    #[serde(default)]
    pub observations: Vec<Observation>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureStartRequest {
    pub capture: String,
    pub capture_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CaptureStartResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureStopRequest {
    pub capture: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::Arc;

    async fn health() -> &'static str {
        "ok"
    }

    async fn gather(
        State(_state): State<Arc<()>>,
        Json(_req): Json<GatherRequest>,
    ) -> Json<GatherResponse> {
        Json(GatherResponse {
            data_refs: vec![
                DataRef::new(
                    "2026-04-11T10:15:00Z".parse().unwrap(),
                    "fixture",
                    "fixture.json",
                    "application/json",
                )
                .with_routing("fixture/capture", "fixture.schema.v1"),
            ],
            observations: vec![],
            warnings: vec![],
        })
    }

    fn spawn_fixture() -> ExtensionClient {
        let token = "test-token".to_string();
        let state = Arc::new(());
        let app = Router::new()
            .route("/v1/health", get(health))
            .route("/v1/gather", post(gather))
            .with_state(state);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(listener, app).await.unwrap();
            });
        });
        ExtensionClient::new(format!("http://127.0.0.1:{port}"), token)
    }

    #[test]
    fn client_decodes_gather_response() {
        let client = spawn_fixture();
        let response = client
            .gather(&GatherRequest {
                connector: "fixture".into(),
                capture_dir: PathBuf::from("/tmp/capture"),
            })
            .unwrap();

        assert_eq!(response.data_refs.len(), 1);
        assert_eq!(response.data_refs[0].producer, "fixture/capture");
    }
}
