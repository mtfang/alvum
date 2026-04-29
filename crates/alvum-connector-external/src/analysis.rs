use crate::client::ManagedExtension;
use crate::registry::ExtensionRegistryStore;
use alvum_core::config::AlvumConfig;
use alvum_core::decision::{Decision, Edge};
use alvum_core::extension::{
    AnalysisComponent, DataScope, ExtensionManifest, ExtensionPackageRecord,
};
use alvum_core::llm::{LlmProvider, complete_observed};
use alvum_core::observation::Observation;
use alvum_knowledge::types::KnowledgeCorpus;
use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisRequest {
    pub analysis: String,
    pub date: String,
    pub output_dir: PathBuf,
    pub context_url: String,
    pub llm_url: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnalysisResponse {
    #[serde(default)]
    pub artifacts: Vec<AnalysisArtifact>,
    #[serde(default)]
    pub graph_overlays: Vec<GraphOverlay>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisArtifact {
    pub relative_path: String,
    pub content: String,
    #[serde(default)]
    pub mime: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphOverlay {
    pub id: String,
    pub content: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextQuery {
    pub scopes: Vec<DataScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextResponse {
    #[serde(default)]
    pub observations: Vec<Observation>,
    #[serde(default)]
    pub decisions: Vec<Decision>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<KnowledgeCorpus>,
    #[serde(default)]
    pub raw_files: Vec<BlobRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobRef {
    pub path: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmCompleteRequest {
    pub system: String,
    pub user_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmCompleteResponse {
    pub text: String,
}

pub async fn run_enabled_analyses(
    config: &AlvumConfig,
    date: &str,
    capture_dir: &Path,
    output_dir: &Path,
    provider: Arc<dyn LlmProvider>,
) -> Result<Vec<(String, AnalysisResponse)>> {
    let store = ExtensionRegistryStore::default();
    let mut results = Vec::new();
    for (_config_name, _record, manifest, connector_id) in
        store.configured_external_records(config)?
    {
        let Some(connector) = manifest.connectors.iter().find(|c| c.id == connector_id) else {
            continue;
        };
        for analysis_id in &connector.analyses {
            let local_id = analysis_id
                .split_once('/')
                .map(|(_, id)| id)
                .unwrap_or(analysis_id);
            let (analysis_record, analysis_manifest) =
                match store.load_component_package(analysis_id) {
                    Ok(component) => component,
                    Err(e) => {
                        tracing::warn!(
                            package = %manifest.id,
                            analysis = %analysis_id,
                            error = %e,
                            "failed to resolve extension analysis; continuing"
                        );
                        continue;
                    }
                };
            match run_analysis(
                analysis_record,
                analysis_manifest,
                local_id,
                date,
                capture_dir,
                output_dir,
                provider.clone(),
            )
            .await
            {
                Ok(response) => results.push((analysis_id.clone(), response)),
                Err(e) => tracing::warn!(
                    package = %manifest.id,
                    analysis = %analysis_id,
                    error = %e,
                    "extension analysis failed; continuing"
                ),
            }
        }
    }
    Ok(results)
}

pub async fn run_analysis(
    record: ExtensionPackageRecord,
    manifest: ExtensionManifest,
    analysis_id: &str,
    date: &str,
    capture_dir: &Path,
    output_dir: &Path,
    provider: Arc<dyn LlmProvider>,
) -> Result<AnalysisResponse> {
    let analysis = manifest
        .analyses
        .iter()
        .find(|analysis| analysis.id == analysis_id)
        .cloned()
        .with_context(|| format!("analysis {analysis_id} not found in {}", manifest.id))?;
    let broker = ContextBroker::start(
        analysis.clone(),
        capture_dir.to_path_buf(),
        output_dir.to_path_buf(),
        provider,
    )
    .await?;
    let host_url = broker.base_url.clone();
    let token = broker.token.clone();
    let manifest_for_blocking = manifest.clone();
    let record_for_blocking = record.clone();
    let analysis_name = analysis.id.clone();
    let date = date.to_string();
    let output_dir_for_request = output_dir.to_path_buf();
    let response = tokio::task::spawn_blocking(move || {
        let managed = ManagedExtension::start(
            &manifest_for_blocking,
            &record_for_blocking.package_dir,
            &extension_log_dir(),
            Some(&host_url),
        )?;
        managed.client().analyze(&AnalysisRequest {
            analysis: analysis_name,
            date,
            output_dir: output_dir_for_request,
            context_url: format!("{host_url}/v1/context/query"),
            llm_url: format!("{host_url}/v1/llm/complete"),
            token,
        })
    })
    .await??;
    write_analysis_outputs(output_dir, analysis_id, &response)?;
    Ok(response)
}

fn write_analysis_outputs(
    output_dir: &Path,
    analysis_id: &str,
    response: &AnalysisResponse,
) -> Result<()> {
    let dir = output_dir.join("extensions").join(analysis_id);
    std::fs::create_dir_all(&dir)?;
    for artifact in &response.artifacts {
        let path = dir.join(sanitize_relative_path(&artifact.relative_path)?);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, artifact.content.as_bytes())?;
    }
    if !response.graph_overlays.is_empty() {
        let path = dir.join("graph-overlays.json");
        std::fs::write(path, serde_json::to_vec_pretty(&response.graph_overlays)?)?;
    }
    Ok(())
}

fn sanitize_relative_path(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        anyhow::bail!("analysis artifact path must be relative and stay inside output dir");
    }
    Ok(path)
}

struct ContextBroker {
    base_url: String,
    token: String,
    _shutdown: Option<oneshot::Sender<()>>,
}

impl ContextBroker {
    async fn start(
        analysis: AnalysisComponent,
        capture_dir: PathBuf,
        output_dir: PathBuf,
        provider: Arc<dyn LlmProvider>,
    ) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let token = new_token(&analysis.id);
        let state = Arc::new(BrokerState {
            token: token.clone(),
            analysis,
            capture_dir,
            output_dir,
            provider,
            blobs: Mutex::new(HashMap::new()),
            base_url: format!("http://{addr}"),
        });
        let (tx, rx) = oneshot::channel();
        let app = Router::new()
            .route("/v1/context/query", post(context_query))
            .route("/v1/llm/complete", post(llm_complete))
            .route("/v1/blob/{id}", get(blob_get))
            .with_state(state);
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            token,
            _shutdown: Some(tx),
        })
    }
}

impl Drop for ContextBroker {
    fn drop(&mut self) {
        if let Some(tx) = self._shutdown.take() {
            let _ = tx.send(());
        }
    }
}

struct BrokerState {
    token: String,
    analysis: AnalysisComponent,
    capture_dir: PathBuf,
    output_dir: PathBuf,
    provider: Arc<dyn LlmProvider>,
    blobs: Mutex<HashMap<String, PathBuf>>,
    base_url: String,
}

async fn context_query(
    State(state): State<Arc<BrokerState>>,
    headers: HeaderMap,
    Json(query): Json<ContextQuery>,
) -> Result<Json<ContextResponse>, StatusCode> {
    authorize(&state, &headers)?;
    ensure_scopes(&state.analysis.scopes, &query.scopes)?;
    let mut response = ContextResponse::default();
    for scope in query.scopes {
        match scope {
            DataScope::All => {
                response.observations = read_jsonl(&state.output_dir.join("transcript.jsonl"));
                response.decisions = read_jsonl(&state.output_dir.join("decisions.jsonl"));
                response.edges = read_jsonl(&state.output_dir.join("tree").join("L4-edges.jsonl"));
                response.briefing = read_to_string(&state.output_dir.join("briefing.md"));
                response.knowledge =
                    alvum_knowledge::store::load(&state.output_dir.join("knowledge")).ok();
                response.raw_files = collect_blob_refs(&state)?;
            }
            DataScope::Observations => {
                response.observations = read_jsonl(&state.output_dir.join("transcript.jsonl"))
            }
            DataScope::Decisions => {
                response.decisions = read_jsonl(&state.output_dir.join("decisions.jsonl"))
            }
            DataScope::Edges => {
                response.edges = read_jsonl(&state.output_dir.join("tree").join("L4-edges.jsonl"))
            }
            DataScope::Briefing => {
                response.briefing = read_to_string(&state.output_dir.join("briefing.md"))
            }
            DataScope::Knowledge => {
                response.knowledge =
                    alvum_knowledge::store::load(&state.output_dir.join("knowledge")).ok()
            }
            DataScope::Capture | DataScope::RawFiles => {
                response.raw_files = collect_blob_refs(&state)?
            }
            DataScope::Threads => {}
        }
    }
    Ok(Json(response))
}

async fn llm_complete(
    State(state): State<Arc<BrokerState>>,
    headers: HeaderMap,
    Json(request): Json<LlmCompleteRequest>,
) -> Result<Json<LlmCompleteResponse>, StatusCode> {
    authorize(&state, &headers)?;
    let text = complete_observed(
        state.provider.as_ref(),
        &request.system,
        &request.user_message,
        &format!("extension/{}/analysis", state.analysis.id),
    )
    .await
    .map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok(Json(LlmCompleteResponse { text }))
}

async fn blob_get(
    State(state): State<Arc<BrokerState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, StatusCode> {
    authorize(&state, &headers)?;
    let path = state
        .blobs
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(bytes))
        .unwrap())
}

fn authorize(state: &BrokerState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let expected = format!("Bearer {}", state.token);
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(expected.as_str())
    {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn ensure_scopes(allowed: &[DataScope], requested: &[DataScope]) -> Result<(), StatusCode> {
    if allowed.contains(&DataScope::All) {
        return Ok(());
    }
    if requested.iter().all(|scope| allowed.contains(scope)) {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

fn collect_blob_refs(state: &BrokerState) -> Result<Vec<BlobRef>, StatusCode> {
    let mut refs = Vec::new();
    collect_files(&state.capture_dir, &mut |path| {
        let id = format!("blob{}", state.blobs.lock().unwrap().len() + 1);
        state
            .blobs
            .lock()
            .unwrap()
            .insert(id.clone(), path.to_path_buf());
        refs.push(BlobRef {
            path: path.display().to_string(),
            url: format!("{}/v1/blob/{id}", state.base_url),
        });
    })
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(refs)
}

fn collect_files(dir: &Path, f: &mut dyn FnMut(&Path)) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, f)?;
        } else {
            f(&path);
        }
    }
    Ok(())
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    alvum_core::storage::read_jsonl(path).unwrap_or_default()
}

fn read_to_string(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn extension_log_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".alvum")
        .join("runtime")
        .join("logs")
        .join("extensions")
}

fn new_token(id: &str) -> String {
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    format!("alvum-broker-{id}-{}-{nanos}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_absolute_artifact_paths() {
        assert!(sanitize_relative_path("/tmp/leak.md").is_err());
        assert!(sanitize_relative_path("../leak.md").is_err());
        assert_eq!(
            sanitize_relative_path("report.md").unwrap(),
            PathBuf::from("report.md")
        );
    }
}
