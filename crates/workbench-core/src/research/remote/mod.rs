//! Persistent research jobs: SSH carries bounded JSON RPC; a detached Rust worker owns the remote process group. While a
//! run goes on, one SSH connection stays open and the worker streams its outputs as they grow; when it drops, the next
//! exchange starts from the bytes already here.
mod worker;
use std::collections::HashMap;
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
#[cfg(test)]
use worker::advance_cursors;
use worker::safe_child;
pub use worker::worker_entry;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use workbench_domain::new_id;
use workbench_domain::research::{is_id, sha256_hex};
use workbench_domain::time::{iso_time, now_millis};
use workbench_tools::shell::Invocation;
use workbench_tools::{EndOnDrop, in_own_group, launch_env, no_window, run_captured};

use super::{pretty, write_whole};
use crate::records::text;
use crate::{CoreError, Kernel, locked};

const OUTPUTS: [&str; 8] = [
    "samples.jsonl",
    "report.jsonl",
    "snapshot.json",
    "metrics.jsonl",
    "artifacts.jsonl",
    "stdout.log",
    "stderr.log",
    "resources.jsonl",
];
/// How much of one output a job may write before synchronization stops. Logs, metrics and resource readings grow with a
/// long training run; the records the app reads whole (samples, the report, the snapshot, the artifact list) stay small.
fn output_limit(name: &str) -> u64 {
    match name {
        "stdout.log" | "stderr.log" | "metrics.jsonl" | "resources.jsonl" => 1024 * 1024 * 1024,
        _ => 64 * 1024 * 1024,
    }
}
/// The most of one output a single exchange carries; while more is waiting the next exchange follows at once.
const CHUNK: u64 = 1024 * 1024;
/// The most one streamed message may be: every output's chunk, encoded, and the state.
const MESSAGE_LIMIT: u64 = 16 * 1024 * 1024;
/// How often the worker looks at the job while a connection follows it, and how long it may stay silent before it says
/// the job is unchanged, so that both ends know the connection is alive.
const FOLLOW_EVERY: Duration = Duration::from_millis(200);
const FOLLOW_SILENCE: Duration = Duration::from_secs(5);
/// How long this side waits for a word from the worker before it treats the connection as gone.
const FOLLOW_TIMEOUT: Duration = Duration::from_secs(20);
const NOTE: &str = "Uses your SSH configuration and host verification. Install this version of workbench-server on the remote host first. Code and data are not uploaded: the host builds the run's snapshot from its own working tree and checks every file against the version, and a difference refuses the run before anything starts. Closing the app detaches; stopping explicitly requests cancellation.";
/// What a refused remote run is told to do.
const REMOTE_HINT: &str = "Bring the host's working folder to the version (with git, say), or freeze a new version, then start again.";

/// Whether this job's remote execution has a final status.
/// The record of what this computer's runtime launched remotely, in the app's own data folder. A project's files say
/// what a run was, but they may come from anyone, so only this decides whether the app connects to a run, where to, and
/// whether it may still start it.
const LEDGER: &str = "remote-launches.json";

/// Said of a run whose remote record came with the project rather than from this runtime.
const NOT_LAUNCHED_HERE: &str = "The app did not start this remote run from this computer, so it does not connect to it or start it. Its record came with the project's files.";

/// Errors that will not go away by trying again: the job is gone or was never recorded, the run is gone, the request
/// is refused, or an output is too large. Anything else, such as SSH not answering, may be temporary.
fn final_remote_error(status: u16) -> bool {
    matches!(status, 400 | 403 | 404 | 410 | 413 | 422)
}

fn finished(status: &str) -> bool {
    matches!(
        status,
        "completed" | "failed" | "stopped" | "timed_out" | "refused"
    )
}

