//! The GitHub tools; `contract/github.test.mjs` drives them against a fake GitHub.
//! Repository content (files, trees, grep, branches, commits, diffs) comes from a local git checkout: the workspace repo when
//! its `origin` remote is the requested owner/name, otherwise a blobless clone kept under <data>/origin/<owner>/<name> (made
//! on first use, refreshed by sync_mirror). Hosted features go to the GitHub API with the user's own token. git always runs
//! with an argument list (never a shell), so tool arguments cannot inject commands.
mod github;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;

use base64::Engine;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use regex::Regex;
use reqwest::Method;
use serde_json::{Map, Value, json};
use tokio::process::Command;
use workbench_domain::js;
use workbench_telemetry::network::{SendNoted, client_builder};
use workbench_tools::shell::process_env;

use crate::Kernel;

const MAX_OUTPUT: usize = 200_000;
const API_TIMEOUT: Duration = Duration::from_secs(300);
const JSON_ACCEPT: &str = "application/vnd.github+json";
const FAILED_CONCLUSIONS: [&str; 4] = ["failure", "timed_out", "cancelled", "action_required"];

type Failure = String;
type Answer = Result<Value, Failure>;

/// A tool's text argument, trimmed; empty when it is missing or not text.
fn string_of(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).map_or("", js::trim)
}

fn arg<'a>(args: &'a Value, key: &str) -> &'a str {
    string_of(args.get(key))
}

/// A tool's number argument, if it is one.
fn number_of(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite())
}

/// `String(value)`, for ids that come as numbers.
fn id_of(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(other) => js::text(Some(other)),
    }
}

fn page(args: &Value) -> i64 {
    let digits: String = arg(args, "pageToken")
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse::<i64>().unwrap_or(1).max(1)
}

fn page_size(args: &Value, fallback: f64) -> i64 {
    number_of(args.get("pageSize"))
        .unwrap_or(fallback)
        .clamp(1.0, 100.0) as i64
}

/// What `encodeURIComponent` leaves as it is, besides letters and digits.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// `encodeURIComponent`.
fn encode(text: &str) -> String {
    utf8_percent_encode(text, URI_COMPONENT).to_string()
}

/// Refs and SHAs the model passes to git. git runs without a shell, so the risks left are a value that reads as an option
/// ("--upload-pack=…"), whitespace or control characters, and ":" (ref:path addressing).
fn safe_ref(value: &str, label: &str) -> Result<String, Failure> {
    let unsafe_character = |character: char| {
        js::is_space(character) || character == ':' || character.is_ascii_control()
    };
    if value.is_empty() || value.starts_with('-') || value.chars().any(unsafe_character) {
        let shown = if value.is_empty() { "(empty)" } else { value };
        return Err(format!("Invalid {label}: {shown}"));
    }
    Ok(value.to_owned())
}

/// `path.posix.normalize`.
fn normalize(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|last| *last != "..") => {
                parts.pop();
            }
            ".." if absolute => {}
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

/// Repository-relative paths only: no absolute paths and no climbing out with "..".
fn safe_path(value: &str) -> Result<String, Failure> {
    let normalized = normalize(&value.replace('\\', "/"));
    if normalized.starts_with('/') || normalized == ".." || normalized.starts_with("../") {
        return Err(format!("Path must be inside the repository: {value}"));
    }
    Ok(if normalized == "." {
        String::new()
    } else {
        normalized
    })
}

/// These become a folder under the data directory, an argument to git and part of an API route, so a name must be only a name.
fn plain_name(value: &str) -> Result<String, Failure> {
    static NAME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.][A-Za-z0-9_.-]*$").expect("a valid pattern"));
    if !NAME.is_match(value) || value.chars().all(|character| character == '.') {
        return Err(format!("Not a repository owner or name: {value}"));
    }
    Ok(value.to_owned())
}

