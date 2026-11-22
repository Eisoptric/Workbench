//! Workspace terminal, file browser, in-app browser proxy, and OAuth callback handlers.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::header::{CONTENT_TYPE, HeaderName};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use workbench_core::projects::{launch_app, launch_terminal};
use workbench_core::{CoreError, Kernel};
use workbench_domain::js;
use workbench_domain::skills::public_address::parse_public_address;
use workbench_telemetry::{Kind, Span};
use workbench_tools::public_host::{SystemLookup, fetch_public};
use workbench_tools::run_captured;
use workbench_tools::sandbox::Platform;
use workbench_tools::shell::{process_env, shell_invocation};

use crate::routes::{Answer, Body, Failure, Shared, body_of, given, ok};
use crate::routes_admin::wanted;
use crate::terminal_sessions;

type QueryText = Query<HashMap<String, String>>;

const CWD_MARKER: &str = "__WORKBENCH_CWD__:";
/// Sent into every page the in-app browser shows: link clicks and GET forms ask the app to load the next page through the proxy,
/// since the sandboxed frame cannot load most sites itself.
const BROWSER_NAVIGATION_SCRIPT: &str = include_str!("browser-navigation.html");

async fn active_root(kernel: &Kernel) -> Result<std::path::PathBuf, Failure> {
    let root = match kernel.active_project().await {
        Ok(project)
            if project
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(|path| !path.is_empty()) =>
        {
            std::path::PathBuf::from(project["path"].as_str().unwrap_or(""))
        }
        _ => kernel.store.root().join("workspace"),
    };
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(CoreError::from)?;
    Ok(root)
}

pub(crate) async fn workspace_info(State(kernel): Shared) -> Answer {
    let root = active_root(&kernel).await?;
    let shell = std::env::var("SHELL")
        .unwrap_or_else(|_| if cfg!(windows) { "cmd.exe" } else { "/bin/zsh" }.to_owned());
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_owned());
    let host = std::env::var("HOSTNAME")
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(|| {
            let mut command = std::process::Command::new("hostname");
            workbench_tools::no_window_blocking(&mut command);
            let output = command.output().ok()?;
            Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        });
    let host = host.unwrap_or_default();
    ok(
        json!({ "cwd": root.to_string_lossy(), "shell": shell, "user": user, "hostname": host.strip_suffix(".local").unwrap_or(&host) }),
    )
}

/// Splits the working folder the wrapped command printed from its output.
fn extract_cwd(output: &str, fallback: &str) -> (String, String) {
    let Some(index) = output.rfind(CWD_MARKER) else {
        return (output.to_owned(), fallback.to_owned());
    };
    let before = &output[..index];
    let before = before
        .strip_suffix('\n')
        .map_or(before, |text| text.strip_suffix('\r').unwrap_or(text));
    let after = output[index + CWD_MARKER.len()..]
        .lines()
        .next()
        .map_or("", js::trim);
    (
        before.to_owned(),
        if after.is_empty() { fallback } else { after }.to_owned(),
    )
}

