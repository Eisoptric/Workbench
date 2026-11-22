//! Settings, connections, projects, usage, plugins, programs, and admin routing table.
use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use serde_json::{Value, json};
use workbench_core::agent_programs::program_id;
use workbench_core::catalog::llm_providers;
use workbench_core::skill_files::{list_skills, load_skill, skill_json};
use workbench_core::{CoreError, Kernel};
use workbench_domain::js;

use crate::routes::{Answer, Body, Failure, Shared, body_of, created, given, layers_json, ok};
use crate::routes_telemetry::{
    cli_status, computer_permissions, install_cli, network_view, open_permission_pane,
    telemetry_clear, telemetry_export, telemetry_logs, telemetry_metrics, telemetry_status,
    telemetry_trace, telemetry_traces,
};
use crate::routes_workspace::{
    browser_proxy, oauth_callback, open_browser, open_terminal, workspace_files, workspace_info,
    workspace_terminal,
};

type QueryText = Query<HashMap<String, String>>;

pub(crate) fn no_content() -> Answer {
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(crate) fn wanted<'a>(query: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    query
        .get(key)
        .map(String::as_str)
        .filter(|text| !text.is_empty())
}

async fn settings(State(kernel): Shared) -> Answer {
    ok(kernel.settings())
}

async fn update_settings(State(kernel): Shared, body: Body) -> Answer {
    ok(kernel.update_settings(&body_of(body)).await?)
}

async fn information(State(kernel): Shared) -> Answer {
    ok(kernel.information().await)
}

async fn activity(State(kernel): Shared) -> Answer {
    ok(kernel.activity())
}

async fn sandbox() -> Answer {
    ok(Kernel::sandbox_status())
}

async fn set_up_sandbox(State(kernel): Shared) -> Answer {
    ok(kernel.set_up_sandbox().await?)
}

async fn logs(Query(query): QueryText) -> Answer {
    let text = |key: &str| wanted(&query, key).unwrap_or("");
    ok(Kernel::list_logs(text("service"), text("level"), text("q")))
}

async fn snapshots(State(kernel): Shared) -> Answer {
    ok(kernel.snapshot_folders().await)
}

async fn delete_snapshots(State(kernel): Shared) -> Answer {
    kernel.delete_snapshot_folders(None).await?;
    ok(json!({ "deleted": true }))
}

async fn delete_snapshot(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    kernel.delete_snapshot_folders(Some(&id)).await?;
    ok(json!({ "deleted": true }))
}

async fn read_memory(State(kernel): Shared, Path(scope): Path<String>) -> Answer {
    ok(json!({ "scope": scope, "content": kernel.read_memory(&scope).await }))
}

async fn write_memory(State(kernel): Shared, Path(scope): Path<String>, body: Body) -> Answer {
    let content = js::text(body_of(body).get("content"));
    kernel
        .write_memory(&scope, &content)
        .await
        .map_err(CoreError::from)?;
    ok(json!({ "scope": scope, "content": kernel.read_memory(&scope).await }))
}

async fn usage(State(kernel): Shared) -> Answer {
    ok(kernel.usage_records())
}

async fn usage_summary(State(kernel): Shared, Query(query): QueryText) -> Answer {
    ok(kernel.usage_summary(wanted(&query, "from"), wanted(&query, "to"))?)
}

async fn providers() -> Answer {
    ok(llm_providers())
}

/// The route keeps the path it had when the local server could only be LM Studio.
async fn local_server(State(kernel): Shared, Query(query): QueryText) -> Answer {
    ok(kernel
        .local_server_models(wanted(&query, "baseUrl").unwrap_or(""))
        .await)
}

async fn provider_models(
    State(kernel): Shared,
    Path(provider): Path<String>,
    Query(query): QueryText,
) -> Answer {
    ok(kernel
        .provider_models(&provider, wanted(&query, "baseUrl").unwrap_or(""), "")
        .await?)
}

/// The same list for a key typed in the settings form and not saved yet, sent in the body so that it is in no address.
async fn provider_models_for_key(
    State(kernel): Shared,
    Path(provider): Path<String>,
    body: Body,
) -> Answer {
    let input = body_of(body);
    let field = |name: &str| input.get(name).and_then(Value::as_str).unwrap_or("");
    ok(kernel
        .provider_models(&provider, field("baseUrl"), field("secret"))
        .await?)
}

