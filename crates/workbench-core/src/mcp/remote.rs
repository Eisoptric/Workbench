//! Remote MCP servers over `rmcp`'s Streamable HTTP transport, with `workbench-telemetry` network logging and OAuth token refresh.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::stream::BoxStream;
use http::{HeaderName, HeaderValue};
use rmcp::ServiceExt;
use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, InitializeResult, JsonRpcMessage, ServerResult,
};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::streamable_http_client::{
    SseError, StreamableHttpClient, StreamableHttpClientTransport,
    StreamableHttpClientTransportConfig, StreamableHttpError, StreamableHttpPostResponse,
    StreamableHttpResponseLimits,
};
use sse_stream::Sse;
use url::Url;

use super::{HANDSHAKE, McpClient, McpError, Target, host_of, locked, secret_key};

/// Who may read a transport error: the message without the address, which can carry tokens.
fn transport_error(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    if let Some(url) = error.url() {
        message = message.replace(&format!(" for url ({url})"), "");
    }
    let mut cause = std::error::Error::source(error);
    while let Some(reason) = cause {
        message = format!("{message}: {reason}");
        cause = std::error::Error::source(reason);
    }
    message
}

fn bearer_token(client: &McpClient, target: &Target) -> Option<String> {
    match target.auth.as_str() {
        "token" => client.store.secret(&secret_key(&target.id, "token")),
        "oauth" => client.saved_access_token(&target.id),
        _ => None,
    }
}

async fn unauthorized(client: &McpClient, target: &Target, challenge: Option<&str>) -> McpError {
    if target.auth == "oauth" {
        return client.begin_sign_in(target, challenge).await;
    }
    McpError::failed(if target.auth == "token" {
        "The server refused the token"
    } else {
        "The server requires sign-in"
    })
}

fn normalize_post_response(
    request: &ClientJsonRpcMessage,
    response: Result<StreamableHttpPostResponse, StreamableHttpError<reqwest::Error>>,
) -> Result<StreamableHttpPostResponse, StreamableHttpError<reqwest::Error>> {
    let is_init = matches!(
        request,
        JsonRpcMessage::Request(req) if matches!(req.request, ClientRequest::InitializeRequest(_))
    );
    let is_notification = matches!(request, JsonRpcMessage::Notification(_));
    match response {
        Ok(StreamableHttpPostResponse::Json(JsonRpcMessage::Response(mut resp), session_id))
            if is_init && matches!(resp.result, ServerResult::CustomResult(_)) =>
        {
            resp.result = ServerResult::InitializeResult(InitializeResult::default());
            Ok(StreamableHttpPostResponse::Json(
                JsonRpcMessage::Response(resp),
                session_id,
            ))
        }
        Err(
            StreamableHttpError::UnexpectedServerResponse(_) | StreamableHttpError::Deserialize(_),
        ) if is_notification => Ok(StreamableHttpPostResponse::Accepted),
        other => other,
    }
}
fn note_url(method: &str, uri: &str) {
    if let Ok(parsed) = Url::parse(uri) {
        workbench_telemetry::network::note(method, &parsed);
    }
}

/// A `StreamableHttpClient` backed by `reqwest::Client` that records every request in `workbench-telemetry` and refreshes OAuth tokens on 401.
#[derive(Clone)]
struct NotedHttpClient {
    client: McpClientHandle,
    target: Target,
    last_error: Arc<Mutex<Option<McpError>>>,
}

#[derive(Clone)]
pub(crate) struct McpClientHandle {
    pub store: Arc<workbench_store::FileStore>,
    pub redirect_url: String,
    pub open_browser: super::OpenBrowser,
    pub http: reqwest::Client,
    pub pending: Arc<Mutex<HashMap<String, super::oauth::Pending>>>,
}

impl NotedHttpClient {
    fn as_mcp(&self) -> McpClient {
        McpClient {
            store: Arc::clone(&self.client.store),
            redirect_url: self.client.redirect_url.clone(),
            open_browser: Arc::clone(&self.client.open_browser),
            http: self.client.http.clone(),
            sessions: Mutex::default(),
            pending: Arc::clone(&self.client.pending),
        }
    }

