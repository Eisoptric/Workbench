//! A local MCP server run over stdio through the official `rmcp` client SDK.
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::{HANDSHAKE, LocalCommand, McpError, locked};

/// Bytes of a server's stderr that are kept.
const STDERR_LIMIT: usize = 4000;
/// Of the runtime's own variables, the only ones a local server gets, so API keys in its environment never reach it.
#[cfg(unix)]
const INHERITED: [&str; 6] = ["HOME", "LOGNAME", "PATH", "SHELL", "TERM", "USER"];
#[cfg(windows)]
const INHERITED: [&str; 13] = [
    "APPDATA",
    "HOMEDRIVE",
    "HOMEPATH",
    "LOCALAPPDATA",
    "PATH",
    "PROCESSOR_ARCHITECTURE",
    "SYSTEMDRIVE",
    "SYSTEMROOT",
    "TEMP",
    "USERNAME",
    "USERPROFILE",
    "PROGRAMFILES",
    "COMSPEC",
];

pub(crate) struct LocalSession {
    service: RunningService<RoleClient, ()>,
    errors: JoinHandle<()>,
}

impl Drop for LocalSession {
    fn drop(&mut self) {
        self.errors.abort();
    }
}

impl LocalSession {
    pub(crate) async fn start(command: &LocalCommand) -> Result<Self, McpError> {
        if let Some(cwd) = &command.cwd {
            tokio::fs::create_dir_all(cwd)
                .await
                .map_err(|error| McpError::failed(error.to_string()))?;
        }
        let mut prepared = Command::new(&command.program);
        prepared.args(&command.args).env_clear();
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            prepared.creation_flags(CREATE_NO_WINDOW);
        }
        for name in INHERITED {
            if let Some(value) = std::env::var_os(name) {
                prepared.env(name, value);
            }
        }
        prepared.envs(&command.env);
        if let Some(cwd) = &command.cwd {
            prepared.current_dir(cwd);
        }
        let failed = |reason: &str, stderr: &str| {
            let detail: String = stderr
                .trim()
                .lines()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            let more = if detail.is_empty() {
                String::new()
            } else {
                format!("\n{detail}")
            };
            McpError::failed(format!(
                "Could not start {}: {reason}{more}",
                command.program
            ))
        };
        let (transport, stderr) = TokioChildProcess::builder(prepared)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| failed(&error.to_string(), ""))?;
        let kept = Arc::new(Mutex::new(Vec::new()));
        let written = Arc::clone(&kept);
        let errors = tokio::spawn(async move {
            let Some(mut stderr) = stderr else { return };
            let mut chunk = [0_u8; 1024];
            while let Ok(read) = stderr.read(&mut chunk).await {
                if read == 0 {
                    break;
                }
                let mut bytes = locked(&written);
                bytes.extend_from_slice(&chunk[..read]);
                let extra = bytes.len().saturating_sub(STDERR_LIMIT);
                bytes.drain(..extra);
            }
        });
        let outcome = tokio::time::timeout(HANDSHAKE * 4, ().serve(transport)).await;
        match outcome {
            Ok(Ok(service)) => Ok(Self { service, errors }),
            Ok(Err(error)) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let stderr = String::from_utf8_lossy(&locked(&kept)).into_owned();
                Err(failed(&error.to_string(), &stderr))
            }
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let stderr = String::from_utf8_lossy(&locked(&kept)).into_owned();
                Err(failed("it did not finish the handshake in time", &stderr))
            }
        }
    }

    pub(crate) async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        match method {
            "tools/list" => {
                let result = self
                    .service
                    .list_tools(None)
                    .await
                    .map_err(|error| McpError::failed(error.to_string()))?;
                serde_json::to_value(result).map_err(|error| McpError::failed(error.to_string()))
            }
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let arguments = params.get("arguments").and_then(Value::as_object).cloned();
                let mut call = CallToolRequestParams::new(name);
                if let Some(arguments) = arguments {
                    call = call.with_arguments(arguments);
                }
                let result = self
                    .service
                    .call_tool(call)
                    .await
                    .map_err(|error| McpError::failed(error.to_string()))?;
                serde_json::to_value(result).map_err(|error| McpError::failed(error.to_string()))
            }
            other => Err(McpError::failed(format!(
                "Unsupported local MCP method: {other}"
            ))),
        }
    }
}
