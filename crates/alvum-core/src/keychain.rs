use anyhow::{Context, Result, bail};
use std::process::Command;

fn provider_service(provider: &str) -> String {
    format!("com.alvum.provider.{provider}")
}

pub fn provider_secret_available(provider: &str, account: &str) -> bool {
    read_provider_secret(provider, account)
        .map(|secret| secret.is_some())
        .unwrap_or(false)
}

pub fn read_provider_secret(provider: &str, account: &str) -> Result<Option<String>> {
    if std::env::var_os("ALVUM_DISABLE_KEYCHAIN").is_some() {
        return Ok(None);
    }
    read_secret(&provider_service(provider), account)
}

pub fn write_provider_secret(provider: &str, account: &str, secret: &str) -> Result<()> {
    if std::env::var_os("ALVUM_DISABLE_KEYCHAIN").is_some() {
        bail!("Keychain access is disabled for this process");
    }
    write_secret(&provider_service(provider), account, secret)
}

#[cfg(target_os = "macos")]
fn read_secret(service: &str, account: &str) -> Result<Option<String>> {
    let output = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .output()
        .with_context(|| format!("failed to read {service}/{account} from Keychain"))?;

    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string();
        return Ok(Some(value));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("could not be found")
        || stderr.contains("The specified item could not be found")
    {
        return Ok(None);
    }

    bail!(
        "Keychain read failed for {service}/{account}: {}",
        stderr.trim()
    );
}

#[cfg(not(target_os = "macos"))]
fn read_secret(_service: &str, _account: &str) -> Result<Option<String>> {
    Ok(None)
}

#[cfg(target_os = "macos")]
fn write_secret(service: &str, account: &str, secret: &str) -> Result<()> {
    if secret.is_empty() {
        return Ok(());
    }
    let output = Command::new("/usr/bin/security")
        .args([
            "add-generic-password",
            "-s",
            service,
            "-a",
            account,
            "-w",
            secret,
            "-U",
        ])
        .output()
        .with_context(|| format!("failed to write {service}/{account} to Keychain"))?;
    if output.status.success() {
        return Ok(());
    }

    bail!(
        "Keychain write failed for {service}/{account}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
}

#[cfg(not(target_os = "macos"))]
fn write_secret(_service: &str, _account: &str, _secret: &str) -> Result<()> {
    bail!("provider secrets require macOS Keychain")
}