/// The asked lines of a text.
fn slice_lines(text: &str, args: &Value) -> Value {
    let lines: Vec<&str> = text.split('\n').collect();
    let total = lines.len() as i64;
    let start = number_of(args.get("startLine")).unwrap_or(1.0).max(1.0) as i64;
    let end = number_of(args.get("endLine"))
        .map_or(total, |end| end as i64)
        .min(total);
    let from = (start - 1).min(total) as usize;
    let to = end.max(0) as usize;
    let selected = if from < to {
        lines[from..to].join("\n")
    } else {
        String::new()
    };
    json!({ "text": selected, "startLine": start, "endLine": end, "totalLines": total })
}

/// The owner and name of a GitHub remote address.
fn parse_github_remote(url: &str) -> Option<(String, String)> {
    static REMOTE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:^|[/@])github\.com[:/]([^/]+)/([^/]+?)(?:\.git)?/?$")
            .expect("a valid pattern")
    });
    let found = REMOTE.captures(js::trim(url))?;
    Some((found[1].to_owned(), found[2].to_owned()))
}

/// The value at a path of keys, when it is there.
fn at<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in keys {
        current = current.get(*key)?;
    }
    Some(current)
}

/// An object of the fields that have values, as a field set to undefined is left out of JSON.
fn fields(items: &[(&str, Option<&Value>)]) -> Value {
    let mut object = Map::new();
    for (key, value) in items {
        if let Some(value) = value {
            object.insert((*key).to_owned(), (*value).clone());
        }
    }
    Value::Object(object)
}

fn items(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn truthy(value: Option<&Value>) -> bool {
    js::truthy(value)
}

/// A pull request in the app's shape.
fn pull(pr: &Value) -> Value {
    let merged = truthy(pr.get("merged_at"));
    let merged_state = json!("merged");
    let mut summary = fields(&[
        ("number", pr.get("number")),
        ("title", pr.get("title")),
        (
            "state",
            if merged {
                Some(&merged_state)
            } else {
                pr.get("state")
            },
        ),
        ("draft", pr.get("draft")),
        ("author", at(pr, &["user", "login"])),
        ("head", at(pr, &["head", "ref"])),
        ("headSha", at(pr, &["head", "sha"])),
        ("base", at(pr, &["base", "ref"])),
        ("createdAt", pr.get("created_at")),
        ("updatedAt", pr.get("updated_at")),
        ("closedAt", pr.get("closed_at")),
        ("mergedAt", pr.get("merged_at")),
        ("url", pr.get("html_url")),
        ("body", pr.get("body")),
    ]);
    let names = |list: &Value, key: &str| -> Value {
        Value::Array(
            items(list)
                .iter()
                .filter_map(|item| item.get(key).cloned())
                .collect(),
        )
    };
    if truthy(pr.get("labels")) {
        summary["labels"] = names(&pr["labels"], "name");
    }
    if truthy(pr.get("requested_reviewers")) {
        summary["requestedReviewers"] = names(&pr["requested_reviewers"], "login");
    }
    if !items(pr.get("requested_teams").unwrap_or(&Value::Null)).is_empty() {
        summary["requestedTeams"] = names(&pr["requested_teams"], "slug");
    }
    if let Some(mergeable) = pr.get("mergeable") {
        summary["mergeable"] = mergeable.clone();
        if let Some(state) = pr.get("mergeable_state") {
            summary["mergeableState"] = state.clone();
        }
    }
    summary
}

fn numstat(output: &str) -> Value {
    Value::Array(
        output
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                let mut parts = line.split('\t');
                let count = |part: Option<&str>| match part {
                    Some("-") | None => Value::Null,
                    Some(number) => json!(number.parse::<i64>().unwrap_or(0)),
                };
                let additions = count(parts.next());
                let deletions = count(parts.next());
                json!({ "path": parts.next().unwrap_or(""), "additions": additions, "deletions": deletions })
            })
            .collect(),
    )
}

/// One tool's run: where the workspace is, and the token, looked up once.
pub(crate) struct Origin<'a> {
    kernel: &'a Kernel,
    repo_dir: Option<PathBuf>,
    token: tokio::sync::OnceCell<Option<String>>,
}

/// git failed, with what it said.
fn git_failure(stderr: &str, fallback: &str) -> Failure {
    let text = js::trim(if stderr.is_empty() { fallback } else { stderr });
    let lines: Vec<&str> = text.split('\n').collect();
    lines[lines.len().saturating_sub(3)..].join(" ")
}

