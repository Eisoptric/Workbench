//! Detached remote worker daemon that owns the remote process group and streams outputs over SSH JSON-RPC.
use super::*;

pub async fn worker_entry(mode: &str, directory: Option<&str>) -> Result<(), CoreError> {
    if cfg!(windows) {
        return Err(CoreError::new(
            501,
            "The detached worker runs on Linux or macOS.",
        ));
    }
    if mode == "--research-worker-execute" {
        let folder =
            Path::new(directory.ok_or_else(|| CoreError::new(400, "Missing job directory."))?);
        // Out of the SSH session first: when that connection closes, the system ends what is still in its session.
        let result = match workbench_tools::leave_session() {
            Ok(()) => execute_worker(folder).await,
            Err(error) => Err(CoreError::new(
                500,
                format!("The worker could not leave the SSH session: {error}"),
            )),
        };
        if result.is_err() {
            write_whole(&folder.join("state.json"), &pretty(&json!({ "schemaVersion": 1, "status": "failed", "endedAt": iso_time(now_millis()), "error": "The remote command could not be started or recorded." }))?).await?;
        }
        return result;
    }
    // The request is the first line; a connection that follows a run keeps stdin open for a stop request.
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take(2 * 1024 * 1024)
        .read_until(b'\n', &mut bytes)?;
    let request: Value = serde_json::from_slice(&bytes)
        .map_err(|_| CoreError::new(400, "Invalid worker request."))?;
    let first = worker_rpc(&request).await;
    let answered = match &first {
        Ok(value) => say(value),
        Err(error) => say(&json!({ "error": error.message, "status": error.status })),
    };
    if let (Ok(first), true, "follow") = (first, answered, text(&request, "action")) {
        worker_follow(&request, first).await?;
    }
    Ok(())
}

/// One line to whoever holds the connection. `false` once they have gone, which ends the exchange quietly.
fn say(message: &Value) -> bool {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{message}").is_ok() && stdout.flush().is_ok()
}

