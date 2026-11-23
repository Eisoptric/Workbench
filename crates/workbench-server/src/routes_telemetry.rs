//! Telemetry, network log, CLI launcher, and OS permission pane handlers.
use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use workbench_core::CoreError;
use workbench_core::projects::launch_app;
use workbench_domain::js;
use workbench_telemetry::formats;
use workbench_telemetry::network;

use crate::routes::{Answer, Body, Shared, body_of, given, ok};

type QueryText = Query<HashMap<String, String>>;

pub(crate) async fn computer_permissions(State(kernel): Shared) -> Answer {
    ok(kernel.computer.permission_status().await)
}

/// The `workbench` command: whether its launcher is installed (and current).
pub(crate) async fn cli_status() -> Answer {
    ok(workbench_core::cli_install::cli_status().await?)
}

/// Installs it in ~/.local/bin.
pub(crate) async fn install_cli() -> Answer {
    ok(workbench_core::cli_install::install_cli().await?)
}

/// Every HTTP(S) request the runtime has sent since it started: by host, and the latest ones (no queries, no bodies).
pub(crate) async fn network_view(Query(query): QueryText) -> Answer {
    let asked = query
        .get("limit")
        .and_then(|limit| limit.parse::<usize>().ok())
        .filter(|limit| *limit > 0);
    ok(network::view(asked.unwrap_or(200).min(500)))
}

pub(crate) async fn telemetry_status() -> Answer {
    ok(workbench_telemetry::status())
}

pub(crate) async fn telemetry_traces() -> Answer {
    ok(workbench_telemetry::summaries())
}

pub(crate) async fn telemetry_trace(Path(id): Path<String>) -> Answer {
    ok(workbench_telemetry::trace(&id)
        .ok_or_else(|| CoreError::new(404, "Trace is no longer in memory"))?)
}

pub(crate) async fn telemetry_metrics() -> Answer {
    ok(workbench_telemetry::metric_view())
}

pub(crate) async fn telemetry_logs(Query(query): QueryText) -> Answer {
    let least = query
        .get("minSeverity")
        .and_then(|least| least.parse::<f64>().ok())
        .filter(|least| *least != 0.0);
    ok(workbench_telemetry::logs(
        query.get("traceId").map(String::as_str),
        least,
    ))
}

/// Saves telemetry.json now and sends it as ?format=workbench (the file itself), otlp (OTLP/JSON lines) or siem (ECS NDJSON).
pub(crate) async fn telemetry_export(Query(query): QueryText) -> Answer {
    let format = match query.get("format").map(String::as_str) {
        Some("otlp") => "otlp",
        Some("siem") => "siem",
        _ => "workbench",
    };
    let document = workbench_telemetry::save()
        .await
        .map_err(|error| CoreError::new(500, error.to_string()))?;
    let (extension, kind) = formats::details(format);
    let saved = document["savedAt"].as_str().unwrap_or_default();
    let stamp = js::prefix(saved, 19).replace([':', 'T'], "-");
    let name = if format == "workbench" {
        "workbench-telemetry".to_owned()
    } else {
        format!("workbench-telemetry-{format}")
    };
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                format!("{kind}; charset=utf-8"),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}-{stamp}.{extension}\""),
            ),
        ],
        formats::render(&document, format),
    )
        .into_response())
}

pub(crate) async fn telemetry_clear() -> Answer {
    workbench_telemetry::clear()
        .await
        .map_err(|error| CoreError::new(500, error.to_string()))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(crate) async fn open_permission_pane(body: Body) -> Answer {
    let body = body_of(body);
    let pane = match given(&body, "which") {
        Some("accessibility") => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        Some("screenRecording") => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        _ => {
            return Err(
                CoreError::new(400, "which must be accessibility or screenRecording").into(),
            );
        }
    };
    if !cfg!(target_os = "macos") {
        return Err(CoreError::new(409, "Only macOS asks for these permissions").into());
    }
    launch_app("System Settings", "open", &[pane], None);
    ok(json!({ "ok": true }))
}
