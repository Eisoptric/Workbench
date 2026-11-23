//! The programs the app downloads rather than ships: Claude Code and Codex, the official programs behind the providers of
//! those names, and uv, which makes Python environments and can install Python itself. The app finds one on the system, or
//! downloads the exact package its tests pinned (the npm registry for the two programs, uv's GitHub release for uv), checks
//! it against the pinned sha512 and unpacks it into `<data>/tools`. Settings shows a download button for a program until
//! one is found, and the sign-in once it is; Environment does the same for uv.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use tokio::sync::{Notify, oneshot};
pub(crate) use workbench_programs::{Answer, Progress, ask_program, platform};
use workbench_programs::{fetch_resumable, make_executable, sha512_base64, untar};

use crate::agent_sessions::SkillsBridge;
use crate::records::text;
use crate::{CoreError, Kernel, locked, log};

/// The packages the app downloads, by program and platform. Written by
/// `scripts/agent-programs-manifest.mjs`.
const PINNED_PACKAGES: &str = include_str!("agent-programs.json");
/// How long a program gets to answer a question about itself.
const QUESTION_TIMEOUT: Duration = Duration::from_secs(20);

/// One sign-in being carried out by `codex login`.
pub(crate) struct SignIn {
    pub(crate) info: Arc<Mutex<Value>>,
    pub(crate) cancel: Arc<Notify>,
}

/// What the runtime keeps for the two programs while it runs.
#[derive(Default)]
pub struct ProgramState {
    installs: Mutex<HashMap<String, Arc<Mutex<Progress>>>>,
    versions: Mutex<HashMap<String, Option<String>>>,
    /// Approvals waiting on the user, by `run:action`.
    pub(crate) pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    /// The app skills endpoints open for running Codex turns, by the token each turn holds.
    pub(crate) bridges: Mutex<HashMap<String, Arc<SkillsBridge>>>,
    pub(crate) sign_in: Mutex<Option<SignIn>>,
    pub(crate) last_sign_in: Mutex<Option<Value>>,
    /// Programs running for a turn or a sign-in, so they end with the runtime.
    pub(crate) running: Mutex<HashSet<u32>>,
}

/// The programs, in the order the list gives them.
const PROGRAMS: [&str; 3] = ["claude-code", "codex", "uv"];

/// One of the programs by id, or the 404 for anything else.
pub fn program_id(value: &str) -> Result<&'static str, CoreError> {
    PROGRAMS
        .iter()
        .copied()
        .find(|id| *id == value)
        .ok_or_else(|| CoreError::new(404, "Unknown program; use claude-code, codex or uv"))
}

fn label(id: &str) -> &'static str {
    match id {
        "codex" => "Codex",
        "uv" => "uv",
        _ => "Claude Code",
    }
}

