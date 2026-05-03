use alvum_core::config::{AlvumConfig, ProcessorConfig};
use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;
use tokio::process::Command as TokioCommand;

#[derive(Subcommand)]
pub(crate) enum Action {
    /// Install a locally managed model asset.
    Install {
        /// Model family to install. Supports `whisper` and `pyannote`.
        model: String,
        /// Model variant.
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
const PYANNOTE_VARIANT: &str = "community-1";
const PYANNOTE_DEFAULT_PIPELINE: &str = "pyannote/speaker-diarization-community-1";
const PYANNOTE_PACKAGE: &str = "pyannote.audio>=4,<5";
const PYANNOTE_SKIP_PIP_ENV: &str = "ALVUM_PYANNOTE_INSTALL_SKIP_PIP";

#[derive(serde::Serialize)]
struct LocalModelInstallReport {
    ok: bool,
    model: String,
    variant: String,
    status: String,
    path: Option<String>,
    command_path: Option<String>,
    config_key: Option<String>,
    bytes: Option<u64>,
    detail: Option<String>,
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
    match model {
        "whisper" => install_whisper(model, variant).await,
        "pyannote" => install_pyannote(model, variant).await,
        _ => {
            println!(
                "{}",
                serde_json::to_string_pretty(&LocalModelInstallReport {
                    ok: false,
                    model: model.into(),
                    variant: variant.into(),
                    status: "unsupported_model".into(),
                    path: None,
                    command_path: None,
                    config_key: None,
                    bytes: None,
                    detail: None,
                    error: Some("supported model families are whisper and pyannote".into()),
                })?
            );
            Ok(())
        }
    }
}

async fn install_whisper(model: &str, variant: &str) -> Result<()> {
    if model != "whisper" {
        println!(
            "{}",
            serde_json::to_string_pretty(&LocalModelInstallReport {
                ok: false,
                model: model.into(),
                variant: variant.into(),
                status: "unsupported_model".into(),
                path: None,
                command_path: None,
                config_key: None,
                bytes: None,
                detail: None,
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
                command_path: None,
                config_key: None,
                bytes: None,
                detail: None,
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
                command_path: None,
                config_key: None,
                bytes,
                detail: None,
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
            command_path: None,
            config_key: None,
            bytes,
            detail: None,
            error,
        })?
    );
    Ok(())
}

struct PyannoteInstallPaths {
    root: PathBuf,
    venv_python: PathBuf,
    runner_path: PathBuf,
    command_path: PathBuf,
}

async fn install_pyannote(model: &str, variant: &str) -> Result<()> {
    let Some(variant) = normalize_pyannote_variant(variant) else {
        println!(
            "{}",
            serde_json::to_string_pretty(&LocalModelInstallReport {
                ok: false,
                model: model.into(),
                variant: variant.into(),
                status: "invalid_variant".into(),
                path: None,
                command_path: None,
                config_key: None,
                bytes: None,
                detail: None,
                error: Some("pyannote currently supports the community-1 variant".into()),
            })?
        );
        return Ok(());
    };

    let paths = pyannote_install_paths()?;
    let skip_pip = skip_pyannote_install_steps();
    let pipeline_model = pyannote_pipeline_model();
    if pyannote_requires_hf_access(&pipeline_model) && !pyannote_hf_token_available()? {
        println!(
            "{}",
            serde_json::to_string_pretty(&LocalModelInstallReport {
                ok: false,
                model: model.into(),
                variant: variant.into(),
                status: "requires_huggingface_access".into(),
                path: None,
                command_path: None,
                config_key: None,
                bytes: None,
                detail: Some(pyannote_access_detail().into()),
                error: Some(pyannote_access_detail().into()),
            })?
        );
        return Ok(());
    }
    let already_installed = !skip_pip
        && paths.command_path.exists()
        && paths.runner_path.exists()
        && paths.venv_python.exists();

    let result = async {
        tokio::fs::create_dir_all(paths.command_path.parent().unwrap()).await?;
        if !skip_pip {
            if !already_installed {
                install_pyannote_environment(&paths).await?;
            }
            preflight_pyannote_model(&paths).await?;
        }
        write_pyannote_runner(&paths).await?;
        configure_pyannote_command(&paths.command_path)?;
        Ok::<(), anyhow::Error>(())
    }
    .await;

    let (ok, status, detail, error) = match result {
        Ok(()) => (
            true,
            if already_installed {
                "present"
            } else {
                "installed"
            }
            .to_string(),
            Some(if already_installed {
                "Pyannote command is installed, validated, and configured for local audio diarization."
                    .into()
            } else {
                "Pyannote is installed under ~/.alvum/runtime/pyannote and configured for local audio diarization."
                    .into()
            }),
            None,
        ),
        Err(err) => {
            let status = classify_pyannote_install_error(&err);
            (
                false,
                status.into(),
                pyannote_install_failure_detail(status).map(str::to_string),
                Some(pyannote_install_error_message(&err, status)),
            )
        }
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&LocalModelInstallReport {
            ok,
            model: model.into(),
            variant: variant.into(),
            status,
            path: Some(paths.root.display().to_string()),
            command_path: Some(paths.command_path.display().to_string()),
            config_key: Some("processors.audio.pyannote_command".into()),
            bytes: None,
            detail,
            error,
        })?
    );
    Ok(())
}

fn normalize_pyannote_variant(variant: &str) -> Option<&'static str> {
    match variant.trim() {
        "" | "base.en" | PYANNOTE_VARIANT => Some(PYANNOTE_VARIANT),
        _ => None,
    }
}

fn pyannote_install_paths() -> Result<PyannoteInstallPaths> {
    let home = dirs::home_dir().context("could not resolve $HOME for pyannote install")?;
    let root = home.join(".alvum/runtime/pyannote");
    let venv_python = root.join("venv/bin/python");
    let runner_path = root.join("alvum_pyannote.py");
    let command_path = root.join("bin/alvum-pyannote");
    Ok(PyannoteInstallPaths {
        root,
        venv_python,
        runner_path,
        command_path,
    })
}

fn skip_pyannote_install_steps() -> bool {
    std::env::var(PYANNOTE_SKIP_PIP_ENV)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn pyannote_pipeline_model() -> String {
    std::env::var("ALVUM_PYANNOTE_PIPELINE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| PYANNOTE_DEFAULT_PIPELINE.into())
}

fn pyannote_requires_hf_access(model: &str) -> bool {
    model.trim().starts_with("pyannote/")
}

fn pyannote_hf_token_available() -> Result<bool> {
    if [
        "HF_TOKEN",
        "HUGGING_FACE_HUB_TOKEN",
        "HUGGINGFACE_HUB_TOKEN",
    ]
    .iter()
    .any(|name| {
        std::env::var(name)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    }) {
        return Ok(true);
    }
    if pyannote_configured_hf_token()?.is_some() {
        return Ok(true);
    }
    let Some(home) = dirs::home_dir() else {
        return Ok(false);
    };
    Ok([
        home.join(".cache/huggingface/token"),
        home.join(".huggingface/token"),
    ]
    .iter()
    .any(|path| path.metadata().map(|meta| meta.len() > 0).unwrap_or(false)))
}

fn pyannote_configured_hf_token() -> Result<Option<String>> {
    Ok(AlvumConfig::load()
        .ok()
        .and_then(|config| config.processor_setting("audio", "pyannote_hf_token"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

fn pyannote_access_detail() -> &'static str {
    "Pyannote Community-1 requires Hugging Face access. Accept the model terms at https://huggingface.co/pyannote/speaker-diarization-community-1, then sign in with Hugging Face or set HF_TOKEN and run install again."
}

async fn install_pyannote_environment(paths: &PyannoteInstallPaths) -> Result<()> {
    let mut venv = TokioCommand::new("python3");
    venv.arg("-m").arg("venv").arg(paths.root.join("venv"));
    run_install_command(venv, "create pyannote virtualenv", MODEL_INSTALL_TIMEOUT).await?;

    let mut pip_upgrade = TokioCommand::new(&paths.venv_python);
    pip_upgrade
        .arg("-m")
        .arg("pip")
        .arg("install")
        .arg("--upgrade")
        .arg("pip");
    run_install_command(
        pip_upgrade,
        "upgrade pyannote installer pip",
        MODEL_INSTALL_TIMEOUT,
    )
    .await?;

    let mut pip_install = TokioCommand::new(&paths.venv_python);
    pip_install
        .arg("-m")
        .arg("pip")
        .arg("install")
        .arg(PYANNOTE_PACKAGE);
    run_install_command(pip_install, "install pyannote.audio", MODEL_INSTALL_TIMEOUT).await
}

async fn preflight_pyannote_model(paths: &PyannoteInstallPaths) -> Result<()> {
    let mut preflight = TokioCommand::new(&paths.venv_python);
    preflight.arg("-c").arg(pyannote_preflight_script());
    run_install_command(
        preflight,
        "load pyannote diarization model",
        MODEL_INSTALL_TIMEOUT,
    )
    .await
}

async fn write_pyannote_runner(paths: &PyannoteInstallPaths) -> Result<()> {
    tokio::fs::write(&paths.runner_path, pyannote_runner_script()).await?;
    tokio::fs::write(&paths.command_path, pyannote_wrapper_script(paths)).await?;
    make_executable(&paths.command_path)?;
    Ok(())
}

fn configure_pyannote_command(command_path: &Path) -> Result<()> {
    let mut config = AlvumConfig::load()?;
    let audio = config
        .processors
        .entry("audio".into())
        .or_insert_with(|| ProcessorConfig {
            settings: std::collections::HashMap::new(),
        });
    audio.settings.insert(
        "diarization_enabled".into(),
        toml::Value::String("true".into()),
    );
    audio.settings.insert(
        "diarization_model".into(),
        toml::Value::String("pyannote-local".into()),
    );
    audio.settings.insert(
        "pyannote_command".into(),
        toml::Value::String(command_path.display().to_string()),
    );
    config.save()
}

async fn run_install_command(
    mut command: TokioCommand,
    label: &str,
    timeout: Duration,
) -> Result<()> {
    command.kill_on_drop(true);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .with_context(|| format!("{label} timed out"))?
        .with_context(|| format!("{label} failed to start"))?;
    if !output.status.success() {
        bail!("{label} failed: {}", command_output_tail(&output));
    }
    Ok(())
}

fn command_output_tail(output: &Output) -> String {
    let mut text = String::new();
    if !output.stdout.is_empty() {
        text.push_str(&String::from_utf8_lossy(&output.stdout));
    }
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    let text = text.trim();
    if text.len() <= 1600 {
        return text.to_string();
    }
    let mut tail = text.chars().rev().take(1600).collect::<Vec<_>>();
    tail.reverse();
    tail.into_iter().collect()
}

fn classify_pyannote_install_error(err: &anyhow::Error) -> &'static str {
    let message = format!("{err:#}").to_lowercase();
    if message.contains("401")
        || message.contains("403")
        || message.contains("gated")
        || message.contains("requires hugging face access")
        || message.contains("hugging face")
        || message.contains("huggingface")
        || message.contains("token")
    {
        "requires_huggingface_access"
    } else if message.contains("python3") || message.contains("venv") {
        "requires_python"
    } else {
        "failed"
    }
}

fn pyannote_install_failure_detail(status: &str) -> Option<&'static str> {
    match status {
        "requires_huggingface_access" => Some(pyannote_access_detail()),
        _ => None,
    }
}

fn pyannote_install_error_message(err: &anyhow::Error, status: &str) -> String {
    if status == "requires_huggingface_access" {
        pyannote_access_detail().into()
    } else {
        format!("{err:#}")
    }
}

fn pyannote_preflight_script() -> &'static str {
    r#"
import os
import sys
from pyannote.audio import Pipeline

ACCESS_MESSAGE = "Pyannote Community-1 requires Hugging Face access. Accept the model terms at https://huggingface.co/pyannote/speaker-diarization-community-1, then sign in with Hugging Face or set HF_TOKEN and run install again."

def cached_hf_token_exists():
    home = os.path.expanduser("~")
    for path in (
        os.path.join(home, ".cache", "huggingface", "token"),
        os.path.join(home, ".huggingface", "token"),
    ):
        if os.path.exists(path) and os.path.getsize(path) > 0:
            return True
    return False

def configured_hf_token():
    try:
        import tomllib
        with open(os.path.expanduser("~/.alvum/runtime/config.toml"), "rb") as fh:
            config = tomllib.load(fh)
        token = (((config.get("processors") or {}).get("audio") or {}).get("pyannote_hf_token"))
        if isinstance(token, str) and token.strip():
            return token.strip()
    except Exception:
        return None
    return None

def auth_token(model):
    token = os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN") or os.environ.get("HUGGINGFACE_HUB_TOKEN") or configured_hf_token()
    if token:
        return token
    if cached_hf_token_exists():
        return True
    if model.startswith("pyannote/"):
        print(ACCESS_MESSAGE, file=sys.stderr)
        raise SystemExit(42)
    return None

def load_pipeline(model):
    token = auth_token(model)
    kwargs = {"token": token} if token is not None else {}
    try:
        return Pipeline.from_pretrained(model, **kwargs)
    except TypeError:
        kwargs = {"use_auth_token": token} if token is not None else {}
        return Pipeline.from_pretrained(model, **kwargs)

model = os.environ.get("ALVUM_PYANNOTE_PIPELINE", "pyannote/speaker-diarization-community-1")
try:
    load_pipeline(model)
except SystemExit:
    raise
except Exception as exc:
    print(f"Pyannote model load failed: {exc}", file=sys.stderr)
    raise
"#
}

fn pyannote_runner_script() -> &'static str {
    r#"#!/usr/bin/env python3
import json
import os
import sys
from pyannote.audio import Pipeline

ACCESS_MESSAGE = "Pyannote Community-1 requires Hugging Face access. Accept the model terms at https://huggingface.co/pyannote/speaker-diarization-community-1, then sign in with Hugging Face or set HF_TOKEN and run install again."

def cached_hf_token_exists():
    home = os.path.expanduser("~")
    for path in (
        os.path.join(home, ".cache", "huggingface", "token"),
        os.path.join(home, ".huggingface", "token"),
    ):
        if os.path.exists(path) and os.path.getsize(path) > 0:
            return True
    return False

def configured_hf_token():
    try:
        import tomllib
        with open(os.path.expanduser("~/.alvum/runtime/config.toml"), "rb") as fh:
            config = tomllib.load(fh)
        token = (((config.get("processors") or {}).get("audio") or {}).get("pyannote_hf_token"))
        if isinstance(token, str) and token.strip():
            return token.strip()
    except Exception:
        return None
    return None

def auth_token(model):
    token = os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN") or os.environ.get("HUGGINGFACE_HUB_TOKEN") or configured_hf_token()
    if token:
        return token
    if cached_hf_token_exists():
        return True
    if model.startswith("pyannote/"):
        print(ACCESS_MESSAGE, file=sys.stderr)
        raise SystemExit(42)
    return None

def load_pipeline(model):
    token = auth_token(model)
    kwargs = {"token": token} if token is not None else {}
    try:
        return Pipeline.from_pretrained(model, **kwargs)
    except TypeError:
        kwargs = {"use_auth_token": token} if token is not None else {}
        return Pipeline.from_pretrained(model, **kwargs)

def iter_turns(annotation):
    if hasattr(annotation, "itertracks"):
        for turn, _, speaker in annotation.itertracks(yield_label=True):
            yield turn, speaker
        return
    for item in annotation:
        if len(item) == 2:
            turn, speaker = item
            yield turn, speaker

def main():
    if len(sys.argv) < 2:
        raise SystemExit("usage: alvum-pyannote AUDIO_FILE")
    audio_path = sys.argv[1]
    model = os.environ.get("ALVUM_PYANNOTE_PIPELINE", "pyannote/speaker-diarization-community-1")
    output = load_pipeline(model)(audio_path)
    annotation = (
        getattr(output, "exclusive_speaker_diarization", None)
        or getattr(output, "speaker_diarization", None)
        or output
    )
    turns = []
    for turn, speaker in iter_turns(annotation):
        turns.append({
            "start": float(turn.start),
            "end": float(turn.end),
            "speaker": str(speaker),
        })
    print(json.dumps({"source": "pyannote", "model": model, "turns": turns}, separators=(",", ":")))

if __name__ == "__main__":
    main()
"#
}

fn pyannote_wrapper_script(paths: &PyannoteInstallPaths) -> String {
    format!(
        "#!/bin/sh\nset -eu\nexec {} {} \"$@\"\n",
        shell_quote_path(&paths.venv_python),
        shell_quote_path(&paths.runner_path)
    )
}

fn shell_quote_path(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}
