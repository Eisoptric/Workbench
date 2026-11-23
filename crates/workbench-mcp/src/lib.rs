//! An MCP client built entirely on the official `rmcp` SDK: Streamable HTTP and local programs over stdio, with OAuth 2.1 PKCE.
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::model::CallToolRequestParams;
use rmcp::service::{RoleClient, RunningService};
use serde_json::{Value, json};
use tokio::sync::OnceCell;
use tokio::task::JoinHandle;
pub(crate) use workbench_domain::sync::locked;
use workbench_store::FileStore;
use workbench_telemetry::network::client_builder;
use workbench_telemetry::{Kind, Span};

mod local;
mod oauth;
mod remote;

use oauth::Pending;

/// How long a server gets to finish the handshake.
const HANDSHAKE: Duration = Duration::from_secs(15);
/// How long a request may take, as the official SDKs allow by default.
const REQUEST: Duration = Duration::from_secs(60);

/// A local server to start and talk to over stdio, already allowed by the user.
#[derive(Clone, Debug, Default)]
pub struct LocalCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
}

/// What the client needs to reach one MCP server.
#[derive(Clone, Debug, Default)]
pub struct Target {
    pub id: String,
    pub url: String,
    /// "oauth": sign in through the browser; "token": a bearer token the user pasted; anything else: an open server.
    pub auth: String,
    pub command: Option<LocalCommand>,
}

/// Why a call did not work.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum McpError {
    /// The server needs the user to sign in; the browser has been sent to this address.
    #[error("Sign-in required")]
    SignIn(String),
    #[error("{0}")]
    Failed(String),
}

impl McpError {
    fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

/// MCP answers with these to steer the run, not because something broke, so the session stays open.
const CONTROL: [&str; 4] = [
    "approval_required",
    "policy_denied",
    "approval_rejected",
    "Agent is not allowed to call this tool",
];

/// An open `rmcp` connection to a local or remote server.
struct Session {
    service: RunningService<RoleClient, ()>,
    stderr: Option<JoinHandle<()>>,
    last_error: Option<Arc<Mutex<Option<McpError>>>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(handle) = &self.stderr {
            handle.abort();
        }
    }
}

impl Session {
    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let map_error = |error: rmcp::ServiceError| {
            if let Some(last) = &self.last_error
                && let Some(saved) = locked(last).take()
            {
                return saved;
            }
            match error {
                rmcp::ServiceError::McpError(data) if !data.message.is_empty() => {
                    McpError::failed(data.message.into_owned())
                }
                other => McpError::failed(other.to_string()),
            }
        };
        match method {
            "tools/list" => {
                let result = self.service.list_tools(None).await.map_err(map_error)?;
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
                let result = self.service.call_tool(call).await.map_err(map_error)?;
                serde_json::to_value(result).map_err(|error| McpError::failed(error.to_string()))
            }
            other => Err(McpError::failed(format!("Unsupported MCP method: {other}"))),
        }
    }
}

/// Host only: the rest of a URL can carry tokens.
pub(crate) fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            let host = parsed.host_str()?.to_owned();
            Some(
                parsed
                    .port()
                    .map_or(host.clone(), |port| format!("{host}:{port}")),
            )
        })
        .unwrap_or_default()
}

/// Opens the sign-in page in the user's browser.
pub type OpenBrowser = Arc<dyn Fn(&str) + Send + Sync>;

/// An MCP client for all of a runtime's servers.
pub struct McpClient {
    pub(crate) store: Arc<FileStore>,
    pub(crate) redirect_url: String,
    pub(crate) open_browser: OpenBrowser,
    pub(crate) http: reqwest::Client,
    pub(crate) sessions: Mutex<HashMap<String, Arc<OnceCell<Arc<Session>>>>>,
    /// OAuth sign-ins waiting for the browser to come back, by `state`.
    pub(crate) pending: Arc<Mutex<HashMap<String, Pending>>>,
}

impl McpClient {
    /// `redirect_url` is where providers send the browser back after sign-in.
    pub fn new(
        store: Arc<FileStore>,
        redirect_url: String,
        open_browser: OpenBrowser,
    ) -> Result<Self, McpError> {
        let http = client_builder()
            .no_proxy()
            .build()
            .map_err(|error| McpError::failed(error.without_url().to_string()))?;
        Ok(Self {
            store,
            redirect_url,
            open_browser,
            http,
            sessions: Mutex::default(),
            pending: Arc::default(),
        })
    }

    pub(crate) fn handle(&self) -> remote::McpClientHandle {
        remote::McpClientHandle {
            store: Arc::clone(&self.store),
            redirect_url: self.redirect_url.clone(),
            open_browser: Arc::clone(&self.open_browser),
            http: self.http.clone(),
            pending: Arc::clone(&self.pending),
        }
    }