/// A command from the app's own terminal panel, which the user typed.
pub(crate) async fn workspace_terminal(State(kernel): Shared, body: Body) -> Answer {
    let root = active_root(&kernel).await?;
    let body = body_of(body);
    let command = js::trim(given(&body, "command").unwrap_or(""));
    let asked = js::trim(given(&body, "cwd").unwrap_or(""));
    let cwd = if !asked.is_empty()
        && tokio::fs::metadata(asked)
            .await
            .is_ok_and(|info| info.is_dir())
    {
        std::path::PathBuf::from(asked)
    } else {
        root
    };
    let shown = cwd.to_string_lossy().into_owned();
    if command.is_empty() {
        return ok(json!({ "stdout": "", "stderr": "", "exitCode": 0, "cwd": shown }));
    }
    // A panel that names itself keeps its shell's environment, functions and aliases from one command to the next.
    let session = given(&body, "session")
        .and_then(terminal_sessions::session_id)
        .filter(|_| !cfg!(windows))
        .map(str::to_owned);
    let kept = session.as_deref().and_then(terminal_sessions::state_of);
    let markers = terminal_sessions::Markers::new();
    let prelude = kept
        .as_ref()
        .filter(|state| !state.definitions.trim().is_empty())
        .map(|state| state.definitions.clone());
    let wrapped = if cfg!(windows) {
        // cmd reads %CD% when it reads the line, before the command has run, so a plain `echo %CD%` named the folder
        // the command started in, and the line's exit code was echo's. `%^CD%` is no variable at that first reading;
        // `call` reads the line again once the command is done, and the last `call` gives the command's own code back.
        format!(
            "{command} & call set __workbench_ec=%^ERRORLEVEL%& call echo {CWD_MARKER}%^CD%& call exit /b %^__workbench_ec%"
        )
    } else {
        // The snapshot comes before the kept definitions, so those count as set by the panel and are kept again.
        let mut before = String::new();
        if session.is_some() {
            before.push_str(terminal_sessions::SNAPSHOT);
            before.push('\n');
        }
        if prelude.is_some() {
            before.push_str(terminal_sessions::PRELUDE);
            before.push('\n');
        }
        let after = if session.is_some() {
            format!("{}\n", terminal_sessions::trailer(&markers))
        } else {
            String::new()
        };
        format!(
            "{before}{command}\n__workbench_ec=$?\nprintf \"\\n{CWD_MARKER}%s\\n\" \"$(pwd)\"\n{after}exit $__workbench_ec"
        )
    };
    let env = process_env();
    let shell = env.get(std::ffi::OsStr::new("SHELL")).map_or_else(
        || "/bin/zsh".to_owned(),
        |shell| shell.to_string_lossy().into_owned(),
    );
    let invocation = shell_invocation(&wrapped, &shell, Platform::current(), &env);
    let fresh = kept.is_none();
    let mut variables: Vec<_> = match kept {
        Some(state) => state.environment,
        None => env.into_iter().collect(),
    };
    if fresh {
        variables.push(("TERM".into(), "xterm-256color".into()));
    }
    if fresh && !cfg!(windows) {
        for item in &mut variables {
            if item.0 == "PATH" {
                item.1 = format!(
                    "{}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
                    item.1.to_string_lossy()
                )
                .into();
            }
        }
    }
    // Only the program name is recorded; arguments may carry secrets.
    let span = Span::start(
        "workspace.exec",
        Kind::Internal,
        json!({
            "process.executable.name": shell.rsplit(['/', '\\']).next().unwrap_or_default(),
            "workbench.command.name": command.split_whitespace().next().unwrap_or_default(),
        }),
    );
    let result = span
        .scope(run_captured(
            &invocation,
            &cwd,
            &variables,
            prelude.as_deref().unwrap_or_default().as_bytes(),
            Duration::from_secs(30),
            512 * 1024,
        ))
        .await;
    let output = match (
        session.as_deref(),
        terminal_sessions::split_state(&result.stdout, &markers),
    ) {
        (Some(session), Some((before, state))) => {
            terminal_sessions::keep(session, state);
            before
        }
        _ => result.stdout.as_str(),
    };
    let (stdout, final_cwd) = extract_cwd(output, &shown);
    let code = result.code.unwrap_or(1);
    span.set("process.exit.code", code);
    if code != 0 {
        span.set("error.type", "nonzero_exit");
    }
    workbench_telemetry::add(
        "workbench.workspace.commands",
        1.0,
        json!({ "workbench.command.outcome": if code == 0 { "ok" } else { "failed" } }),
    );
    let stderr = if code != 0 && result.stderr.is_empty() {
        "Command failed".to_owned()
    } else {
        result.stderr
    };
    ok(json!({ "stdout": stdout, "stderr": stderr, "exitCode": code, "cwd": final_cwd }))
}

pub(crate) async fn open_terminal(State(kernel): Shared, body: Body) -> Answer {
    let root = active_root(&kernel).await?;
    let body = body_of(body);
    let asked = js::trim(given(&body, "cwd").unwrap_or(""));
    // Absolute, so a folder cannot be read as one of the program's options.
    let cwd = if asked.is_empty() {
        root
    } else {
        std::path::absolute(asked).map_err(CoreError::from)?
    };
    launch_terminal(&cwd);
    ok(json!({ "opened": true, "cwd": cwd.to_string_lossy() }))
}

