//! Local agents and the model connections they use.
mod agents;
mod mcp;
mod models;
use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::mcp::{LocalCommand, McpError, Target};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use url::Url;
use workbench_domain::js;
use workbench_domain::llm::{llm_provider, runs_locally, runs_on_plan};
use workbench_domain::new_id;
use workbench_llm::{Effort, ModelConnection};

use crate::builtins::{
    COMPUTER_USE_ID, CUSTOMIZATIONS_ID, GENERAL_SKILLS_ID, ORIGIN_ID, RESEARCH_ID, builtin_server,
    with_general_skills,
};
use crate::records::{by_id, by_id_mut, remove, strings, text};
use crate::{CoreError, Kernel, locked, log};

/// The providers the settings page offers. Only a program that runs on the person's own plan comes with names for its
/// models; a provider reached with a key lists its own.
static PROVIDERS: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../llm-providers.json")).expect("this file is valid JSON")
});

/// A model a provider listed, as the settings form takes it: its id, a name, and the sizes the provider gave.
fn listed_choice(model: &workbench_llm::listing::ListedModel) -> Value {
    let mut choice =
        json!({ "id": model.id, "label": model.label.as_deref().unwrap_or(&model.id) });
    if let Some(tokens) = model.context_tokens {
        choice["contextTokens"] = tokens.into();
    }
    if let Some(tokens) = model.output_tokens {
        choice["outputTokens"] = tokens.into();
    }
    if !model.capabilities.unknown() {
        choice["capabilities"] = json!(model.capabilities);
    }
    choice
}

/// The providers, for the settings page.
pub fn llm_providers() -> &'static Value {
    &PROVIDERS
}

/// The name the app has for what a program on the person's own plan can be asked for (`opus`, `default`). A provider
/// reached with a key names its own models, so there is none for those.
fn known_label(provider: &str, model: &str) -> Option<&'static str> {
    let listed = PROVIDERS
        .as_array()?
        .iter()
        .find(|item| text(item, "id") == provider)?;
    let found = listed
        .get("models")?
        .as_array()?
        .iter()
        .find(|item| text(item, "id") == model)?;
    Some(text(found, "label"))
}

/// Where a local MCP server's environment variables are kept: the encrypted secrets file, never state.json.
pub fn env_secret_key(id: &str) -> String {
    format!("mcp:{id}:env")
}

/// Host only: MCP and knowledge source URLs can carry tokens in their path or query.
fn host_of(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|parsed| {
            let host = parsed.host_str()?.to_owned();
            Some(
                parsed
                    .port()
                    .map_or(host.clone(), |port| format!("{host}:{port}")),
            )
        })
        .unwrap_or_else(|| "(invalid url)".to_owned())
}

/// An MCP server is reached by URL, so adding one by address never makes the app start a program. Anything else (a command line,
/// a file: or stdio: address) is refused.
pub fn mcp_endpoint(url: &str) -> Result<String, CoreError> {
    let value = js::trim(url);
    let parsed = Url::parse(value).map_err(|_| {
        CoreError::new(400, "An MCP server is added by its URL (https://…). The app does not start local programs as MCP servers.")
    })?;
    if !matches!(parsed.scheme(), "https" | "http") {
        return Err(CoreError::new(
            400,
            format!(
                "MCP servers are reached over HTTP(S), not {}. The app does not start local programs as MCP servers.",
                parsed.scheme()
            ),
        ));
    }
    Ok(value.to_owned())
}

/// Remote tool names, prefixed per server so two servers' `search` tools never collide; safe for every LLM API.
pub fn namespaced_tool(server: &str, name: &str) -> String {
    let joined: String = format!("{server}__{name}")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    js::prefix(&joined, 64).to_owned()
}

/// A tool as saved, from what a request says about it.
fn route(server: &str, tool: &Value) -> Value {
    json!({
        "toolId": text(tool, "toolId"),
        "serverId": server,
        "description": text(tool, "description"),
        "inputSchema": { "type": "object", "properties": {} },
        "environments": ["production"],
    })
}