/// Fetches an explicitly requested small analysis output, never a credential or an arbitrary remote path.
pub(super) async fn artifact_bytes(run: &Value, path: &str) -> Result<Vec<u8>, CoreError> {
    if run["archived"] == true {
        return Err(CoreError::new(
            409,
            "Archived remote references are not automatically reconnected.",
        ));
    }
    let mut bytes = Vec::new();
    let mut expected = None;
    loop {
        let response = rpc(&run["remote"], &json!({ "action": "artifact", "workdir": run["remote"]["workdir"], "id": run["id"], "path": path, "cursor": bytes.len() })).await?;
        if response["size"]
            .as_u64()
            .is_none_or(|size| size > 16 * 1024 * 1024)
        {
            return Err(CoreError::new(
                413,
                "Remote artifact snapshots are limited to 16 MiB.",
            ));
        }
        if bytes.is_empty() {
            expected = response
                .get("sha256")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        let chunk = STANDARD
            .decode(text(&response, "data"))
            .map_err(|_| CoreError::new(502, "Invalid artifact encoding."))?;
        if chunk.len() as u64 > CHUNK || (chunk.is_empty() && response["next"].is_number()) {
            return Err(CoreError::new(502, "Invalid artifact chunk."));
        }
        bytes.extend(chunk);
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(CoreError::new(
                413,
                "Remote artifact snapshot is too large.",
            ));
        }
        if response["next"].is_null() {
            if expected.as_deref() != Some(sha256_hex(&bytes).as_str())
                || response["sha256"].as_str() != expected.as_deref()
            {
                return Err(CoreError::new(
                    409,
                    "The remote artifact changed during transfer.",
                ));
            }
            return Ok(bytes);
        }
        if response["next"].as_u64() != Some(bytes.len() as u64) {
            return Err(CoreError::new(502, "Invalid artifact cursor."));
        }
    }
}

/// A shell argument on the remote side; command contents themselves travel as JSON on stdin.
pub(super) fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Only a host alias or user@host is accepted here; extra SSH options belong in the user's SSH configuration.
pub(super) fn check_target(target: &Value) -> Result<(), CoreError> {
    let host = text(target, "host");
    if host.is_empty()
        || !host.as_bytes()[0].is_ascii_alphanumeric()
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"@._-".contains(&byte))
    {
        return Err(CoreError::new(
            400,
            "Use an SSH host alias or user@host, without options or shell syntax.",
        ));
    }
    // The host is Linux or macOS whatever this computer is: on Windows `Path::is_absolute` asks for a drive letter, and
    // refused every remote folder.
    if !text(target, "workdir").starts_with('/')
        || text(target, "program").is_empty()
        || text(target, "program").contains(['\n', '\r', '\0'])
    {
        return Err(CoreError::new(
            400,
            "Name an absolute remote working directory and the remote workbench-server program.",
        ));
    }
    Ok(())
}

/// An SSH command with no terminal, agent forwarding, stored keys or disabled host verification, and a connection
/// deadline. `remote` is what the host runs, in the user's own shell there.
pub(super) fn ssh(host: &str, remote: String) -> Invocation {
    Invocation {
        file: "ssh".to_owned(),
        args: vec![
            "-T".into(),
            "-a".into(),
            "-oBatchMode=yes".into(),
            "-oConnectTimeout=10".into(),
            // A connection that follows a run stays open for hours; a dead one is noticed within a minute.
            "-oServerAliveInterval=15".into(),
            "-oServerAliveCountMax=3".into(),
            host.into(),
            remote,
        ],
        verbatim: false,
    }
}

/// This process's environment without the app's own variables, for the SSH client.
pub(super) fn ssh_env() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    std::env::vars_os()
        .filter(|(name, _)| !name.to_string_lossy().starts_with("WORKBENCH_"))
        .collect()
}

/// A time-bounded exchange with the worker. Every request carries this app's version, which a worker of another
/// version refuses.
pub(super) async fn rpc(target: &Value, request: &Value) -> Result<Value, CoreError> {
    check_target(target)?;
    let invocation = ssh(
        text(target, "host"),
        format!("exec {} --research-worker", quoted(text(target, "program"))),
    );
    let mut request = request.clone();
    request["workbenchVersion"] = json!(super::workers::VERSION);
    let answer = run_captured(
        &invocation,
        Path::new("."),
        &ssh_env(),
        request.to_string().as_bytes(),
        Duration::from_secs(20),
        2 * 1024 * 1024,
    )
    .await;
    if answer.timed_out || answer.code != Some(0) {
        return Err(CoreError::new(
            502,
            "SSH did not answer. Check the host, its installed worker, authentication and known_hosts; the remote job is not assumed stopped.",
        ));
    }
    worker_message(answer.stdout.as_bytes())
}

/// What one exchange about a remote run works with.
struct RemoteExchange {
    run: Value,
    version: Value,
    /// Where this computer launched it, from the ledger: never the project's own record.
    target: Value,
    launched: bool,
    records: PathBuf,
    folder: PathBuf,
    request: Value,
}