    async fn record_failure(
        &self,
        error: &StreamableHttpError<reqwest::Error>,
    ) -> Option<McpError> {
        let mcp = self.as_mcp();
        match error {
            StreamableHttpError::AuthRequired(auth) => {
                Some(unauthorized(&mcp, &self.target, Some(&auth.www_authenticate_header)).await)
            }
            StreamableHttpError::UnexpectedServerResponse(text)
                if text.starts_with("HTTP 401") || text.contains("401 Unauthorized") =>
            {
                Some(unauthorized(&mcp, &self.target, None).await)
            }
            StreamableHttpError::Client(err) => Some(McpError::failed(format!(
                "Could not reach the MCP server at {}: {}",
                host_of(&self.target.url),
                transport_error(err)
            ))),
            StreamableHttpError::HttpStatus(status) => Some(McpError::failed(format!(
                "The MCP server answered {}",
                status.status
            ))),
            _ => None,
        }
    }
}

impl StreamableHttpClient for NotedHttpClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.post_message_with_response_limits(
            uri,
            message,
            session_id,
            auth_header,
            custom_headers,
            StreamableHttpResponseLimits::default(),
        )
        .await
    }

    async fn post_message_with_response_limits(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        limits: StreamableHttpResponseLimits,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let mcp = self.as_mcp();
        note_url("POST", &uri);
        let token = bearer_token(&mcp, &self.target);
        let first = normalize_post_response(
            &message,
            <reqwest::Client as StreamableHttpClient>::post_message_with_response_limits(
                &self.client.http,
                Arc::clone(&uri),
                message.clone(),
                session_id.clone(),
                token,
                custom_headers.clone(),
                limits,
            )
            .await,
        );
        match first {
            Ok(response) => Ok(response),
            Err(error) => {
                let is_401 = matches!(&error, StreamableHttpError::AuthRequired(_))
                    || matches!(&error, StreamableHttpError::UnexpectedServerResponse(t) if t.starts_with("HTTP 401"));
                if is_401 && self.target.auth == "oauth" && mcp.refresh_tokens(&self.target).await {
                    note_url("POST", &uri);
                    let refreshed = bearer_token(&mcp, &self.target);
                    let retry = normalize_post_response(
                        &message,
                        <reqwest::Client as StreamableHttpClient>::post_message_with_response_limits(
                            &self.client.http,
                            uri,
                            message.clone(),
                            session_id,
                            refreshed,
                            custom_headers,
                            limits,
                        )
                        .await,
                    );
                    if let Err(err) = &retry
                        && let Some(mapped) = self.record_failure(err).await
                    {
                        *locked(&self.last_error) = Some(mapped);
                    }
                    return retry;
                }
                if let Some(mapped) = self.record_failure(&error).await {
                    *locked(&self.last_error) = Some(mapped);
                }
                Err(error)
            }
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let mcp = self.as_mcp();
        note_url("DELETE", &uri);
        let token = bearer_token(&mcp, &self.target);
        <reqwest::Client as StreamableHttpClient>::delete_session(
            &self.client.http,
            uri,
            session_id,
            token,
            custom_headers,
        )
        .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let mcp = self.as_mcp();
        note_url("GET", &uri);
        let token = bearer_token(&mcp, &self.target);
        <reqwest::Client as StreamableHttpClient>::get_stream(
            &self.client.http,
            uri,
            session_id,
            last_event_id,
            token,
            custom_headers,
        )
        .await
    }
}

/// Opens a remote Streamable HTTP MCP session through `rmcp`.
pub(crate) async fn connect_remote(
    client: &McpClient,
    target: &Target,
) -> Result<(RunningService<RoleClient, ()>, Arc<Mutex<Option<McpError>>>), McpError> {
    let last_error = Arc::new(Mutex::new(None));
    let noted = NotedHttpClient {
        client: client.handle(),
        target: target.clone(),
        last_error: Arc::clone(&last_error),
    };
    let mut config = StreamableHttpClientTransportConfig::with_uri(target.url.clone());
    config.allow_stateless = true;
    let transport = StreamableHttpClientTransport::with_client(noted, config);
    match tokio::time::timeout(HANDSHAKE, ().serve(transport)).await {
        Ok(Ok(service)) => Ok((service, last_error)),
        Ok(Err(error)) => {
            if let Some(saved) = locked(&last_error).take() {
                return Err(saved);
            }
            Err(McpError::failed(format!(
                "Could not connect to the MCP server at {}: {error}",
                host_of(&target.url)
            )))
        }
        Err(_) => Err(McpError::failed(format!(
            "The MCP server at {} did not finish the handshake in time",
            host_of(&target.url)
        ))),
    }
}