fn routes(server: &str, input: &Value) -> Vec<Value> {
    input
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|tool| route(server, tool))
        .collect()
}

/// What a program, its arguments and its variables are, checked: a local MCP server's command.
pub fn parse_command(input: &Value) -> Result<(Value, BTreeMap<String, String>), CoreError> {
    let program = js::trim(text(input, "program"));
    if program.is_empty() {
        return Err(CoreError::new(
            400,
            "A local MCP server needs the program to run, for example npx or uvx.",
        ));
    }
    if program.contains(['\r', '\n', '\0']) {
        return Err(CoreError::new(
            400,
            "The program name contains a line break.",
        ));
    }
    let args: Vec<&Value> = input
        .get("args")
        .and_then(Value::as_array)
        .map_or(Vec::new(), |items| items.iter().collect());
    let mut texts = Vec::new();
    for arg in args {
        let arg = arg
            .as_str()
            .ok_or_else(|| CoreError::new(400, "Arguments must be text."))?;
        if arg.contains('\0') {
            return Err(CoreError::new(400, "An argument contains a NUL character."));
        }
        texts.push(arg);
    }
    let mut env = BTreeMap::new();
    for (name, value) in input
        .get("env")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let valid = name.chars().enumerate().all(|(index, character)| {
            character == '_'
                || character.is_ascii_alphabetic()
                || (index > 0 && character.is_ascii_digit())
        });
        if name.is_empty() || !valid {
            return Err(CoreError::new(
                400,
                format!("\"{name}\" is not a valid environment variable name."),
            ));
        }
        let value = value
            .as_str()
            .ok_or_else(|| CoreError::new(400, format!("The value of {name} must be text.")))?;
        env.insert(name.clone(), value.to_owned());
    }
    let names: Vec<&String> = env.keys().collect();
    Ok((
        json!({ "program": program, "args": texts, "env": names }),
        env,
    ))
}

/// The fingerprint the user approves: program, arguments, and every environment variable with its value.
pub fn command_fingerprint(command: &Value, env: &BTreeMap<String, String>) -> String {
    let entries: Vec<Value> = env
        .iter()
        .map(|(name, value)| json!([name, value]))
        .collect();
    let digest = Sha256::digest(
        json!([command.get("program"), command.get("args"), entries])
            .to_string()
            .as_bytes(),
    );
    workbench_domain::research::hex(&digest)
}

/// How the command reads in a terminal, for the approval card. Arguments with spaces or quotes are quoted.
pub fn command_line(command: &Value) -> String {
    let quote = |part: &str| {
        let plain = !part.is_empty()
            && part.chars().all(|character| {
                character.is_ascii_alphanumeric() || "_@%+=:,./-".contains(character)
            });
        if plain {
            part.to_owned()
        } else {
            format!("'{}'", part.replace('\'', "'\\''"))
        }
    };
    let mut parts = vec![quote(text(command, "program"))];
    parts.extend(strings(command, "args").iter().map(|arg| quote(arg)));
    parts.join(" ")
}

/// A readable placeholder for the url every MCP connection has.
fn command_address(command: &Value) -> String {
    let program = text(command, "program");
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    format!("stdio:{name}")
}

/// A saved model as the broker reads it. An effort it does not know counts as none.
pub fn connection_of(model: &Value) -> ModelConnection {
    ModelConnection {
        id: text(model, "id").to_owned(),
        name: text(model, "name").to_owned(),
        provider: text(model, "provider").to_owned(),
        model: text(model, "model").to_owned(),
        base_url: text(model, "baseUrl").to_owned(),
        effort: model
            .get("effort")
            .and_then(|effort| serde_json::from_value::<Effort>(effort.clone()).ok()),
        capabilities: model
            .get("capabilities")
            .and_then(|said| serde_json::from_value(said.clone()).ok())
            .unwrap_or_default(),
    }
}

/// The value of a field if the input has one, else the fallback.
fn or(input: &Value, key: &str, fallback: Value) -> Value {
    input
        .get(key)
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or(fallback)
}