fn executable(id: &str) -> String {
    let name = match id {
        "codex" => "codex",
        "uv" => "uv",
        _ => "claude",
    };
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// The program inside an unpacked platform package: claude at its root, codex under vendor/<target>/bin, and uv at the
/// root (the Windows zip) or in the one folder its tar.gz holds (`uv-<target>/uv`).
fn binary_in(id: &str, package: &Path) -> Option<PathBuf> {
    let folders = match id {
        "codex" => std::fs::read_dir(package.join("vendor"))
            .ok()?
            .flatten()
            .map(|entry| entry.path().join("bin"))
            .collect(),
        "uv" => std::iter::once(package.to_path_buf())
            .chain(
                std::fs::read_dir(package)
                    .ok()?
                    .flatten()
                    .map(|entry| entry.path()),
            )
            .collect(),
        _ => vec![package.to_path_buf()],
    };
    folders
        .into_iter()
        .map(|folder| folder.join(executable(id)))
        .find(|file| file.is_file())
}

impl Kernel {
    fn program_root(&self, id: &str) -> PathBuf {
        self.store.root().join("tools").join(id)
    }

    /// The package to download on this computer; WORKBENCH_PROGRAM_PACKAGES points tests at their own pins.
    fn program_package(&self, id: &str) -> Option<Value> {
        let packages: Value = match std::env::var_os("WORKBENCH_PROGRAM_PACKAGES") {
            Some(file) => std::fs::read(file)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(Value::Null),
            None => serde_json::from_str(PINNED_PACKAGES).unwrap_or(Value::Null),
        };
        packages.get(id)?.get(platform()).cloned()
    }

    /// The program on the user's PATH. On Windows only a real .exe counts: npm's .cmd shims cannot start without a shell.
    fn system_program(id: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|folder| folder.join(executable(id)))
            .find(|file| file.is_file())
    }

    /// The app's own download first (the tested version), then the system's.
    pub(crate) fn resolve_program(&self, id: &str) -> Option<(PathBuf, &'static str)> {
        let workbench = self.program_package(id).and_then(|item| {
            binary_in(
                id,
                &self
                    .program_root(id)
                    .join(text(&item, "version"))
                    .join("package"),
            )
        });
        workbench
            .map(|file| (file, "workbench"))
            .or_else(|| Self::system_program(id).map(|file| (file, "system")))
    }

    /// The program to run, or a 409 that says where to get it.
    pub(crate) fn program_path(&self, id: &str) -> Result<PathBuf, CoreError> {
        self.resolve_program(id)
            .map(|(file, _)| file)
            .ok_or_else(|| {
                let name = label(id);
                CoreError::new(
                    409,
                    format!("{name} is not installed. Download it in Settings → General → {name}."),
                )
            })
    }

    /// What the program itself reports (`--version`), once per copy of it.
    async fn program_version(&self, file: &Path) -> Option<String> {
        let stamp = std::fs::metadata(file)
            .and_then(|meta| meta.modified())
            .ok()?
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let key = format!("{}:{stamp}", file.display());
        if let Some(known) = locked(&self.programs.versions).get(&key) {
            return known.clone();
        }
        let answer = ask_program(file, &["--version"], |_| {}, Duration::from_secs(15)).await;
        let version = (answer.code == Some(0))
            .then(|| {
                answer
                    .stdout
                    .lines()
                    .next()
                    .map(|line| line.trim().to_owned())
            })
            .flatten()
            .filter(|line| !line.is_empty());
        locked(&self.programs.versions).insert(key, version.clone());
        version
    }

    /// Whether a program is installed, where from, and what a download would bring.
    pub async fn program_status(&self, id: &str) -> Value {
        let found = self.resolve_program(id);
        let item = self.program_package(id);
        // A snapshot: the download keeps running while the version is asked for below.
        let current = locked(&self.programs.installs)
            .get(id)
            .map(|progress| locked(progress).clone());
        let version = match &found {
            Some((file, _)) => self.program_version(file).await,
            None => None,
        };
        let mut status = json!({
            "id": id,
            "label": label(id),
            "installed": found.is_some(),
            "source": found.as_ref().map(|(_, source)| *source),
            "path": found.as_ref().map(|(file, _)| file.to_string_lossy()),
            "version": version,
            "downloadVersion": item.as_ref().map(|item| text(item, "version")),
            "downloadSize": item.as_ref().and_then(|item| item.get("size")).cloned().unwrap_or_else(|| json!(0)),
        });
        if let Some(progress) = current.filter(|progress| progress.state != "done") {
            let mut install = json!({
                "state": progress.state,
                "received": progress.received,
                "total": progress.total,
            });
            if let Some(error) = progress.error {
                install["error"] = json!(error);
            }
            status["install"] = install;
        }
        status
    }

    /// Every program.
    pub async fn program_statuses(&self) -> Value {
        let mut statuses = Vec::with_capacity(PROGRAMS.len());
        for id in PROGRAMS {
            statuses.push(self.program_status(id).await);
        }
        Value::Array(statuses)
    }

    /// Starts the download in the background; the status reports its progress.
    pub async fn install_program(self: &Arc<Self>, id: &'static str) -> Result<Value, CoreError> {
        let downloading = locked(&self.programs.installs)
            .get(id)
            .is_some_and(|progress| {
                matches!(
                    locked(progress).state,
                    "downloading" | "verifying" | "unpacking"
                )
            });
        if downloading {
            return Ok(self.program_status(id).await);
        }
        let Some(item) = self.program_package(id) else {
            return Err(CoreError::new(
                409,
                format!("{} has no download for {}.", label(id), platform()),
            ));
        };
        let progress = Arc::new(Mutex::new(Progress {
            state: "downloading",
            received: 0,
            total: item.get("size").and_then(Value::as_u64).unwrap_or(0),
            error: None,
        }));
        locked(&self.programs.installs).insert(id.to_owned(), Arc::clone(&progress));
        let kernel = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = kernel.download_program(id, &item, &progress).await {
                log(
                    "admin",
                    "warning",
                    &format!("Downloading {} failed: {error}", label(id)),
                );
                let mut progress = locked(&progress);
                progress.state = "failed";
                progress.error = Some(error);
            }
        });
        Ok(self.program_status(id).await)
    }

    async fn download_program(
        &self,
        id: &str,
        item: &Value,
        progress: &Arc<Mutex<Progress>>,
    ) -> Result<(), String> {
        let describe = |error: std::io::Error| error.to_string();
        let root = self.program_root(id);
        tokio::fs::create_dir_all(&root).await.map_err(describe)?;
        let version = text(item, "version");
        let archive = root.join(format!("{version}.tgz.part"));
        let unpacking = root.join(format!("{version}.unpacking"));
        let target = root.join(version);
        log(
            "admin",
            "info",
            &format!("Downloading {} {version}", label(id)),
        );
        let size = item.get("size").and_then(Value::as_u64).unwrap_or(0);
        fetch_resumable(text(item, "url"), size, &archive, progress, |last_error| {
            log(
                "admin",
                "warning",
                &format!("A download was interrupted ({last_error}); resuming"),
            );
        })
        .await?;

        locked(progress).state = "verifying";
        let expected = text(item, "integrity").trim_start_matches("sha512-");
        if sha512_base64(&archive).await.map_err(describe)? != expected {
            let _ = tokio::fs::remove_file(&archive).await;
            return Err("the download does not match the checksum the app pinned for it, so it was discarded".to_owned());
        }

        locked(progress).state = "unpacking";
        let unpacked = unpack(id, &archive, &unpacking, &target, &root, version).await;
        let _ = tokio::fs::remove_file(&archive).await;
        let _ = tokio::fs::remove_dir_all(&unpacking).await;
        unpacked?;
        locked(progress).state = "done";
        log(
            "admin",
            "info",
            &format!("{} {version} is installed", label(id)),
        );
        Ok(())
    }

    /// Removes the app's own copy (a system install is left alone).
    pub async fn remove_program(&self, id: &str) -> Result<Value, CoreError> {
        // Windows keeps the file of a program that has only just ended (asked for its version a moment ago, say) for
        // an instant longer, and refuses to delete it meanwhile: tried again a few times before it is an error.
        let mut tries = 0;
        loop {
            match tokio::fs::remove_dir_all(self.program_root(id)).await {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error)
                    if cfg!(windows)
                        && tries < 20
                        && error.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    tries += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Err(error) => return Err(CoreError::new(500, error.to_string())),
            }
        }
        locked(&self.programs.installs).remove(id);
        log(
            "admin",
            "info",
            &format!("Removed the app's copy of {}", label(id)),
        );
        Ok(self.program_status(id).await)
    }

    /// Ends what runs for the programs, when the runtime stops.
    pub fn stop_programs(&self) {
        if let Some(current) = locked(&self.programs.sign_in).as_ref() {
            current.cancel.notify_one();
        }
        for pid in locked(&self.programs.running).drain() {
            workbench_tools::terminate(pid);
        }
    }
}