impl<'a> Origin<'a> {
    pub(crate) fn new(kernel: &'a Kernel, repo_dir: Option<PathBuf>) -> Self {
        Self {
            kernel,
            repo_dir,
            token: tokio::sync::OnceCell::new(),
        }
    }

    /// Runs one of the tools.
    pub(crate) async fn run(&self, tool: &str, args: &Value) -> Answer {
        match tool {
            // Repository content: local git
            "get_file_contents" => self.get_file_contents(args).await,
            "get_git_tree" => self.get_git_tree(args).await,
            "grep_contents" => self.grep_contents(args).await,
            "list_branches" => self.list_branches(args).await,
            "list_commits" => self.list_commits(args).await,
            "commit_read" => self.commit_read(args).await,
            "compare_commits" => self.compare_commits(args).await,
            "sync_mirror" => self.sync_mirror(args).await,
            // Hosting: GitHub
            "get_repository" => self.get_repository(args).await,
            "list_repositories" => self.list_repositories(args).await,
            "list_namespaces" => self.list_namespaces(args).await,
            "create_repository" => self.create_repository(args).await,
            "list_pull_requests" => self.list_pull_requests(args).await,
            "pull_request_read" => self.pull_request_read(args).await,
            "checks_read" => self.checks_read(args).await,
            // Not offered to agents: the pull request follow-up reads a failed check's log tail with it.
            "check_run_log" => self.check_run_log(args).await,
            "create_pull_request" => self.create_pull_request(args).await,
            "update_pull_request" => self.update_pull_request(args).await,
            "update_pull_request_base" => self.update_pull_request_base(args).await,
            "merge_pull_request" => self.merge_pull_request(args).await,
            "create_pull_request_review" => self.create_review(args).await,
            "update_pull_request_review" => self.update_review(args).await,
            "dismiss_pull_request_review" => self.dismiss_review(args).await,
            "create_pull_request_comment" => self.create_comment(args).await,
            "create_pull_request_inline_comment" => self.create_inline_comment(args).await,
            "reply_pull_request_review_thread" => self.reply_thread(args).await,
            "update_pull_request_review_thread" => self.update_thread(args).await,
            "update_pull_request_comment" => self.update_comment(args).await,
            "update_pull_request_labels" => self.update_labels(args).await,
            "create_label" => self.create_label(args).await,
            "request_pull_request_reviewers" => self.reviewers(args, Method::POST).await,
            "remove_pull_request_reviewers" => self.reviewers(args, Method::DELETE).await,
            _ => Err(format!("Unknown origin tool: {tool}")),
        }
    }

    // git