    /// The tools a server has, as saved routes.
    pub async fn list_tools(&self, target: &Target) -> Result<Vec<Value>, McpError> {
        let payload = self.rpc(target, "tools/list", json!({})).await?;
        Ok(payload
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tool| {
                let name = tool.get("name")?.as_str()?;
                let schema = tool.get("inputSchema").filter(|schema| schema.is_object());
                Some(json!({
                    "toolId": name,
                    "serverId": "",
                    "description": tool.get("description").and_then(Value::as_str).unwrap_or(""),
                    "inputSchema": schema.cloned().unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
                    "environments": ["production"],
                }))
            })
            .collect())
    }

    /// Calls one tool.
    pub async fn call(
        &self,
        target: &Target,
        tool: &str,
        arguments: &Value,
        _headers: &[(&str, &str)],
    ) -> Result<Value, McpError> {
        self.rpc(
            target,
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        )
        .await
    }

    /// Connects, signing in if needed.
    pub async fn connect(&self, target: &Target) -> Result<(), McpError> {
        self.session(target).await.map(|_| ())
    }

    /// Ends a server's session (stopping a local server's process) without touching its credentials.
    pub fn close(&self, id: &str) {
        locked(&self.sessions).remove(id);
    }

    /// Drops the session and every stored credential for a server.
    pub async fn forget(&self, id: &str) -> Result<(), McpError> {
        self.close(id);
        locked(&self.pending).retain(|_, waiting| waiting.target.id != id);
        for part in ["client", "tokens", "verifier", "token"] {
            self.store
                .delete_secret(&secret_key(id, part))
                .await
                .map_err(|error| McpError::failed(error.to_string()))?;
        }
        Ok(())
    }

    /// Saves a bearer token the user pasted.
    pub async fn save_token(&self, id: &str, token: &str) -> Result<(), McpError> {
        self.store
            .put_secret(&secret_key(id, "token"), token.trim())
            .await
            .map_err(|error| McpError::failed(error.to_string()))?;
        self.close(id);
        Ok(())
    }

    /// Registers a client the user created with the provider, for servers that do not allow self-registration.
    pub async fn save_oauth_client(
        &self,
        id: &str,
        client_id: &str,
        client_secret: Option<&str>,
    ) -> Result<(), McpError> {
        let mut info = json!({ "client_id": client_id.trim() });
        if let Some(secret) = client_secret
            .map(str::trim)
            .filter(|secret| !secret.is_empty())
        {
            info["client_secret"] = secret.into();
        }
        self.store
            .put_secret(&secret_key(id, "client"), &info.to_string())
            .await
            .map_err(|error| McpError::failed(error.to_string()))?;
        self.close(id);
        Ok(())
    }

    async fn session(&self, target: &Target) -> Result<Arc<Session>, McpError> {
        let cell = Arc::clone(locked(&self.sessions).entry(target.id.clone()).or_default());
        cell.get_or_try_init(|| async { self.open(target).await.map(Arc::new) })
            .await
            .cloned()
    }

    async fn open(&self, target: &Target) -> Result<Session, McpError> {
        if let Some(command) = &target.command {
            let (service, stderr) = local::connect_local(command).await?;
            return Ok(Session {
                service,
                stderr: Some(stderr),
                last_error: None,
            });
        }
        let (service, last_error) = remote::connect_remote(self, target).await?;
        Ok(Session {
            service,
            stderr: None,
            last_error: Some(last_error),
        })
    }

    async fn rpc(&self, target: &Target, method: &str, params: Value) -> Result<Value, McpError> {
        let tool = (method == "tools/call").then(|| {
            params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        });
        let name = tool.as_ref().map_or_else(
            || format!("mcp {method}"),
            |tool| format!("mcp {method} {tool}"),
        );
        let base = json!({ "mcp.method.name": method, "server.address": host_of(&target.url) });
        let mut attributes = base.clone();
        attributes["rpc.system"] = json!("jsonrpc");
        if let Some(tool) = &tool {
            attributes["gen_ai.tool.name"] = json!(tool);
        }
        let span = Span::start(name, Kind::Client, attributes);
        let started = Instant::now();
        let answer = span
            .scope(async {
                let session = self.session(target).await?;
                tokio::time::timeout(REQUEST, session.request(method, params))
                    .await
                    .unwrap_or_else(|_| {
                        Err(McpError::failed(format!(
                            "The MCP server did not answer {method} in time"
                        )))
                    })
            })
            .await;
        let outcome = match &answer {
            Ok(_) => "ok".to_owned(),
            Err(McpError::Failed(message)) if CONTROL.contains(&message.as_str()) => {
                message.clone()
            }
            Err(McpError::Failed(message)) => {
                span.fail(message, "error");
                self.close(&target.id);
                "error".to_owned()
            }
            Err(_) => "sign_in_required".to_owned(),
        };
        if outcome != "ok" {
            span.set("workbench.mcp.outcome", outcome.as_str());
        }
        let mut labels = base;
        labels["workbench.mcp.outcome"] = json!(outcome);
        workbench_telemetry::record(
            "workbench.mcp.request.duration",
            started.elapsed().as_secs_f64(),
            labels,
        );
        answer
    }
}

/// Where a server's credential is kept: the encrypted secrets file.
pub(crate) fn secret_key(id: &str, part: &str) -> String {
    format!("mcp:{id}:{part}")
}