/// What the user typed in the address bar as a URL: a link as it is, a bare name with a scheme, anything else a search.
fn normalize_url(input: &str) -> String {
    let trimmed = js::trim(input);
    if trimmed.is_empty() {
        return "https://example.com".to_owned();
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return trimmed.to_owned();
    }
    let host = trimmed.split('/').next().unwrap_or("");
    let bare = host.split(':').next().unwrap_or("");
    if matches!(
        bare.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1"
    ) && host
        .split(':')
        .nth(1)
        .is_none_or(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return format!("http://{trimmed}");
    }
    let label = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    };
    if host.contains('.') && host.split('.').all(label) {
        return format!("https://{trimmed}");
    }
    let mut search = url::Url::parse("https://duckduckgo.com/").expect("this address is valid");
    search.query_pairs_mut().append_pair("q", trimmed);
    search.to_string()
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(crate) async fn open_browser(body: Body) -> Answer {
    let body = body_of(body);
    let url = normalize_url(given(&body, "url").unwrap_or("https://www.google.com"));
    if cfg!(target_os = "macos") {
        launch_app("the browser", "open", &[&url], None);
    } else if cfg!(windows) {
        // Not through cmd, which would read `&` in an address as the start of a second command: Windows' own handler
        // takes the address as one argument, as the desktop app's links do.
        launch_app(
            "the browser",
            "rundll32",
            &["url.dll,FileProtocolHandler", &url],
            None,
        );
    } else {
        launch_app("the browser", "xdg-open", &[&url], None);
    }
    ok(json!({ "opened": true, "url": url }))
}

/// One page for the in-app browser. Redirects are followed by hand, so every hop gets the public-address check (at most four).
async fn proxied_page(target: &str) -> Result<(String, String), String> {
    let mut current = parse_public_address(target)?;
    let headers = [
        (
            "user-agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        ),
        (
            "accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        ),
    ];
    let mut hop = 0;
    let response = loop {
        let response = fetch_public(&current, &headers, Arc::new(SystemLookup)).await?;
        let location = response
            .status()
            .is_redirection()
            .then(|| {
                response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned)
            })
            .flatten();
        let Some(location) = location else {
            break response;
        };
        if hop == 4 {
            return Err("Too many redirects".to_owned());
        }
        hop += 1;
        let next = current.join(&location).map_err(|error| error.to_string())?;
        current = parse_public_address(next.as_str())?;
    };
    let final_url = current.to_string();
    let kind = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("text/html; charset=utf-8")
        .to_owned();
    let base = format!(
        "<base href=\"{}\">{BROWSER_NAVIGATION_SCRIPT}",
        escape_attribute(&final_url)
    );
    let page = if kind.contains("html") {
        let body = response
            .text()
            .await
            .map_err(|error| error.without_url().to_string())?;
        let head = regex::Regex::new("(?i)<head[^>]*>").expect("this pattern is valid");
        match head.find(&body) {
            Some(found) => format!("{}{base}{}", &body[..found.end()], &body[found.end()..]),
            None => format!("{base}{body}"),
        }
    } else if kind.starts_with("image/") {
        format!(
            "<!doctype html><html><head>{base}</head><body style=\"margin:0;display:grid;place-items:center;min-height:100vh;background:#111\"><img src=\"{}\" style=\"max-width:100%\"></body></html>",
            escape_attribute(&final_url)
        )
    } else {
        let body = response
            .text()
            .await
            .map_err(|error| error.without_url().to_string())?;
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\">{base}</head><body style=\"margin:0\"><pre style=\"white-space:pre-wrap;word-break:break-word;padding:16px;font:13px ui-monospace,monospace\">{}</pre></body></html>",
            escape_attribute(js::prefix(&body, 2_000_000))
        )
    };
    Ok((final_url, page))
}