/// Streams a job's outputs and state to the connection as they change, after the first answer went: a message
/// whenever something grew or the state moved, else one every few seconds, until the job has ended and everything
/// has been given, or the other side goes. A `{"cancel":true}` line from them asks the job to stop.
async fn worker_follow(request: &Value, first: Value) -> Result<(), CoreError> {
    let folder = job_folder(request).await?;
    let state_file = safe_child(&folder, "state.json").await?;
    let mut cursors = request["cursors"].clone();
    advance_cursors(&mut cursors, &first);
    if finished(text(&first["state"], "status")) && first["more"] == false {
        return Ok(());
    }
    // Their lines, read on a thread since stdin blocks; `None` when they have gone.
    let (sender, mut lines) = tokio::sync::mpsc::unbounded_channel::<Option<String>>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            match line {
                Ok(line) => {
                    if sender.send(Some(line)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = sender.send(None);
    });
    let mut last_state = first["state"].clone();
    let mut last_said = tokio::time::Instant::now();
    loop {
        while let Ok(received) = lines.try_recv() {
            match received {
                None => return Ok(()),
                Some(line) => {
                    if serde_json::from_str::<Value>(&line)
                        .is_ok_and(|asked| asked["cancel"] == true)
                    {
                        write_whole(&safe_child(&folder, "cancel.request").await?, b"cancel\n")
                            .await?;
                    }
                }
            }
        }
        let state: Value = serde_json::from_slice(&tokio::fs::read(&state_file).await?)
            .map_err(|_| CoreError::new(422, "Remote state is damaged."))?;
        let answer = outputs_answer(&folder, &state, &cursors).await?;
        let grew = answer["files"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|chunk| !text(chunk, "data").is_empty() || chunk["reset"] == true);
        if grew || state != last_state || last_said.elapsed() >= FOLLOW_SILENCE {
            if !say(&answer) {
                return Ok(());
            }
            last_said = tokio::time::Instant::now();
            last_state = state;
            advance_cursors(&mut cursors, &answer);
            if finished(text(&answer["state"], "status")) && answer["more"] == false {
                return Ok(());
            }
        }
        tokio::time::sleep(FOLLOW_EVERY).await;
    }
}

/// Moves each output's cursor past the chunk a message carried.
pub(super) fn advance_cursors(cursors: &mut Value, message: &Value) {
    for chunk in message["files"].as_array().into_iter().flatten() {
        if let Some(end) = chunk["end"].as_u64() {
            cursors[text(chunk, "name")] = json!(end);
        }
    }
}

/// A record directory must never follow an output or state symlink outside its configured root.
pub(super) async fn safe_child(parent: &Path, name: &str) -> Result<PathBuf, CoreError> {
    let path = parent.join(name);
    if tokio::fs::symlink_metadata(&path)
        .await
        .is_ok_and(|info| info.file_type().is_symlink())
    {
        return Err(CoreError::new(
            403,
            "Remote state or output is a symbolic link.",
        ));
    }
    Ok(path)
}

/// A single bounded worker exchange; repeated launch intents find the same job directory.
async fn worker_rpc(request: &Value) -> Result<Value, CoreError> {
    // A copy of the app of another version may expect another worker; it is told so rather than half understood.
    let asking = text(request, "workbenchVersion");
    if !asking.is_empty() && asking != crate::research::workers::VERSION {
        return Err(CoreError::new(
            409,
            format!(
                "This worker is the app {}, and the app {asking} asked. Install the matching worker from Environment.",
                crate::research::workers::VERSION
            ),
        ));
    }
    let root = tokio::fs::canonicalize(text(request, "workdir")).await?;
    if text(request, "action") == "check" {
        return Ok(
            json!({ "protocol": 1, "platform": std::env::consts::OS, "workdir": root, "version": crate::research::workers::VERSION }),
        );
    }
    let id = text(request, "id");
    let folder = job_folder(request).await?;
    if text(request, "action") == "artifact" {
        let requested = text(request, "path");
        if [
            "start.json",
            "state.json",
            "launch.claim",
            "execute.claim",
            "cancel.request",
        ]
        .contains(&requested)
        {
            return Err(CoreError::new(
                403,
                "Worker control files are not analysis artifacts.",
            ));
        }
        let path = workbench_tools::file::inside_workspace(
            &workbench_domain::skills::workspace::Workspace::new(&folder),
            requested,
        )
        .await
        .map_err(|message| CoreError::new(403, message))?;
        if [
            "start.json",
            "state.json",
            "launch.claim",
            "execute.claim",
            "cancel.request",
        ]
        .iter()
        .any(|name| path == folder.join(name))
        {
            return Err(CoreError::new(
                403,
                "Worker control files are not analysis artifacts.",
            ));
        }
        let mut file = std::fs::File::open(&path)?;
        let size = file.metadata()?.len();
        if size > 16 * 1024 * 1024 {
            return Err(CoreError::new(
                413,
                "Remote artifact snapshots are limited to 16 MiB.",
            ));
        }
        let cursor = request["cursor"].as_u64().unwrap_or(0);
        if cursor > size {
            return Err(CoreError::new(409, "The artifact was truncated."));
        }
        file.seek(SeekFrom::Start(cursor))?;
        let mut bytes = Vec::new();
        file.take(CHUNK).read_to_end(&mut bytes)?;
        let end = cursor + bytes.len() as u64;
        let hash = if cursor == 0 || end == size {
            Some(crate::research::freeze::hash_file(&path).await?.0)
        } else {
            None
        };
        return Ok(
            json!({ "data": STANDARD.encode(bytes), "size": size, "sha256": hash, "next": if end < size { json!(end) } else { Value::Null } }),
        );
    }
    let state_file = safe_child(&folder, "state.json").await?;
    if !tokio::fs::try_exists(&state_file).await? {
        if request.get("start").is_none_or(Value::is_null) {
            return Err(CoreError::new(
                410,
                "This job is not on the remote host: its folder is gone, or it was never started. The app does not start it again; start a new run.",
            ));
        }
        let claim = safe_child(&folder, "launch.claim").await?;
        tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(claim)
            .await
            .map_err(|_| {
                CoreError::new(
                    410,
                    "A launch was claimed but never recorded, so it may or may not have run. It is not submitted again; start a new run.",
                )
            })?;
        let start = &request["start"];
        if text(start, "command").trim().is_empty()
            || !start["timeoutSeconds"]
                .as_u64()
                .is_some_and(|seconds| (1..=604800).contains(&seconds))
        {
            return Err(CoreError::new(
                400,
                "The worker needs a command and a bounded wall-clock limit.",
            ));
        }
        write_whole(
            &folder.join("start.json"),
            &pretty(&json!({ "workdir": root, "id": id, "spec": start }))?,
        )
        .await?;
        let now = iso_time(now_millis());
        let cancel = request["cancel"] == true;
        write_whole(&state_file, &pretty(&json!({ "schemaVersion": 1, "status": if cancel { "stopped" } else { "starting" }, "createdAt": now, "endedAt": if cancel { json!(now) } else { Value::Null } }))?).await?;
        if !cancel {
            // Started directly, not through nohup: macOS's nohup refuses to run inside an SSH session ("can't detach
            // from console"). The worker leaves the session itself, which also takes it out of reach of hangups.
            let mut child = Command::new(std::env::current_exe()?);
            launch_env(&mut child);
            child
                .arg("--research-worker-execute")
                .arg(&folder)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            // Not in its own group: it leaves the session itself, which a group leader cannot do.
            if child.spawn().is_err() {
                write_whole(&state_file, &pretty(&json!({ "schemaVersion": 1, "status": "failed", "endedAt": iso_time(now_millis()), "error": "The detached worker could not be started." }))?).await?;
            } else {
                // Answer only once it has left this SSH session, or the session could end it first.
                let claimed = folder.join("execute.claim");
                for _ in 0..100 {
                    if tokio::fs::try_exists(&claimed).await.unwrap_or(false) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
    if request["cancel"] == true {
        write_whole(&safe_child(&folder, "cancel.request").await?, b"cancel\n").await?;
    }
    let state: Value = serde_json::from_slice(&tokio::fs::read(&state_file).await?)
        .map_err(|_| CoreError::new(422, "Remote state is damaged."))?;
    outputs_answer(&folder, &state, &request["cursors"]).await
}

/// The job's folder on this host, `<workdir>/.workbench/remote-jobs/<id>`, made if it is not there.
async fn job_folder(request: &Value) -> Result<PathBuf, CoreError> {
    let root = tokio::fs::canonicalize(text(request, "workdir")).await?;
    let id = text(request, "id");
    if !is_id(id) || !id.starts_with("run_") {
        return Err(CoreError::new(400, "Invalid remote job ID."));
    }
    let workbench = safe_child(&root, ".workbench").await?;
    tokio::fs::create_dir_all(&workbench).await?;
    let jobs = safe_child(&workbench, "remote-jobs").await?;
    tokio::fs::create_dir_all(&jobs).await?;
    let folder = safe_child(&jobs, id).await?;
    tokio::fs::create_dir_all(&folder).await?;
    Ok(folder)
}

/// What one exchange gives back: the state, the next part of each output past the caller's cursors (with where it
/// ends), whether more waits, every output's hash once the job has ended and all of it has been given, and whether the
/// heartbeat is stale.
async fn outputs_answer(folder: &Path, state: &Value, cursors: &Value) -> Result<Value, CoreError> {
    let mut files = Vec::new();
    let mut more = false;
    for name in OUTPUTS {
        let path = safe_child(folder, name).await?;
        let (size, mut file) = match std::fs::File::open(&path) {
            Ok(file) => (file.metadata()?.len(), Some(file)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (0, None),
            Err(error) => return Err(error.into()),
        };
        let limit = output_limit(name);
        if size > limit {
            return Err(CoreError::new(
                413,
                format!(
                    "The remote {name} exceeded its {} MiB synchronization limit. The job may still be running.",
                    limit / (1024 * 1024)
                ),
            ));
        }
        let cursor = cursors[name].as_u64().unwrap_or(0);
        let reset = cursor > size;
        let start = if reset { 0 } else { cursor };
        let mut bytes = Vec::new();
        if let Some(file) = file.as_mut() {
            file.seek(SeekFrom::Start(start))?;
            file.take(CHUNK).read_to_end(&mut bytes)?;
        }
        let end = start + bytes.len() as u64;
        more |= end < size;
        files.push(json!({ "name": name, "offset": start, "end": end, "reset": reset, "data": STANDARD.encode(&bytes) }));
    }
    let mut hashes = json!({});
    if finished(text(state, "status")) && !more {
        for name in OUTPUTS {
            let path = safe_child(folder, name).await?;
            hashes[name] = match crate::research::freeze::hash_file(&path).await {
                Ok((hash, _)) => json!(hash),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    json!(sha256_hex(b""))
                }
                Err(error) => return Err(error.into()),
            };
        }
    }
    let unresponsive = !finished(text(state, "status"))
        && state["heartbeatMs"]
            .as_i64()
            .is_some_and(|time| now_millis().saturating_sub(time) > 10_000);
    Ok(
        json!({ "state": state, "files": files, "more": more, "hashes": hashes, "unresponsive": unresponsive }),
    )
}

/// The detached worker executes one command and owns its cancellation deadline and complete process group.
async fn execute_worker(folder: &Path) -> Result<(), CoreError> {
    let claim = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(folder.join("execute.claim"))
        .await;
    match claim {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let start: Value = serde_json::from_slice(&tokio::fs::read(folder.join("start.json")).await?)
        .map_err(|_| CoreError::new(422, "Invalid remote launch record."))?;
    let spec = &start["spec"];
    let workdir = PathBuf::from(text(&start, "workdir"));
    // The snapshot is built and checked before anything runs; a difference ends the job without running it.
    let snapshot_folder = folder.join("snapshot");
    let mut working = workdir.clone();
    let mut observed = Value::Null;
    if spec["expected"].is_object() {
        let tree = snapshot_folder.join("tree");
        let _ = tokio::fs::remove_dir_all(&snapshot_folder).await;
        tokio::fs::create_dir_all(&tree).await?;
        match crate::research::snapshot::fill(
            None,
            &workdir,
            &spec["expected"],
            &snapshot_folder,
            &tree,
            REMOTE_HINT,
        )
        .await
        {
            Ok(made) => {
                observed = made;
                working = tree.join(text(&spec["expected"], "cwd"));
            }
            Err(error) => {
                let _ = tokio::fs::remove_dir_all(&snapshot_folder).await;
                let now = iso_time(now_millis());
                write_whole(&folder.join("state.json"), &pretty(&json!({ "schemaVersion": 1, "status": "refused", "error": error.message, "createdAt": now, "endedAt": now }))?).await?;
                return Ok(());
            }
        }
    }
    let seeds: Vec<String> = spec["expected"]["seeds"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|seed| {
            seed.as_str()
                .map_or_else(|| seed.to_string(), str::to_owned)
        })
        .collect();
    let mut git_command = Command::new("git");
    no_window(&mut git_command);
    let git = git_command
        .args(["rev-parse", "HEAD"])
        .current_dir(&workdir)
        .output()
        .await
        .ok();
    // With a snapshot the source was rebuilt from the host's tree and checked against the version; without one (a
    // start from before snapshots) it is only observed.
    let note = if observed.is_null() {
        "Remote source is observed, not uploaded or certified identical to local frozen files."
    } else {
        "Remote source was rebuilt on the host from its working tree and git, and every file in the version was checked against the version's hash before the command ran; what the version does not name is listed as uncontrolled."
    };
    let source = json!({ "workdir": start["workdir"], "gitCommit": git.filter(|result| result.status.success()).map(|result| String::from_utf8_lossy(&result.stdout).trim().to_owned()), "note": note });
    // A job with a snapshot runs in the sandbox its version asks for, as a local run does; one sent by a from
    // before snapshots runs in the working folder, as it did then.
    let (mut command, sandbox) = if observed.is_null() {
        let mut command = Command::new("/bin/sh");
        launch_env(&mut command);
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("WORKBENCH_") {
                command.env_remove(name);
            }
        }
        command
            .args(["-c", text(spec, "command")])
            .current_dir(&working);
        (command, Value::Null)
    } else {
        let placement = crate::research::isolation::Placement {
            tree: snapshot_folder.join("tree"),
            temporary: snapshot_folder.join("tmp"),
            records: folder.to_path_buf(),
            protected: [
                "start.json",
                "state.json",
                "launch.claim",
                "execute.claim",
                "snapshot.json",
                "resources.jsonl",
            ]
            .iter()
            .map(|name| folder.join(name))
            .collect(),
            sources: vec![workdir.clone()],
            readable: Vec::new(),
        };
        let data = crate::research::isolation::data_folders(
            &crate::research::isolation::home().join(".workbench-app"),
        );
        crate::research::isolation::command(
            &spec["expected"],
            &placement,
            &data,
            &working,
            text(spec, "command"),
        )
        .await?
    };
    if !observed.is_null() {
        if sandbox["isolation"] == "sandbox" {
            observed["isolation"] = json!("sandbox");
        }
        // Written once, before the command runs, and brought back with the outputs.
        write_whole(&folder.join("snapshot.json"), &pretty(&observed)?).await?;
    }
    command
        .stdin(Stdio::null())
        .stdout(
            tokio::fs::File::create(folder.join("stdout.log"))
                .await?
                .into_std()
                .await,
        )
        .stderr(
            tokio::fs::File::create(folder.join("stderr.log"))
                .await?
                .into_std()
                .await,
        )
        .env("WORKBENCH_RUN_ID", text(&start, "id"))
        .env("WORKBENCH_RUN_TOKEN", workbench_domain::random_hex(16)?)
        .env("WORKBENCH_RUN_VERSION", text(spec, "versionId"))
        .env("WORKBENCH_RUN_DIRECTORY", folder)
        .env("WORKBENCH_RUN_SAMPLES", folder.join("samples.jsonl"))
        .env("WORKBENCH_RUN_METRICS", folder.join("metrics.jsonl"))
        .env("WORKBENCH_RUN_ARTIFACTS", folder.join("artifacts.jsonl"))
        .env("WORKBENCH_RUN_REPORT", folder.join("report.jsonl"))
        .env("WORKBENCH_RESUME_FROM", text(spec, "resumeFrom"));
    if let Some(first) = seeds.first() {
        command
            .env("WORKBENCH_RUN_SEED", first)
            .env("WORKBENCH_RUN_SEEDS", seeds.join(","));
    }
    in_own_group(&mut command);
    let mut child = command.spawn()?;
    let pid = child.id().unwrap_or(0);
    let mut guard = EndOnDrop::new(child.id());
    // The job's processor, memory and GPUs, sampled on this host as a local run is, and synced back with the outputs.
    let stop_sampling = Arc::new(AtomicBool::new(false));
    let sampler = tokio::spawn(crate::research::resources::sample_until(
        pid,
        folder.to_path_buf(),
        Some(Arc::clone(&stop_sampling)),
    ));
    let begin = tokio::time::Instant::now();
    let limit = Duration::from_secs(spec["timeoutSeconds"].as_u64().unwrap_or(3600));
    let mut state = json!({ "schemaVersion": 1, "status": "running", "startedAt": iso_time(now_millis()), "source": source, "system": crate::research::resources::system(), "snapshotted": !observed.is_null(), "seedsPassed": seeds, "sandbox": sandbox });
    let (status, code) = loop {
        state["heartbeatMs"] = json!(now_millis());
        write_whole(&folder.join("state.json"), &pretty(&state)?).await?;
        if tokio::fs::try_exists(folder.join("cancel.request")).await? || begin.elapsed() >= limit {
            let status = if begin.elapsed() >= limit {
                "timed_out"
            } else {
                "stopped"
            };
            crate::research::runs::end_group(pid).await;
            let code = child.wait().await.ok().and_then(|status| status.code());
            break (status, code);
        }
        if let Some(exit) = child.try_wait()? {
            break (
                if exit.success() {
                    "completed"
                } else {
                    "failed"
                },
                exit.code(),
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    crate::research::runs::end_group(pid).await;
    guard.finished();
    stop_sampling.store(true, Ordering::SeqCst);
    let _ = sampler.await;
    if !observed.is_null() {
        state["afterRun"] = crate::research::snapshot::close(
            &snapshot_folder,
            &folder.join("written"),
            &observed,
            &workdir,
            true,
        )
        .await;
    }
    state["status"] = json!(status);
    state["exitCode"] = json!(code);
    state["endedAt"] = json!(iso_time(now_millis()));
    write_whole(&folder.join("state.json"), &pretty(&state)?).await?;
    Ok(())
}
