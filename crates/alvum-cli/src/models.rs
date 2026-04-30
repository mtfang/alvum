use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Subcommand)]
pub(crate) enum Action {
    /// Install a locally managed model asset.
    Install {
        /// Model family to install. Currently supports `whisper`.
        model: String,
        /// Whisper.cpp model variant.
        #[arg(long, default_value = "base.en")]
        variant: String,
    },
}

pub(crate) async fn run(action: Action) -> Result<()> {
    match action {
        Action::Install { model, variant } => install(&model, &variant).await,
    }
}

const MODEL_INSTALL_TIMEOUT: Duration = Duration::from_secs(60 * 60);

#[derive(serde::Serialize)]
struct LocalModelInstallReport {
    ok: bool,
    model: String,
    variant: String,
    status: String,
    path: Option<String>,
    bytes: Option<u64>,
    error: Option<String>,
}

fn valid_model_variant(variant: &str) -> bool {
    let variant = variant.trim();
    !variant.is_empty()
        && variant.len() <= 64
        && !variant.starts_with('-')
        && variant
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

fn whisper_model_path(variant: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not resolve $HOME for model install")?;
    Ok(home
        .join(".alvum")
        .join("runtime")
        .join("models")
        .join(format!("ggml-{variant}.bin")))
}

async fn install(model: &str, variant: &str) -> Result<()> {
    if model != "whisper" {
        println!(
            "{}",
            serde_json::to_string_pretty(&LocalModelInstallReport {
                ok: false,
                model: model.into(),
                variant: variant.into(),
                status: "unsupported_model".into(),
                path: None,
                bytes: None,
                error: Some("only the whisper model family is supported".into()),
            })?
        );
        return Ok(());
    }
    if !valid_model_variant(variant) {
        println!(
            "{}",
            serde_json::to_string_pretty(&LocalModelInstallReport {
                ok: false,
                model: model.into(),
                variant: variant.into(),
                status: "invalid_variant".into(),
                path: None,
                bytes: None,
                error: Some("model variants may only contain letters, numbers, ., _, and -".into()),
            })?
        );
        return Ok(());
    }

    let path = whisper_model_path(variant)?;
    if path.exists() {
        let bytes = std::fs::metadata(&path).ok().map(|meta| meta.len());
        println!(
            "{}",
            serde_json::to_string_pretty(&LocalModelInstallReport {
                ok: true,
                model: model.into(),
                variant: variant.into(),
                status: "present".into(),
                path: Some(path.display().to_string()),
                bytes,
                error: None,
            })?
        );
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp_path = path.with_extension(format!("bin.tmp-{}", std::process::id()));
    let url =
        format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{variant}.bin");
    let client = reqwest::Client::builder()
        .timeout(MODEL_INSTALL_TIMEOUT)
        .build()?;
    let result = async {
        let bytes = client
            .get(url)
            .send()
            .await
            .context("failed to download Whisper model")?
            .error_for_status()
            .context("Whisper model download failed")?
            .bytes()
            .await
            .context("failed to read Whisper model bytes")?;
        tokio::fs::write(&tmp_path, &bytes).await?;
        tokio::fs::rename(&tmp_path, &path).await?;
        Ok::<u64, anyhow::Error>(bytes.len() as u64)
    }
    .await;

    let _ = tokio::fs::remove_file(&tmp_path).await;
    let (ok, status, bytes, error) = match result {
        Ok(bytes) => (true, "installed".to_string(), Some(bytes), None),
        Err(e) => (false, "failed".to_string(), None, Some(format!("{e:#}"))),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&LocalModelInstallReport {
            ok,
            model: model.into(),
            variant: variant.into(),
            status,
            path: Some(path.display().to_string()),
            bytes,
            error,
        })?
    );
    Ok(())
}