/// The in-app browser's page loader. Public addresses only, like the agents' web tool: this runs as the user, and a page shown
/// here could otherwise read anything on localhost or the private network, the runtime included.
/// Where a provider sends the browser back after an MCP server's sign-in. The browser carries no token, so the route is
/// open: it finishes only a sign-in whose random state the app started and has not finished, exchanging the code with
/// that sign-in's PKCE verifier, and answers with a page that runs nothing and links nowhere.
pub(crate) async fn oauth_callback(State(kernel): Shared, Query(query): QueryText) -> Response {
    let given = |name: &str| query.get(name).map(String::as_str).unwrap_or("");
    let (title, message) = if !given("error").is_empty() {
        let said = if given("error_description").is_empty() {
            given("error")
        } else {
            given("error_description")
        };
        (
            "Sign-in was not completed",
            format!("The server said: {said}. Start it again from Settings."),
        )
    } else {
        match kernel
            .mcp
            .finish_sign_in(given("state"), given("code"))
            .await
        {
            Ok(target) => {
                let host = url::Url::parse(&target.url)
                    .ok()
                    .and_then(|address| address.host_str().map(str::to_owned))
                    .unwrap_or_default();
                workbench_core::logger::log(
                    "mcp",
                    "info",
                    &format!("Signed in to the MCP server at {host}"),
                );
                (
                    "Signed in",
                    format!("The app is signed in to {host}. You can close this tab."),
                )
            }
            Err(error) => ("Sign-in was not completed", error.to_string()),
        }
    };
    let page = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>{}</title><body style=\"font:15px system-ui;margin:3em\"><h1 style=\"font-size:18px\">{}</h1><p>{}</p>",
        escape_attribute(title),
        escape_attribute(title),
        escape_attribute(&message)
    );
    (
        [
            (CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                HeaderName::from_static("content-security-policy"),
                "default-src 'none'; style-src 'unsafe-inline'",
            ),
            (HeaderName::from_static("referrer-policy"), "no-referrer"),
        ],
        page,
    )
        .into_response()
}

pub(crate) async fn browser_proxy(Query(query): QueryText) -> Response {
    let target = normalize_url(wanted(&query, "url").unwrap_or("https://example.com"));
    let html: HeaderName = CONTENT_TYPE;
    let loading = tokio::time::timeout(Duration::from_secs(12), proxied_page(&target));
    match loading
        .await
        .unwrap_or_else(|_| Err("The page took too long to load".to_owned()))
    {
        Ok((url, page)) => (
            [
                (html, "text/html; charset=utf-8".to_owned()),
                (HeaderName::from_static("x-eisoptric-url"), url),
            ],
            page,
        )
            .into_response(),
        Err(message) => {
            workbench_core::log(
                "workspace",
                "warning",
                &format!("Browser preview could not load a page: {message}"),
            );
            let page = format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"><style>body{{font-family:-apple-system,sans-serif;background:#111318;color:#e5e7eb;padding:32px;line-height:1.5}}code{{background:#1f2430;padding:2px 6px;border-radius:4px;color:#93c5fd}}</style></head><body><h3>Could not load page</h3><p><code>{}</code></p><p>{}</p></body></html>",
                escape_attribute(&target),
                escape_attribute(&message)
            );
            ([(html, "text/html; charset=utf-8")], page).into_response()
        }
    }
}

/// A file or folder in the active project for the file view. A symbolic link counts as where it points: a cloned repository can
/// bring one that leads out of the folder.
pub(crate) async fn workspace_files(State(kernel): Shared, Query(query): QueryText) -> Answer {
    let root = active_root(&kernel).await?;
    let asked = js::trim(wanted(&query, "path").unwrap_or(""));
    let workspace = workbench_domain::skills::workspace::Workspace::new(&root);
    let target = match workbench_tools::file::inside_workspace(
        &workspace,
        if asked.is_empty() { "." } else { asked },
    )
    .await
    {
        Ok(target) => target,
        Err(_) => {
            workbench_core::log(
                "workspace",
                "warning",
                "Refused a file path outside the workspace",
            );
            workspace.root().to_path_buf()
        }
    };
    let base = workspace.root().to_path_buf();
    let shown = |path: &std::path::Path| {
        path.strip_prefix(&base)
            .map_or(String::new(), |item| item.to_string_lossy().into_owned())
    };
    let info = tokio::fs::metadata(&target)
        .await
        .map_err(CoreError::from)?;
    if info.is_dir() {
        let mut entries = tokio::fs::read_dir(&target)
            .await
            .map_err(CoreError::from)?;
        let mut items = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let folder = entry.file_type().await.is_ok_and(|kind| kind.is_dir());
            items.push(json!({ "name": name, "path": shown(&target.join(&name)), "kind": if folder { "directory" } else { "file" } }));
        }
        let place = shown(&target);
        return ok(
            json!({ "kind": "directory", "path": if place.is_empty() { ".".to_owned() } else { place }, "entries": items }),
        );
    }
    let content = tokio::fs::read(&target).await.map_err(CoreError::from)?;
    let content = String::from_utf8_lossy(&content);
    ok(json!({ "kind": "file", "path": shown(&target), "content": js::prefix(&content, 100_000) }))
}