    async fn git_with(
        &self,
        cwd: &Path,
        args: &[&str],
        allow_exit_one: bool,
        token: Option<&str>,
    ) -> Result<String, Failure> {
        let mut command = Command::new("git");
        workbench_tools::no_window(&mut command);
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(process_env())
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        if let Some(token) = token {
            // Lets git reach private GitHub repos with the same token, without putting it in a URL, on disk or in the
            // process list.
            let basic =
                base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
            command
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraheader")
                .env(
                    "GIT_CONFIG_VALUE_0",
                    format!("AUTHORIZATION: basic {basic}"),
                );
        }
        let output = command.output().await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "git is not installed or not on PATH".to_owned()
            } else {
                error.to_string()
            }
        })?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            return Ok(stdout);
        }
        // git grep exits 1 when nothing matched.
        if allow_exit_one && output.status.code() == Some(1) {
            return Ok(stdout);
        }
        Err(git_failure(
            &String::from_utf8_lossy(&output.stderr),
            "git failed",
        ))
    }

    async fn git(&self, cwd: &Path, args: &[&str]) -> Result<String, Failure> {
        self.git_with(cwd, args, false, None).await
    }

    fn repo_of(args: &Value) -> Result<(String, String), Failure> {
        let (owner, name) = (arg(args, "owner"), arg(args, "name"));
        if owner.is_empty() || name.is_empty() {
            return Err("owner and name are required".to_owned());
        }
        Ok((plain_name(owner)?, plain_name(name)?))
    }

    /// The local checkout for owner/name: the workspace repo if it is that repo, else a mirror clone made on demand.
    async fn checkout(&self, args: &Value) -> Result<PathBuf, Failure> {
        let (owner, name) = Self::repo_of(args)?;
        let workspace = self
            .repo_dir
            .clone()
            .or_else(|| std::env::var_os("WORKBENCH_ORIGIN_REPO").map(PathBuf::from))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        if let Ok(top) = self
            .git(&workspace, &["rev-parse", "--show-toplevel"])
            .await
        {
            let top = PathBuf::from(js::trim(&top));
            let remote = self
                .git(&top, &["remote", "get-url", "origin"])
                .await
                .unwrap_or_default();
            let parsed = parse_github_remote(js::trim(&remote));
            let folder = top
                .file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let local = owner.eq_ignore_ascii_case("local") && name.to_lowercase() == folder;
            let same = parsed.is_some_and(|(remote_owner, remote_name)| {
                remote_owner.eq_ignore_ascii_case(&owner) && remote_name.eq_ignore_ascii_case(&name)
            });
            if local || same {
                return Ok(top);
            }
        }
        if owner.eq_ignore_ascii_case("local") {
            return Err(format!("No local repository named {name}"));
        }
        let parent = self.kernel.store.root().join("origin").join(&owner);
        let mirror = parent.join(&name);
        if !mirror.join(".git").exists() {
            tokio::fs::create_dir_all(&parent)
                .await
                .map_err(|error| error.to_string())?;
            let address = format!("https://github.com/{owner}/{name}.git");
            let token = self.token().await;
            self.git_with(
                &parent,
                &[
                    "clone",
                    "--filter=blob:none",
                    "--no-checkout",
                    &address,
                    &name,
                ],
                false,
                token.as_deref(),
            )
            .await?;
        }
        Ok(mirror)
    }

    async fn default_ref(&self, dir: &Path) -> String {
        let head = self
            .git(
                dir,
                &[
                    "symbolic-ref",
                    "--quiet",
                    "--short",
                    "refs/remotes/origin/HEAD",
                ],
            )
            .await
            .unwrap_or_default();
        if !js::trim(&head).is_empty() {
            return js::trim(&head).to_owned();
        }
        let current = self
            .git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
            .await
            .unwrap_or_else(|_| "HEAD".to_owned());
        let current = js::trim(&current);
        if current.is_empty() { "HEAD" } else { current }.to_owned()
    }

    /// The ref asked for under `key`, or the default one, checked.
    async fn ref_or_default(
        &self,
        dir: &Path,
        args: &Value,
        key: &str,
        label: &str,
    ) -> Result<String, Failure> {
        let given = arg(args, key);
        if given.is_empty() {
            safe_ref(&self.default_ref(dir).await, label)
        } else {
            safe_ref(given, label)
        }
    }

    async fn get_file_contents(&self, args: &Value) -> Answer {
        let dir = self.checkout(args).await?;
        let reference = self.ref_or_default(&dir, args, "ref", "ref").await?;
        let file = safe_path(arg(args, "path"))?;
        let address = format!("{reference}:{file}");
        let kind = self
            .git(&dir, &["cat-file", "-t", &address])
            .await
            .unwrap_or_default();
        let kind = js::trim(&kind);
        if kind.is_empty() {
            let shown = if file.is_empty() { "/" } else { &file };
            return Err(format!("{shown} does not exist at {reference}"));
        }
        if kind == "tree" {
            let scope = if file.is_empty() {
                ".".to_owned()
            } else {
                format!("{file}/")
            };
            let listing = self
                .git(&dir, &["ls-tree", "--long", &reference, "--", &scope])
                .await?;
            let entries: Vec<Value> = listing
                .split('\n')
                .filter(|line| !line.is_empty())
                .map(|line| {
                    let (meta, entry) = line.split_once('\t').unwrap_or((line, ""));
                    let parts: Vec<&str> = meta.split_whitespace().collect();
                    let mut item = json!({
                        "path": entry,
                        "name": entry.rsplit('/').next().unwrap_or(entry),
                        "type": if parts.get(1) == Some(&"tree") { "dir" } else { "file" },
                        "sha": parts.get(2),
                    });
                    if let Some(size) = parts.get(3).filter(|size| **size != "-") {
                        item["size"] = json!(size.parse::<i64>().unwrap_or(0));
                    }
                    item
                })
                .collect();
            return Ok(
                json!({ "type": "dir", "ref": reference, "path": file, "entries": entries }),
            );
        }
        let sha = self.git(&dir, &["rev-parse", &address]).await?;
        let sha = js::trim(&sha);
        let content = self.git(&dir, &["show", &address]).await?;
        if content.contains('\0') {
            return Ok(
                json!({ "type": "file", "ref": reference, "path": file, "sha": sha, "binary": true, "size": js::length(&content) }),
            );
        }
        let sliced = slice_lines(&content, args);
        Ok(
            json!({ "type": "file", "ref": reference, "path": file, "sha": sha, "content": sliced["text"], "startLine": sliced["startLine"], "endLine": sliced["endLine"], "totalLines": sliced["totalLines"] }),
        )
    }

    async fn get_git_tree(&self, args: &Value) -> Answer {
        let dir = self.checkout(args).await?;
        let reference = self.ref_or_default(&dir, args, "sha", "sha").await?;
        let mut command = vec!["ls-tree"];
        if args.get("recursive") != Some(&Value::Bool(false)) {
            command.push("-r");
        }
        command.extend(["--long", &reference]);
        let listing = self.git(&dir, &command).await?;
        let tree: Vec<Value> = listing
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                let (meta, entry) = line.split_once('\t').unwrap_or((line, ""));
                let parts: Vec<&str> = meta.split_whitespace().collect();
                let mut item = json!({ "path": entry, "mode": parts.first(), "type": parts.get(1), "sha": parts.get(2) });
                if let Some(size) = parts.get(3).filter(|size| **size != "-") {
                    item["size"] = json!(size.parse::<i64>().unwrap_or(0));
                }
                item
            })
            .collect();
        let count = tree.len();
        Ok(
            json!({ "sha": reference, "tree": tree.into_iter().take(5000).collect::<Vec<_>>(), "count": count, "truncated": count > 5000 }),
        )
    }

    async fn grep_contents(&self, args: &Value) -> Answer {
        let dir = self.checkout(args).await?;
        let query = arg(args, "query");
        if query.is_empty() {
            return Err("query is required".to_owned());
        }
        let reference = self.ref_or_default(&dir, args, "ref", "ref").await?;
        let mut command: Vec<String> = ["grep", "-n", "-I", "--no-color", "--full-name"]
            .map(str::to_owned)
            .to_vec();
        for (key, flag) in [
            ("literal", "-F"),
            ("caseInsensitive", "-i"),
            ("wholeWord", "-w"),
        ] {
            if truthy(args.get(key)) {
                command.push(flag.to_owned());
            }
        }
        for (key, flag) in [("contextBefore", "-B"), ("contextAfter", "-A")] {
            let lines = number_of(args.get(key)).unwrap_or(0.0);
            if lines > 0.0 {
                command.push(flag.to_owned());
                command.push(lines.min(20.0).to_string());
            }
        }
        command.extend([
            "-e".to_owned(),
            query.to_owned(),
            reference.clone(),
            "--".to_owned(),
        ]);
        if !arg(args, "filterPath").is_empty() {
            command.push(safe_path(arg(args, "filterPath"))?);
        }
        for item in items(args.get("includes").unwrap_or(&Value::Null)) {
            command.push(format!(":(glob){}", js::text(Some(item))));
        }
        for item in items(args.get("excludes").unwrap_or(&Value::Null)) {
            command.push(format!(":(exclude,glob){}", js::text(Some(item))));
        }
        let command: Vec<&str> = command.iter().map(String::as_str).collect();
        let output = self.git_with(&dir, &command, true, None).await?;
        let max = number_of(args.get("maxResults"))
            .unwrap_or(100.0)
            .clamp(0.0, 500.0) as usize;
        let prefix = format!("{reference}:");
        let matches: Vec<(Value, bool)> = output
            .split('\n')
            .filter(|line| !line.is_empty() && *line != "--")
            .map(|line| {
                // "<ref>:<path>:<line>:<text>" for matches, "<ref>:<path>-<line>-<text>" for context lines.
                let rest = line.strip_prefix(&prefix).unwrap_or(line);
                match grep_line(rest) {
                    Some((path, number, text, context)) => (
                        json!({ "path": path, "line": number, "text": text, "context": context }),
                        context,
                    ),
                    None => (
                        json!({ "path": "", "line": 0, "text": rest, "context": true }),
                        true,
                    ),
                }
            })
            .collect();
        let total = matches.iter().filter(|(_, context)| !context).count();
        let truncated = matches.len() > max;
        let shown: Vec<Value> = matches
            .into_iter()
            .take(max)
            .map(|(item, _)| item)
            .collect();
        Ok(
            json!({ "ref": reference, "query": query, "matches": shown, "totalMatches": total, "truncated": truncated }),
        )
    }

    async fn list_branches(&self, args: &Value) -> Answer {
        let dir = self.checkout(args).await?;
        let output = self
            .git(
                &dir,
                &[
                    "for-each-ref",
                    "--format=%(refname)%09%(objectname)%09%(committerdate:iso-strict)",
                    "refs/heads",
                    "refs/remotes/origin",
                ],
            )
            .await?;
        let mut seen: Vec<(String, Value)> = Vec::new();
        for line in output.split('\n').filter(|line| !line.is_empty()) {
            let parts: Vec<&str> = line.split('\t').collect();
            let reference = parts.first().copied().unwrap_or("");
            let name = reference
                .strip_prefix("refs/heads/")
                .or_else(|| reference.strip_prefix("refs/remotes/origin/"))
                .unwrap_or(reference);
            if name != "HEAD" && !seen.iter().any(|(known, _)| known == name) {
                seen.push((
                    name.to_owned(),
                    json!({ "name": name, "sha": parts.get(1), "updatedAt": parts.get(2) }),
                ));
            }
        }
        let prefix = arg(args, "prefix");
        let all: Vec<Value> = seen
            .into_iter()
            .filter(|(name, _)| prefix.is_empty() || name.starts_with(prefix))
            .map(|(_, branch)| branch)
            .collect();
        let (size, at) = (page_size(args, 30.0), page(args));
        let count = all.len() as i64;
        let shown: Vec<Value> = all
            .into_iter()
            .skip(((at - 1) * size) as usize)
            .take(size as usize)
            .collect();
        let next = if at * size < count {
            json!((at + 1).to_string())
        } else {
            Value::Null
        };
        Ok(json!({ "branches": shown, "totalCount": count, "nextPageToken": next }))
    }

    async fn list_commits(&self, args: &Value) -> Answer {
        let dir = self.checkout(args).await?;
        let reference = self.ref_or_default(&dir, args, "ref", "ref").await?;
        let (size, at) = (page_size(args, 20.0), page(args));
        let mut command = vec![
            "log".to_owned(),
            format!("--max-count={}", size + 1),
            format!("--skip={}", (at - 1) * size),
            "--format=%H%x1f%s%x1f%an%x1f%ae%x1f%aI%x1f%P".to_owned(),
        ];
        for email in items(args.get("authorEmails").unwrap_or(&Value::Null)) {
            command.push(format!("--author={}", js::text(Some(email))));
        }
        for email in items(args.get("committerEmails").unwrap_or(&Value::Null)) {
            command.push(format!("--committer={}", js::text(Some(email))));
        }
        if !arg(args, "since").is_empty() {
            command.push(format!("--since={}", arg(args, "since")));
        }
        if !arg(args, "until").is_empty() {
            command.push(format!("--until={}", arg(args, "until")));
        }
        command.extend([reference.clone(), "--".to_owned()]);
        let command: Vec<&str> = command.iter().map(String::as_str).collect();
        let output = self.git(&dir, &command).await?;
        let mut commits: Vec<Value> = output
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                let parts: Vec<&str> = line.split('\u{1f}').collect();
                let part = |index: usize| parts.get(index).copied().unwrap_or("");
                json!({
                    "sha": part(0),
                    "message": part(1),
                    "author": { "name": part(2), "email": part(3) },
                    "date": part(4),
                    "parents": parents_of(part(5)),
                })
            })
            .collect();
        let more = commits.len() as i64 > size;
        commits.truncate(size as usize);
        let next = if more {
            json!((at + 1).to_string())
        } else {
            Value::Null
        };
        Ok(json!({ "ref": reference, "commits": commits, "nextPageToken": next }))
    }

    async fn commit_read(&self, args: &Value) -> Answer {
        let dir = self.checkout(args).await?;
        let sha = safe_ref(arg(args, "sha"), "sha")?;
        let meta = self
            .git(
                &dir,
                &[
                    "show",
                    "-s",
                    "--format=%H%x1f%an%x1f%ae%x1f%aI%x1f%cn%x1f%cI%x1f%P%x1f%B",
                    &sha,
                ],
            )
            .await?;
        let parts: Vec<&str> = meta.split('\u{1f}').collect();
        let part = |index: usize| parts.get(index).copied().unwrap_or("");
        let mut metadata = json!({
            "sha": js::trim(part(0)),
            "author": { "name": part(1), "email": part(2) },
            "date": part(3),
            "committer": { "name": part(4), "date": part(5) },
            "parents": parents_of(part(6)),
            "message": js::trim(part(7)),
        });
        match arg(args, "view") {
            "files" => {
                let stat = self
                    .git(
                        &dir,
                        &["show", "--format=", "--numstat", "--find-renames", &sha],
                    )
                    .await?;
                metadata["files"] = numstat(&stat);
            }
            "patch" => {
                let patch = self
                    .git(
                        &dir,
                        &["show", "--format=", "--patch", "--find-renames", &sha],
                    )
                    .await?;
                let sliced = slice_lines(js::prefix(&patch, MAX_OUTPUT), args);
                metadata["patch"] = sliced["text"].clone();
                metadata["startLine"] = sliced["startLine"].clone();
                metadata["endLine"] = sliced["endLine"].clone();
                metadata["totalLines"] = sliced["totalLines"].clone();
            }
            _ => {}
        }
        Ok(metadata)
    }

    async fn compare_commits(&self, args: &Value) -> Answer {
        let dir = self.checkout(args).await?;
        let base = safe_ref(arg(args, "base"), "base")?;
        let head = safe_ref(arg(args, "head"), "head")?;
        let merge_base = self
            .git(&dir, &["merge-base", &base, &head])
            .await
            .unwrap_or_default();
        let merge_base = js::trim(&merge_base);
        let counts = self
            .git(
                &dir,
                &[
                    "rev-list",
                    "--left-right",
                    "--count",
                    &format!("{base}...{head}"),
                ],
            )
            .await?;
        let counts: Vec<i64> = counts
            .split_whitespace()
            .map(|count| count.parse().unwrap_or(0))
            .collect();
        let log = self
            .git(
                &dir,
                &[
                    "log",
                    "--format=%H%x1f%s%x1f%an%x1f%aI",
                    "--max-count=250",
                    &format!("{base}..{head}"),
                ],
            )
            .await?;
        let stat = self
            .git(
                &dir,
                &[
                    "diff",
                    "--numstat",
                    "--find-renames",
                    &format!("{base}...{head}"),
                ],
            )
            .await?;
        let commits: Vec<Value> = log
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                let parts: Vec<&str> = line.split('\u{1f}').collect();
                json!({ "sha": parts.first(), "message": parts.get(1), "author": parts.get(2), "date": parts.get(3) })
            })
            .collect();
        Ok(json!({
            "base": base,
            "head": head,
            "mergeBase": if merge_base.is_empty() { Value::Null } else { json!(merge_base) },
            "behindBy": counts.first().copied().unwrap_or(0),
            "aheadBy": counts.get(1).copied().unwrap_or(0),
            "commits": commits,
            "files": numstat(&stat),
        }))
    }

    async fn sync_mirror(&self, args: &Value) -> Answer {
        let dir = self.checkout(args).await?;
        let reference = safe_ref(arg(args, "ref"), "ref")?;
        let token = self.token().await;
        self.git_with(
            &dir,
            &["fetch", "--prune", "origin", &reference],
            false,
            token.as_deref(),
        )
        .await?;
        let fetched = self.git(&dir, &["rev-parse", "FETCH_HEAD"]).await?;
        let fetched = js::trim(&fetched);
        let expected = arg(args, "sha");
        if !expected.is_empty() && !fetched.starts_with(expected) {
            return Err(format!("{reference} is at {fetched}, not {expected}"));
        }
        Ok(
            json!({ "synced": true, "ref": reference, "sha": fetched, "checkout": dir.to_string_lossy() }),
        )
    }
}