async fn list_connections(State(kernel): Shared, Path(kind): Path<String>) -> Answer {
    ok(kernel.list_connections(&kind).await?)
}

/// Removes a connection. With `moveAgentsTo` (a model's) or `releaseAgents` (a server's or index's) its agents let go of
/// it first; without either, a connection still in use is refused.
async fn remove_connection(
    kernel: &Kernel,
    kind: &str,
    id: &str,
    query: &HashMap<String, String>,
) -> Result<(), CoreError> {
    let move_to = query.get("moveAgentsTo").map(String::as_str);
    if move_to.is_some()
        || query
            .get("releaseAgents")
            .is_some_and(|value| value == "true")
    {
        kernel.release_connection(kind, id, move_to).await
    } else {
        kernel.delete_connection(kind, id).await
    }
}

async fn delete_connection(
    State(kernel): Shared,
    Path((kind, id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
) -> Answer {
    remove_connection(&kernel, &kind, &id, &query).await?;
    no_content()
}

async fn list_mcp(State(kernel): Shared) -> Answer {
    ok(kernel.list_connections("mcp").await?)
}

async fn list_rag(State(kernel): Shared) -> Answer {
    ok(kernel.list_connections("rag").await?)
}

async fn delete_model(
    State(kernel): Shared,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Answer {
    remove_connection(&kernel, "models", &id, &query).await?;
    no_content()
}

async fn delete_mcp_connection(
    State(kernel): Shared,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Answer {
    remove_connection(&kernel, "mcp", &id, &query).await?;
    no_content()
}

async fn delete_rag(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    kernel.delete_connection("rag", &id).await?;
    no_content()
}

async fn update_model(State(kernel): Shared, Path(id): Path<String>, body: Body) -> Answer {
    ok(kernel.update_model(&id, &body_of(body)).await?)
}

async fn create_mcp(State(kernel): Shared, body: Body) -> Answer {
    created(kernel.create_mcp(&body_of(body)).await?)
}

async fn update_mcp(State(kernel): Shared, Path(id): Path<String>, body: Body) -> Answer {
    ok(kernel.update_mcp(&id, &body_of(body)).await?)
}

async fn approve_mcp(State(kernel): Shared, Path(id): Path<String>, body: Body) -> Answer {
    let fingerprint = js::text(body_of(body).get("fingerprint"));
    ok(kernel.approve_mcp(&id, &fingerprint).await?)
}

async fn discover_mcp(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    ok(kernel.discover_mcp(&id).await?)
}

async fn delete_mcp(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    kernel.delete_mcp_server(&id).await?;
    no_content()
}

async fn create_rag(State(kernel): Shared, body: Body) -> Answer {
    created(kernel.create_rag(&body_of(body)).await?)
}

async fn update_rag(State(kernel): Shared, Path(id): Path<String>, body: Body) -> Answer {
    ok(kernel.update_rag(&id, &body_of(body)).await?)
}

async fn hook_sets(State(kernel): Shared, Query(query): QueryText) -> Answer {
    let sets = kernel.hook_sets(wanted(&query, "projectId")).await;
    ok(sets
        .iter()
        .map(workbench_core::hooks::HookSet::to_json)
        .collect::<Vec<_>>())
}

async fn set_hooks(State(kernel): Shared, body: Body) -> Answer {
    let body = body_of(body);
    let sets = kernel
        .set_user_hooks(body.get("hooks").unwrap_or(&Value::Null))
        .await?;
    ok(sets
        .iter()
        .map(workbench_core::hooks::HookSet::to_json)
        .collect::<Vec<_>>())
}

async fn approve_hooks(State(kernel): Shared, Path(id): Path<String>, body: Body) -> Answer {
    let body = body_of(body);
    let sets = kernel
        .approve_hooks(
            &id,
            &js::text(body.get("file")),
            &js::text(body.get("fingerprint")),
        )
        .await?;
    ok(sets
        .iter()
        .map(workbench_core::hooks::HookSet::to_json)
        .collect::<Vec<_>>())
}

async fn revoke_hooks(State(kernel): Shared, Path(id): Path<String>, body: Body) -> Answer {
    let sets = kernel
        .revoke_hooks(&id, &js::text(body_of(body).get("file")))
        .await?;
    ok(sets
        .iter()
        .map(workbench_core::hooks::HookSet::to_json)
        .collect::<Vec<_>>())
}

async fn approve_rules(State(kernel): Shared, Path(id): Path<String>, body: Body) -> Answer {
    let body = body_of(body);
    ok(layers_json(
        &kernel
            .approve_rules_file(
                &id,
                &js::text(body.get("file")),
                &js::text(body.get("fingerprint")),
            )
            .await?,
    ))
}

async fn revoke_rules(State(kernel): Shared, Path(id): Path<String>, body: Body) -> Answer {
    ok(layers_json(
        &kernel
            .revoke_rules_file(&id, &js::text(body_of(body).get("file")))
            .await?,
    ))
}

async fn active_project(State(kernel): Shared) -> Answer {
    ok(kernel.active_project().await?)
}

async fn set_active(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    ok(kernel.set_active_project(&id).await?)
}

async fn update_project(State(kernel): Shared, Path(id): Path<String>, body: Body) -> Answer {
    ok(kernel.update_project(&id, &body_of(body)).await?)
}

async fn delete_project(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    kernel.delete_project(&id).await?;
    no_content()
}

async fn project_access(State(kernel): Shared, Path(id): Path<String>, body: Body) -> Answer {
    let granted = body_of(body).get("granted") != Some(&Value::Bool(false));
    ok(kernel.set_project_access(&id, granted).await?)
}

async fn project_terminal(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    ok(kernel.open_project_terminal(&id).await?)
}

async fn project_folder(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    ok(kernel.open_project_folder(&id).await?)
}

async fn project_rules(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    ok(kernel.project_rules_view(&id).await?)
}

async fn save_rules(State(kernel): Shared, Path(id): Path<String>, body: Body) -> Answer {
    let body = body_of(body);
    ok(kernel
        .save_project_rules(
            &id,
            &js::text(body.get("content")),
            given(&body, "targetFile"),
        )
        .await?)
}

/// A project's instruction files and skills, and whether the user allowed them as they are.
async fn project_instructions(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    ok(kernel.project_instructions(&id).await?)
}

async fn approve_instructions(State(kernel): Shared, Path(id): Path<String>, body: Body) -> Answer {
    ok(kernel
        .approve_instructions(&id, &js::text(body_of(body).get("fingerprint")))
        .await?)
}

async fn revoke_instructions(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    ok(kernel.revoke_instructions(&id).await?)
}

async fn project_context(State(kernel): Shared, Path(id): Path<String>, body: Body) -> Answer {
    let body = body_of(body);
    let items = body
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    ok(kernel.expand_context(&id, &items).await?)
}

async fn project_files(
    State(kernel): Shared,
    Path(id): Path<String>,
    Query(query): QueryText,
) -> Answer {
    let limit = wanted(&query, "limit")
        .and_then(|limit| limit.parse().ok())
        .filter(|limit| *limit != 0)
        .unwrap_or(200);
    ok(kernel
        .project_files(&id, wanted(&query, "q").unwrap_or(""), limit)
        .await?)
}

/// The project folder a request's `projectId` names, which must be one the user has allowed.
fn folder_of(
    kernel: &Kernel,
    project: Option<&str>,
) -> Result<Option<std::path::PathBuf>, Failure> {
    Ok(project.map(|id| kernel.workdir(id)).transpose()?)
}

async fn skills(State(kernel): Shared, Query(query): QueryText) -> Answer {
    let root = folder_of(&kernel, wanted(&query, "projectId"))?;
    let listed = list_skills(
        root.as_deref(),
        &kernel.store.root().join("skills"),
        &kernel.enabled_plugin_folders(),
    )
    .await;
    ok(listed.iter().map(skill_json).collect::<Vec<_>>())
}

async fn skill(State(kernel): Shared, Path(name): Path<String>, Query(query): QueryText) -> Answer {
    let root = folder_of(&kernel, wanted(&query, "projectId"))?;
    let loaded = load_skill(
        root.as_deref(),
        &kernel.store.root().join("skills"),
        &kernel.enabled_plugin_folders(),
        &name,
    )
    .await;
    ok(loaded.ok_or_else(|| CoreError::new(404, "No such skill"))?)
}

async fn plugins(State(kernel): Shared) -> Answer {
    ok(kernel.list_plugins().await)
}

async fn install_plugin(State(kernel): Shared, body: Body) -> Answer {
    created(
        kernel
            .install_plugin(&js::text(body_of(body).get("source")))
            .await?,
    )
}

async fn change_plugin(State(kernel): Shared, Path(name): Path<String>, body: Body) -> Answer {
    let enabled = body_of(body).get("enabled") != Some(&Value::Bool(false));
    ok(kernel.set_plugin_enabled(&name, enabled).await?)
}

async fn approve_plugin_hooks(
    State(kernel): Shared,
    Path(name): Path<String>,
    body: Body,
) -> Answer {
    ok(kernel
        .approve_plugin_hooks(&name, &js::text(body_of(body).get("fingerprint")))
        .await?)
}

async fn uninstall_plugin(State(kernel): Shared, Path(name): Path<String>) -> Answer {
    kernel.uninstall_plugin(&name).await?;
    no_content()
}

async fn programs(State(kernel): Shared) -> Answer {
    ok(kernel.program_statuses().await)
}

async fn program(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    ok(kernel.program_status(program_id(&id)?).await)
}

async fn install_program(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    let status = kernel.install_program(program_id(&id)?).await?;
    Ok((StatusCode::ACCEPTED, axum::Json(status)).into_response())
}

async fn remove_program(State(kernel): Shared, Path(id): Path<String>) -> Answer {
    ok(kernel.remove_program(program_id(&id)?).await?)
}

async fn claude_status(State(kernel): Shared) -> Answer {
    ok(kernel.claude_status().await?)
}

async fn claude_logout(State(kernel): Shared) -> Answer {
    ok(kernel.claude_logout().await?)
}

async fn codex_status(State(kernel): Shared) -> Answer {
    ok(kernel.codex_status().await?)
}

/// Starts the official `codex login`: mode "device" (a one-time code, no local redirect) or "browser".
async fn codex_sign_in(State(kernel): Shared, body: Body) -> Answer {
    let browser = body_of(body).get("mode").and_then(Value::as_str) == Some("browser");
    ok(kernel.start_codex_sign_in(browser).await?)
}

/// Opens the page Codex printed for this sign-in in the default browser (never a URL from the request).
async fn codex_sign_in_open(State(kernel): Shared) -> Answer {
    ok(kernel.open_codex_sign_in()?)
}

async fn codex_sign_in_cancel(State(kernel): Shared) -> Answer {
    kernel.cancel_codex_sign_in();
    ok(kernel.codex_status().await?)
}

async fn codex_logout(State(kernel): Shared) -> Answer {
    ok(kernel.codex_logout().await?)
}

/// The app's skills as a stateless MCP server for Codex, which only reaches MCP servers out of process. The bearer token picks the
/// turn; it stops working when the turn ends.
async fn agent_mcp(
    State(kernel): Shared,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let message: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    match kernel.answer_agent_mcp(token, &message).await {
        None => (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({ "error": "Unauthorized" })),
        )
            .into_response(),
        Some(Some(answer)) => axum::Json(answer).into_response(),
        // A notification has nothing to answer.
        Some(None) => StatusCode::ACCEPTED.into_response(),
    }
}

/// Stateless: no session stream to open or close.
async fn agent_mcp_other() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(axum::http::header::ALLOW, "POST")],
        axum::Json(json!({ "error": "Method not allowed" })),
    )
        .into_response()
}