/// Unpacks a checked package, makes its program executable, and leaves only this version in the program's folder. An npm
/// tarball holds a `package` folder; uv's archive holds its files bare, so it is unpacked into one of that name.
async fn unpack(
    id: &str,
    archive: &Path,
    unpacking: &Path,
    target: &Path,
    root: &Path,
    version: &str,
) -> Result<(), String> {
    let describe = |error: std::io::Error| error.to_string();
    let _ = tokio::fs::remove_dir_all(unpacking).await;
    let into = if id == "uv" {
        unpacking.join("package")
    } else {
        unpacking.to_path_buf()
    };
    tokio::fs::create_dir_all(&into).await.map_err(describe)?;
    untar(archive, &into).await?;
    let binary = binary_in(id, &unpacking.join("package"))
        .ok_or_else(|| "the package does not contain the program".to_owned())?;
    make_executable(&binary).await.map_err(describe)?;
    let _ = tokio::fs::remove_dir_all(target).await;
    tokio::fs::rename(unpacking, target)
        .await
        .map_err(describe)?;
    // Older versions and leftovers go; only the version this app uses stays.
    let mut entries = tokio::fs::read_dir(root).await.map_err(describe)?;
    while let Some(entry) = entries.next_entry().await.map_err(describe)? {
        if entry.file_name().to_string_lossy() == version {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            let _ = tokio::fs::remove_dir_all(&path).await;
        } else if path != archive {
            let _ = tokio::fs::remove_file(&path).await;
        }
    }
    Ok(())
}

/// The timeout for a status or sign-out question.
pub(crate) const fn question_timeout() -> Duration {
    QUESTION_TIMEOUT
}