fn number_value(number: f64) -> Value {
    if number.fract() == 0.0 {
        json!(number as i64)
    } else {
        json!(number)
    }
}

fn next_page(found: usize, size: i64, at_page: i64) -> Value {
    if found as i64 == size {
        json!((at_page + 1).to_string())
    } else {
        Value::Null
    }
}

fn parents_of(text: &str) -> Value {
    if text.is_empty() {
        json!([])
    } else {
        json!(text.split(' ').collect::<Vec<_>>())
    }
}

/// A `git grep` line without its ref: the path, the line number, the text, and whether it is only context. The separator
/// (":" for a match, "-" for context) is the first one that has a number and the same separator after it.
fn grep_line(rest: &str) -> Option<(&str, i64, &str, bool)> {
    for (index, separator) in rest.char_indices() {
        if separator != ':' && separator != '-' {
            continue;
        }
        let after = &rest[index + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with(separator) {
            let number = after[..digits].parse().ok()?;
            return Some((
                &rest[..index],
                number,
                &after[digits + 1..],
                separator == '-',
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_component_is_encoded_as_encode_uri_component_does() {
        assert_eq!(encode("a/b c?d&e=f"), "a%2Fb%20c%3Fd%26e%3Df");
        assert_eq!(encode("keep-_.!~*'()AZaz09"), "keep-_.!~*'()AZaz09");
        assert_eq!(encode("é日\u{1F600}"), "%C3%A9%E6%97%A5%F0%9F%98%80");
    }

    #[test]
    fn a_grep_line_splits_at_its_first_numbered_separator() {
        assert_eq!(
            grep_line("a/b.ts:3:x:4:y"),
            Some(("a/b.ts", 3, "x:4:y", false))
        );
        assert_eq!(grep_line("a-b.ts-3-x"), Some(("a-b.ts", 3, "x", true)));
        assert_eq!(grep_line("no numbers"), None);
    }

    #[test]
    fn paths_stay_inside_the_repository() {
        assert_eq!(safe_path("./src//a.ts").unwrap(), "src/a.ts");
        assert_eq!(safe_path("src/../a.ts").unwrap(), "a.ts");
        assert_eq!(safe_path("src\\a.ts").unwrap(), "src/a.ts");
        assert_eq!(safe_path("").unwrap(), "");
        for outside in ["../a", "..", "/etc/passwd", "a/../../b"] {
            assert!(safe_path(outside).is_err(), "{outside}");
        }
    }

    #[test]
    fn a_name_is_only_a_name() {
        assert!(plain_name("acme-site.v2_x").is_ok());
        for bad in ["..", "...", "a/b", "../x", "-x", "--upload-pack=y", ""] {
            assert!(plain_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_github_remote_is_read_only_from_github() {
        let acme = Some(("acme".to_owned(), "widgets".to_owned()));
        assert_eq!(
            parse_github_remote("https://github.com/acme/widgets.git"),
            acme
        );
        assert_eq!(parse_github_remote("git@github.com:acme/widgets.git"), acme);
        assert_eq!(
            parse_github_remote("ssh://git@github.com/acme/widgets"),
            acme
        );
        assert_eq!(
            parse_github_remote("https://notgithub.com/acme/widgets.git"),
            None
        );
        assert_eq!(
            parse_github_remote("https://gitlab.com/acme/widgets.git"),
            None
        );
    }
}
