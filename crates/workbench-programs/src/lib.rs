//! Resumable SHA-512-verified archive downloads, system `tar` unpacking, and bounded CLI probing.
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha512};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use workbench_foundation::js;
use workbench_foundation::sync::locked;
use workbench_telemetry::network::{SendNoted, client_builder};

/// Attempts at a download before returning an error and waiting for the caller to retry.
const DOWNLOAD_ATTEMPTS: u32 = 5;
/// A connection that sends nothing for this long is treated as dropped, and the download resumes.
const DOWNLOAD_STALL: Duration = Duration::from_secs(60);

/// A download in progress, or how the last one ended.
#[derive(Clone, Debug)]
pub struct Progress {
    pub state: &'static str,
    pub received: u64,
    pub total: u64,
    pub error: Option<String>,
}

/// A program's answer to a question about itself.
#[derive(Clone, Debug)]
pub struct Answer {
    /// None when it could not start or ran out of time.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// This computer as npm platform packages name it (e.g. `darwin-arm64`, `win32-x64`, `linux-x64`).
pub fn platform() -> String {
    let system = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let architecture = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    format!("{system}-{architecture}")
}

/// Runs a program and waits for its answer. Dropping the future ends it.
pub async fn ask_program(
    file: &Path,
    args: &[&str],
    configure: impl FnOnce(&mut Command),
    timeout: Duration,
) -> Answer {
    let mut command = Command::new(file);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure(&mut command);
    workbench_sandbox::no_window(&mut command);
    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => Answer {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
        Ok(Err(error)) => Answer {
            code: None,
            stdout: String::new(),
            stderr: error.to_string(),
        },
        Err(_) => Answer {
            code: None,
            stdout: String::new(),
            stderr: "it did not answer in time".to_owned(),
        },
    }
}

/// Marks a file executable on Unix platforms; no-op on Windows.
#[cfg(unix)]
pub async fn make_executable(file: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).await
}

/// Marks a file executable on Unix platforms; no-op on Windows.
#[cfg(not(unix))]
pub async fn make_executable(_file: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Downloads `url` into `archive`, resuming after dropped connections via HTTP Range requests.
pub async fn fetch_resumable(
    url: &str,
    expected_size: u64,
    archive: &Path,
    progress: &Arc<Mutex<Progress>>,
    mut on_retry: impl FnMut(&str),
) -> Result<(), String> {
    let client = client_builder()
        .read_timeout(DOWNLOAD_STALL)
        .build()
        .map_err(|error| error.to_string())?;
    let mut last_error = String::new();
    for attempt in 1..=DOWNLOAD_ATTEMPTS {
        let have = tokio::fs::metadata(archive)
            .await
            .map_or(0, |meta| meta.len());
        if expected_size > 0 && have >= expected_size {
            return Ok(());
        }
        match fetch_once(&client, url, expected_size, archive, progress, have).await {
            Ok(true) => return Ok(()),
            // Too much was kept to resume from: start over, without counting it as a wait.
            Ok(false) => last_error = "the connection closed early".to_owned(),
            Err(error) => last_error = error,
        }
        if attempt < DOWNLOAD_ATTEMPTS {
            on_retry(&last_error);
            tokio::time::sleep(Duration::from_secs(2 * u64::from(attempt))).await;
        }
    }
    Err(format!(
        "the download kept failing ({last_error}). What arrived is kept: press Download again to resume."
    ))
}

/// One request for the rest of the file. True when the whole file arrived.
async fn fetch_once(
    client: &reqwest::Client,
    url: &str,
    expected_size: u64,
    archive: &Path,
    progress: &Arc<Mutex<Progress>>,
    have: u64,
) -> Result<bool, String> {
    let describe = |error: std::io::Error| error.to_string();
    let mut request = client.get(url);
    if have > 0 {
        request = request.header("range", format!("bytes={have}-"));
    }
    let mut response = request
        .send_noted()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    if status == 416 {
        let _ = tokio::fs::remove_file(archive).await;
        return Ok(false);
    }
    if !response.status().is_success() {
        return Err(format!("the registry answered {status}"));
    }
    let resuming = have > 0 && status == 206;
    let start = if resuming { have } else { 0 };
    let length = response.content_length().unwrap_or(0);
    {
        let mut shown = locked(progress);
        shown.total = if length > 0 {
            start + length
        } else {
            expected_size
        };
        shown.received = start;
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true);
    if resuming {
        options.append(true);
    } else {
        options.write(true).truncate(true);
    }
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(archive).await.map_err(describe)?;
    let mut failed = None;
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                file.write_all(&chunk).await.map_err(describe)?;
                locked(progress).received += chunk.len() as u64;
            }
            Ok(None) => break,
            // What arrived stays, for the next attempt to resume from.
            Err(error) => {
                failed = Some(error.to_string());
                break;
            }
        }
    }
    file.flush().await.map_err(describe)?;
    if let Some(error) = failed {
        return Err(error);
    }
    let shown = locked(progress).clone();
    Ok(shown.total == 0 || shown.received >= shown.total)
}

/// A file's SHA-512 encoded as standard base64, matching npm's SRI integrity format.
pub async fn sha512_base64(file: &Path) -> std::io::Result<String> {
    let mut hash = Sha512::new();
    let mut source = tokio::fs::File::open(file).await?;
    let mut chunk = vec![0_u8; 256 * 1024];
    loop {
        let read = source.read(&mut chunk).await?;
        if read == 0 {
            return Ok(STANDARD.encode(hash.finalize()));
        }
        hash.update(&chunk[..read]);
    }
}

/// Unpacks a `.tgz` (or `.zip` via bsdtar on Windows) with the system's `tar`.
pub async fn untar(archive: &Path, destination: &Path) -> Result<(), String> {
    let tar = if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        PathBuf::from(root).join("System32").join("tar.exe")
    } else {
        PathBuf::from("tar")
    };
    let archive = archive.to_string_lossy();
    let destination = destination.to_string_lossy();
    let answer = ask_program(
        &tar,
        &["-xzf", &archive, "-C", &destination],
        |_| {},
        Duration::from_secs(600),
    )
    .await;
    if answer.code == Some(0) {
        return Ok(());
    }
    let detail = js::trim(&answer.stderr);
    Err(format!(
        "tar could not unpack the package: {}",
        if detail.is_empty() {
            format!("exit {}", answer.code.unwrap_or(-1))
        } else {
            detail.to_owned()
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sha512_matches_known_digest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("sample.txt");
        tokio::fs::write(&file, b"hello").await.expect("write");
        let digest = sha512_base64(&file).await.expect("hash");
        assert_eq!(
            digest,
            "m3HSJL1i83hdltRq0+o9czGb+8KJDKra4t/3JRlnPKcjI8PZm6XBHXx6zG4UuMXaDEZjR1wuXDre9G9zvN7AQw=="
        );
    }
}