/// One message from the worker, or the error it reports.
fn worker_message(bytes: &[u8]) -> Result<Value, CoreError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| {
        CoreError::new(
            502,
            "The remote worker returned an invalid or oversized response.",
        )
    })?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        // The worker says how it failed, so that a job that is gone is not mistaken for a host that is away.
        let status = value
            .get("status")
            .and_then(Value::as_u64)
            .and_then(|status| u16::try_from(status).ok())
            .filter(|status| (400..600).contains(status))
            .unwrap_or(502);
        return Err(CoreError::new(status, error));
    }
    Ok(value)
}

impl Kernel {
    /// Reads or sets the project's explicit remote execution target; credentials remain with SSH.
    pub async fn research_remote(
        &self,
        project: &str,
        target: Option<&Value>,
    ) -> Result<Value, CoreError> {
        let records = if target.is_some() {
            self.records_for_writing(project).await?
        } else {
            self.records_folder(project).await?
        };
        if let Some(target) = target {
            check_target(target)?;
            let value = json!({ "host": target["host"], "workdir": target["workdir"], "program": target["program"] });
            write_whole(&records.join("remote.json"), &pretty(&value)?).await?;
        }
        let target = match tokio::fs::read(safe_child(&records, "remote.json").await?).await {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes).ok(),
            Err(_) => None,
        };
        Ok(json!({ "target": target, "note": NOTE }))
    }

    /// Checks a configured host without starting a job or changing SSH trust.
    pub async fn research_remote_check(&self, project: &str) -> Result<Value, CoreError> {
        let settings = self.research_remote(project, None).await?;
        let check = rpc(
            &settings["target"],
            &json!({ "action": "check", "workdir": settings["target"]["workdir"] }),
        )
        .await?;
        // A worker from before versions were told says none, and it is not this app's either.
        if text(&check, "version") != super::workers::VERSION {
            return Err(CoreError::new(
                409,
                format!(
                    "The worker on the host is not the app {}. Connect in Environment to install the matching one.",
                    super::workers::VERSION
                ),
            ));
        }
        Ok(check)
    }

    /// Saves a remote launch intent before SSH runs. Retrying the same client/version cannot submit twice.
    pub async fn research_remote_start(
        self: &Arc<Self>,
        project: &str,
        request: &Value,
    ) -> Result<Value, CoreError> {
        let client = text(request, "clientId");
        if client.is_empty() {
            return Err(CoreError::new(400, "A remote start needs clientId."));
        }
        let version = self
            .research_record(project, "versions", text(request, "versionId"))
            .await?;
        if version.get("imported") == Some(&json!(true)) || version["archived"] == true {
            return Err(CoreError::new(
                409,
                "Set a command and freeze a new executable version before starting a remote job.",
            ));
        }
        let timeout = request
            .get("timeoutSeconds")
            .and_then(Value::as_u64)
            .unwrap_or(3600);
        if !(1..=604800).contains(&timeout) {
            return Err(CoreError::new(
                400,
                "Choose a wall-clock limit from 1 second to 7 days.",
            ));
        }
        let settings = self.research_remote(project, None).await?;
        check_target(&settings["target"])?;
        let records = self.records_for_writing(project).await?;
        let lock = self.records_lock(&records);
        let _writing = lock.lock().await;
        let runs = self.research_runs(project).await?;
        if let Some(run) = runs["items"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|run| text(run, "clientId") == client)
        {
            if run["versionId"] != version["id"] {
                return Err(CoreError::new(
                    409,
                    "This clientId already belongs to another version.",
                ));
            }
            return Ok(run.clone());
        }
        let id = new_id("run")?;
        // Recorded here first: a run whose record exists only in the project is never connected to.
        self.set_launch(
            project,
            &id,
            Some(json!({ "target": settings["target"], "launched": false })),
        )
        .await?;
        let now = iso_time(now_millis());
        let run = json!({ "schemaVersion": 1, "revision": 1, "id": id, "createdAt": now, "updatedAt": now, "status": "starting", "clientId": client,
            "experimentId": version["experimentId"], "versionId": version["id"], "kind": version.get("kind").cloned().unwrap_or(json!("evaluation")),
            "remote": settings["target"], "timeoutSeconds": timeout, "resumeFrom": request.get("resumeFrom"), "remoteConnection": "connecting", "warnings": [NOTE] });
        let folder = Self::collection_folder(&records, "runs").await?.join(&id);
        tokio::fs::create_dir_all(&folder).await?;
        write_whole(&folder.join("run.json"), &pretty(&run)?).await?;
        self.follow_remote(project.to_owned(), id);
        self.research_changed();
        Ok(run)
    }

    /// Explicit cancellation is persisted even while a host is disconnected; the next exchange delivers it.
    pub async fn research_remote_stop(&self, project: &str, id: &str) -> Result<Value, CoreError> {
        let records = self.records_folder(project).await?;
        let lock = self.records_lock(&records);
        let _writing = lock.lock().await;
        let mut run = self.research_run(project, id).await?;
        if !workbench_domain::research::run_is_active(text(&run, "status")) {
            return Ok(run);
        }
        run["status"] = json!("stopping");
        run["cancelRequested"] = json!(true);
        write_whole(
            &records.join("runs").join(id).join("run.json"),
            &pretty(&run)?,
        )
        .await?;
        self.wake_follower(id);
        self.research_changed();
        Ok(run)
    }

    /// Follows a remote run until it ends: one exchange to start it or catch up, then one connection that stays open
    /// while the worker streams what the run writes. A dropped connection is retried a second later from the bytes
    /// already here. This side may stop at any time; only the remote worker owns the computation.
    pub(super) fn follow_remote(self: &Arc<Self>, project: String, id: String) {
        let waker = Arc::new(tokio::sync::Notify::new());
        {
            let mut followed = locked(&self.research.remote);
            if followed.contains_key(&id) {
                return;
            }
            followed.insert(id.clone(), Arc::clone(&waker));
        }
        let kernel = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                if kernel.research.ending.load(Ordering::SeqCst) {
                    break;
                }
                let outcome = match kernel.sync_remote(&project, &id).await {
                    // Output is still waiting: pull the next part now rather than a second later.
                    Ok((false, true)) => continue,
                    Ok((false, false)) => kernel.stream_remote(&project, &id, &waker).await,
                    other => other.map(|(complete, _)| complete),
                };
                match outcome {
                    Ok(true) => {
                        let _ = kernel.set_launch(&project, &id, None).await;
                        break;
                    }
                    Ok(false) => {}
                    Err(error) if final_remote_error(error.status) => {
                        let _ = kernel.remote_ended(&project, &id, &error).await;
                        break;
                    }
                    Err(_) => {
                        let _ = kernel.remote_disconnected(&project, &id).await;
                    }
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            locked(&kernel.research.remote).remove(&id);
        });
    }

    /// Wakes the follower of a run, so that a stop requested here reaches the worker now rather than at the next exchange.
    fn wake_follower(&self, id: &str) {
        if let Some(waker) = locked(&self.research.remote).get(id) {
            waker.notify_one();
        }
    }

    /// What this runtime recorded of a remote launch, if it made one.
    async fn launch(&self, project: &str, id: &str) -> Option<Value> {
        let _reading = self.research.remote_ledger.lock().await;
        let ledger = tokio::fs::read(self.store.root().join(LEDGER)).await.ok()?;
        let ledger: Value = serde_json::from_slice(&ledger).ok()?;
        ledger.get(format!("{project}/{id}")).cloned()
    }

    /// Records a remote launch, or forgets it once its run has ended.
    async fn set_launch(
        &self,
        project: &str,
        id: &str,
        entry: Option<Value>,
    ) -> Result<(), CoreError> {
        let _writing = self.research.remote_ledger.lock().await;
        let path = self.store.root().join(LEDGER);
        let mut ledger: Value = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({})),
            Err(_) => json!({}),
        };
        if !ledger.is_object() {
            ledger = json!({});
        }
        let key = format!("{project}/{id}");
        match entry {
            Some(entry) => ledger[&key] = entry,
            None => {
                if let Some(entries) = ledger.as_object_mut() {
                    entries.remove(&key);
                }
            }
        }
        write_whole(&path, &pretty(&ledger)?).await
    }

    /// A remote run that cannot go on: it says why, and it is no longer followed. A run that is gone stays gone.
    async fn remote_ended(
        &self,
        project: &str,
        id: &str,
        error: &CoreError,
    ) -> Result<(), CoreError> {
        let _ = self.set_launch(project, id, None).await;
        let records = self.records_folder(project).await?;
        let lock = self.records_lock(&records);
        let _writing = lock.lock().await;
        let mut run = self.research_run(project, id).await?;
        if !workbench_domain::research::run_is_active(text(&run, "status")) {
            return Ok(());
        }
        // Not started here, or its job is gone: it may have run, so it is interrupted rather than failed.
        run["status"] = json!(if matches!(error.status, 403 | 410) {
            "interrupted"
        } else {
            "failed"
        });
        run["remoteConnection"] = json!("ended");
        run["remoteError"] = json!(error.message);
        run["endedAt"] = json!(iso_time(now_millis()));
        run["updatedAt"] = run["endedAt"].clone();
        write_whole(
            &records.join("runs").join(id).join("run.json"),
            &pretty(&run)?,
        )
        .await?;
        self.research_changed();
        Ok(())
    }

    /// Connection failure is not evidence that an expensive remote computation ended.
    async fn remote_disconnected(&self, project: &str, id: &str) -> Result<(), CoreError> {
        let records = self.records_folder(project).await?;
        let lock = self.records_lock(&records);
        let _writing = lock.lock().await;
        let mut run = self.research_run(project, id).await?;
        run["remoteConnection"] = json!("disconnected");
        run["remoteError"] = json!(
            "Cannot reach the remote worker. It may still be running; cancellation is pending until acknowledged."
        );
        write_whole(
            &records.join("runs").join(id).join("run.json"),
            &pretty(&run)?,
        )
        .await?;
        self.research_changed();
        Ok(())
    }

    /// What an exchange about a remote run needs: the run, its version, where it was launched from this computer, and
    /// the request that asks the worker for everything past what is already here.
    async fn remote_exchange(
        &self,
        project: &str,
        id: &str,
        action: &str,
    ) -> Result<Option<RemoteExchange>, CoreError> {
        let run = self.research_run(project, id).await?;
        if !workbench_domain::research::run_is_active(text(&run, "status")) {
            return Ok(None);
        }
        let version = self
            .research_record(project, "versions", text(&run, "versionId"))
            .await?;
        let launch = self
            .launch(project, id)
            .await
            .ok_or_else(|| CoreError::new(403, NOT_LAUNCHED_HERE))?;
        let target = launch["target"].clone();
        let launched = launch["launched"] == true;
        let records = self.records_folder(project).await?;
        let folder = records.join("runs").join(id);
        let mut cursors = HashMap::new();
        for name in OUTPUTS {
            cursors.insert(
                name,
                tokio::fs::metadata(folder.join(name))
                    .await
                    .map_or(0, |info| info.len()),
            );
        }
        let request = json!({ "action": action, "workdir": target["workdir"], "id": id, "cancel": run.get("cancelRequested").and_then(Value::as_bool).unwrap_or(false), "cursors": cursors });
        Ok(Some(RemoteExchange {
            run,
            version,
            target,
            launched,
            records,
            folder,
            request,
        }))
    }

    /// One bounded exchange with the worker, which starts the job the first time: whether the run is complete, and
    /// whether output is still waiting to be pulled.
    async fn sync_remote(&self, project: &str, id: &str) -> Result<(bool, bool), CoreError> {
        let Some(mut exchange) = self.remote_exchange(project, id, "sync").await? else {
            return Ok((true, false));
        };
        // Only the first exchange may start the job. After that a job the worker no longer has is reported, not resent.
        if !exchange.launched {
            let (run, version) = (&exchange.run, &exchange.version);
            // What the host must hold, so that it checks its own files against the version before it runs anything.
            let expected = json!({ "number": version["number"], "git": version["git"], "changedFiles": version["changedFiles"], "inputs": version["inputs"], "cwd": version["cwd"], "seeds": version["seeds"], "sandbox": version.get("sandbox").cloned().unwrap_or(Value::Null), "envNames": version.get("envNames").cloned().unwrap_or_else(|| json!([])) });
            exchange.request["start"] = json!({ "command": version["command"], "kind": run["kind"], "timeoutSeconds": run["timeoutSeconds"], "resumeFrom": run["resumeFrom"], "versionId": version["id"], "expected": expected });
        }
        let response = rpc(&exchange.target, &exchange.request).await?;
        if !exchange.launched {
            self.set_launch(
                project,
                id,
                Some(json!({ "target": exchange.target, "launched": true })),
            )
            .await?;
        }
        self.apply_remote_answer(project, id, &exchange, &response)
            .await
    }

    /// Holds one connection open while the worker streams the run's outputs and state as they change, until the run is
    /// complete (`true`) or the connection is gone. A stop requested meanwhile goes down the same connection.
    async fn stream_remote(
        &self,
        project: &str,
        id: &str,
        waker: &tokio::sync::Notify,
    ) -> Result<bool, CoreError> {
        let Some(exchange) = self.remote_exchange(project, id, "follow").await? else {
            return Ok(true);
        };
        let invocation = ssh(
            text(&exchange.target, "host"),
            format!(
                "exec {} --research-worker",
                quoted(text(&exchange.target, "program"))
            ),
        );
        let mut command = Command::new(&invocation.file);
        no_window(&mut command);
        command
            .args(&invocation.args)
            .env_clear()
            .envs(ssh_env())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| CoreError::new(502, format!("SSH could not be started: {error}")))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| CoreError::new(502, "no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::new(502, "no stdout"))?;
        let mut request = exchange.request.clone();
        request["workbenchVersion"] = json!(super::workers::VERSION);
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .map_err(|_| CoreError::new(502, "SSH did not take the request."))?;
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        let mut cancelled = exchange.request["cancel"] == true;
        loop {
            let mut bounded = (&mut reader).take(MESSAGE_LIMIT.saturating_sub(line.len() as u64));
            tokio::select! {
                read = tokio::time::timeout(FOLLOW_TIMEOUT, bounded.read_until(b'\n', &mut line)) => {
                    match read {
                        Err(_) => return Err(CoreError::new(502, "The remote worker has not answered for a while; the connection is treated as lost. The job is not assumed stopped.")),
                        Ok(Err(_)) | Ok(Ok(0)) => return Err(CoreError::new(502, "The connection to the remote worker ended; the job is not assumed stopped.")),
                        Ok(Ok(_)) => {}
                    }
                    if line.last() != Some(&b'\n') {
                        if line.len() as u64 >= MESSAGE_LIMIT {
                            return Err(CoreError::new(502, "The remote worker returned an oversized message."));
                        }
                        continue;
                    }
                    let message = std::mem::take(&mut line);
                    let response = worker_message(&message)?;
                    let (complete, _) = self.apply_remote_answer(project, id, &exchange, &response).await?;
                    if complete {
                        return Ok(true);
                    }
                }
                _ = waker.notified(), if !cancelled => {
                    let run = self.research_run(project, id).await?;
                    if run.get("cancelRequested") == Some(&json!(true)) {
                        cancelled = true;
                        stdin
                            .write_all(b"{\"cancel\":true}\n")
                            .await
                            .map_err(|_| CoreError::new(502, "The stop request could not be sent; the connection is treated as lost."))?;
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    if self.research.ending.load(Ordering::SeqCst) {
                        return Err(CoreError::new(503, "The app is stopping."));
                    }
                }
            }
        }
    }

    /// Writes what the worker gave into the run's folder and record: the next part of each output, the remote state, and
    /// on the last message the verified completion. Gives whether the run is complete and whether output still waits.
    async fn apply_remote_answer(
        &self,
        project: &str,
        id: &str,
        exchange: &RemoteExchange,
        response: &Value,
    ) -> Result<(bool, bool), CoreError> {
        let RemoteExchange {
            version,
            records,
            folder,
            ..
        } = exchange;
        let lock = self.records_lock(records);
        let _writing = lock.lock().await;
        let mut current = self.research_run(project, id).await?;
        for chunk in response["files"].as_array().into_iter().flatten() {
            let name = text(chunk, "name");
            if !OUTPUTS.contains(&name) {
                return Err(CoreError::new(
                    502,
                    "The worker named an unexpected output.",
                ));
            }
            let bytes = STANDARD
                .decode(text(chunk, "data"))
                .map_err(|_| CoreError::new(502, "Invalid output encoding."))?;
            if bytes.len() as u64 > CHUNK {
                return Err(CoreError::new(502, "Oversized remote output chunk."));
            }
            let offset = chunk["offset"]
                .as_u64()
                .ok_or_else(|| CoreError::new(502, "Missing remote cursor."))?;
            let target = folder.join(name);
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&target)
                .await?;
            if chunk["reset"] == true {
                file.set_len(0).await?;
            }
            if file.metadata().await?.len() != offset {
                return Err(CoreError::new(409, "Remote stream cursor changed."));
            }
            use tokio::io::{AsyncSeekExt, AsyncWriteExt};
            file.seek(SeekFrom::Start(offset)).await?;
            file.write_all(&bytes).await?;
        }
        let state = &response["state"];
        let remote_status = text(state, "status");
        current["remoteStatus"] = json!(remote_status);
        current["remoteConnection"] = json!(if response["unresponsive"] == true {
            "unresponsive"
        } else {
            "connected"
        });
        current["remoteError"] = if response["unresponsive"] == true {
            json!(
                "The worker heartbeat is stale. Its computation may still be running; no automatic resubmission is attempted."
            )
        } else {
            Value::Null
        };
        current["remoteSource"] = state["source"].clone();
        if state["system"].is_object() {
            current["remoteSystem"] = state["system"].clone();
        }
        current["updatedAt"] = json!(iso_time(now_millis()));
        let complete = finished(remote_status) && response["more"] == false;
        if complete {
            let bytes = tokio::fs::read(folder.join("samples.jsonl")).await?;
            for name in OUTPUTS {
                let (hash, _) = super::freeze::hash_file(&folder.join(name)).await?;
                if response["hashes"][name] != hash {
                    return Err(CoreError::new(
                        409,
                        "A remote output changed during synchronization; completion is not certified.",
                    ));
                }
            }
            let scan = super::runs::scan_samples(&folder.join("samples.jsonl"))?;
            let count = scan.samples;
            let mut protocol_error = !scan.problems.is_empty();
            let (metric_count, metrics_valid) =
                super::imports::metric_output(&folder.join("metrics.jsonl"))?;
            let has_metrics = metric_count > 0;
            protocol_error |= !metrics_valid;
            let status = match remote_status {
                "stopped" | "timed_out" => "stopped",
                "failed" | "refused" => "failed",
                _ if protocol_error
                    || (count == 0 && !(current["kind"] == "training" && has_metrics)) =>
                {
                    "protocol_error"
                }
                _ => "completed",
            };
            current["status"] = json!(status);
            current["samples"] = json!(count);
            current["samplesSize"] = json!(bytes.len());
            current["samplesSha256"] = json!(sha256_hex(&bytes));
            current["exitCode"] = state["exitCode"].clone();
            current["endedAt"] = state["endedAt"].clone();
            if remote_status == "refused" {
                current["refused"] = json!(true);
                current["error"] = state["error"].clone();
            }
            current["inputsChanged"] = state
                .pointer("/afterRun/changed")
                .cloned()
                .unwrap_or(json!([]));
            super::evidence::remote_finished(folder, version, state, &current).await;
        } else if current.get("cancelRequested") != Some(&json!(true)) {
            current["status"] = json!(if remote_status == "starting" {
                "starting"
            } else {
                "running"
            });
        }
        write_whole(&folder.join("run.json"), &pretty(&current)?).await?;
        self.research_changed();
        Ok((complete, response["more"] == true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The remote folder is a path on the host, so it is judged as one there, on whatever system the app runs.
    #[test]
    fn a_remote_folder_is_absolute_the_way_the_host_writes_it() {
        let target = |workdir: &str| json!({ "host": "gpu-box", "workdir": workdir, "program": "workbench-server" });
        assert!(check_target(&target("/home/someone/study")).is_ok());
        for refused in ["study", "~/study", "C:\\study", ""] {
            assert!(check_target(&target(refused)).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_message_moves_each_cursor_to_where_its_chunk_ended() {
        let mut cursors = json!({ "stdout.log": 10, "samples.jsonl": 0 });
        let message = json!({ "files": [
            { "name": "stdout.log", "offset": 10, "end": 1034, "reset": false, "data": "" },
            { "name": "samples.jsonl", "offset": 0, "end": 0, "reset": false, "data": "" },
            { "name": "metrics.jsonl", "offset": 0, "end": 7, "reset": true, "data": "" },
        ] });
        advance_cursors(&mut cursors, &message);
        assert_eq!(
            cursors,
            json!({ "stdout.log": 1034, "samples.jsonl": 0, "metrics.jsonl": 7 })
        );
        assert_eq!(
            worker_message(br#"{"error":"gone","status":410}"#)
                .expect_err("an error")
                .status,
            410
        );
        assert_eq!(
            worker_message(b"not json").expect_err("an error").status,
            502
        );
    }

    #[test]
    fn only_errors_that_cannot_clear_end_a_remote_run() {
        for status in [400, 403, 404, 410, 413, 422] {
            assert!(final_remote_error(status), "{status}");
        }
        for status in [409, 500, 502, 503, 504] {
            assert!(!final_remote_error(status), "{status} may clear");
        }
    }
}