pub fn router(kernel: Arc<Kernel>) -> Router {
    Router::new()
        .route("/programs", get(programs))
        .route("/programs/{id}", get(program).delete(remove_program))
        .route("/programs/{id}/install", post(install_program))
        .route("/claude-code/status", get(claude_status))
        .route("/claude-code/logout", post(claude_logout))
        .route("/codex/status", get(codex_status))
        .route(
            "/codex/sign-in",
            post(codex_sign_in).delete(codex_sign_in_cancel),
        )
        .route("/codex/sign-in/open", post(codex_sign_in_open))
        .route("/codex/logout", post(codex_logout))
        .route("/agent-mcp", post(agent_mcp).fallback(agent_mcp_other))
        .route("/settings", get(settings).patch(update_settings))
        .route("/information", get(information))
        .route("/activity", get(activity))
        .route("/sandbox", get(sandbox))
        .route("/sandbox/setup", post(set_up_sandbox))
        .route("/logs", get(logs))
        .route(
            "/storage/snapshots",
            get(snapshots).delete(delete_snapshots),
        )
        .route("/storage/snapshots/{id}", delete(delete_snapshot))
        .route("/memory/{scope}", get(read_memory).put(write_memory))
        .route("/usage", get(usage))
        .route("/usage/summary", get(usage_summary))
        .route("/llm/providers", get(providers))
        .route(
            "/llm/providers/{provider}/models",
            get(provider_models).post(provider_models_for_key),
        )
        .route("/llm/lmstudio/models", get(local_server))
        .route("/connections/{kind}", get(list_connections))
        .route("/connections/{kind}/{id}", delete(delete_connection))
        .route(
            "/connections/models/{id}",
            patch(update_model).delete(delete_model),
        )
        .route("/connections/mcp", get(list_mcp).post(create_mcp))
        .route(
            "/connections/mcp/{id}",
            patch(update_mcp).delete(delete_mcp_connection),
        )
        .route("/connections/mcp/{id}/approve", post(approve_mcp))
        .route("/connections/mcp/{id}/discover", post(discover_mcp))
        .route("/mcp/{id}", delete(delete_mcp))
        .route("/connections/rag", get(list_rag).post(create_rag))
        .route(
            "/connections/rag/{id}",
            patch(update_rag).delete(delete_rag),
        )
        .route("/hooks", get(hook_sets).put(set_hooks))
        .route("/projects/active", get(active_project))
        .route(
            "/projects/{id}",
            patch(update_project).delete(delete_project),
        )
        .route("/projects/{id}/active", post(set_active))
        .route("/projects/{id}/access", post(project_access))
        .route("/projects/{id}/terminal", post(project_terminal))
        .route("/projects/{id}/folder", post(project_folder))
        .route("/projects/{id}/rules", get(project_rules).put(save_rules))
        .route("/oauth/callback", get(oauth_callback))
        .route("/projects/{id}/instructions", get(project_instructions))
        .route(
            "/projects/{id}/instructions/approve",
            post(approve_instructions).delete(revoke_instructions),
        )
        .route("/projects/{id}/context", post(project_context))
        .route("/projects/{id}/files", get(project_files))
        .route(
            "/projects/{id}/hooks/approve",
            post(approve_hooks).delete(revoke_hooks),
        )
        .route(
            "/projects/{id}/permissions/approve",
            post(approve_rules).delete(revoke_rules),
        )
        .route("/skills", get(skills))
        .route("/skills/{name}", get(skill))
        .route("/workspace/info", get(workspace_info))
        .route("/workspace/terminal", post(workspace_terminal))
        .route("/workspace/terminal/open", post(open_terminal))
        .route("/workspace/files", get(workspace_files))
        .route("/workspace/browser/open", post(open_browser))
        .route("/workspace/browser/proxy", get(browser_proxy))
        .route("/cli", get(cli_status))
        .route("/cli/install", post(install_cli))
        .route("/network", get(network_view))
        .route("/telemetry", delete(telemetry_clear))
        .route("/telemetry/status", get(telemetry_status))
        .route("/telemetry/traces", get(telemetry_traces))
        .route("/telemetry/traces/{id}", get(telemetry_trace))
        .route("/telemetry/metrics", get(telemetry_metrics))
        .route("/telemetry/logs", get(telemetry_logs))
        .route("/telemetry/export", get(telemetry_export))
        .route("/computer/permissions", get(computer_permissions))
        .route("/computer/permissions/open", post(open_permission_pane))
        .route("/plugins", get(plugins).post(install_plugin))
        .route(
            "/plugins/{name}",
            patch(change_plugin).delete(uninstall_plugin),
        )
        .route("/plugins/{name}/hooks/approve", post(approve_plugin_hooks))
        .with_state(kernel)
}
