//! Model connection CRUD and provider model discovery for `Kernel`.
use super::*;

impl Kernel {
    /// Whether a model has a key to send.
    fn with_key(&self, model: &Value) -> Value {
        let mut keyed = model.clone();
        keyed["hasSecret"] = self.broker.has_key(&connection_of(model)).into();
        keyed
    }

    /// The saved models, each saying whether it has a key. Never the key.
    pub fn list_models(&self) -> Vec<Value> {
        self.store
            .read(|state| state.models.clone())
            .iter()
            .map(|model| self.with_key(model))
            .collect()
    }

    /// Connects a model, saving its key when one came with it.
    pub async fn create_model(&self, input: &Value) -> Result<Value, CoreError> {
        let wanted = text(input, "provider");
        let provider = llm_provider(if wanted.is_empty() { "openai" } else { wanted }).ok_or_else(
            || {
                CoreError::new(
                    400,
                    "Choose OpenAI, Claude, Claude (via Claude Code), Codex, Gemini, DeepSeek, or a local server",
                )
            },
        )?;
        let secret = text(input, "secret");
        if runs_on_plan(provider.id) && !secret.is_empty() {
            return Err(CoreError::new(
                400,
                format!(
                    "{} runs on your own plan through its official program and takes no API key. For API access, add the model under Claude or OpenAI.",
                    provider.label
                ),
            ));
        }
        let named = |key: &str| workbench_domain::js::trim(text(input, key)).to_owned();
        let model_id = if text(input, "model").is_empty() {
            named("name")
        } else {
            named("model")
        };
        if model_id.is_empty() {
            return Err(CoreError::new(400, "Choose a model"));
        }
        let base_url = if text(input, "baseUrl").is_empty() {
            provider.base_url
        } else {
            text(input, "baseUrl")
        };
        let mut fields = Map::new();
        fields.insert("id".to_owned(), new_id("mdl")?.into());
        let given = named("name");
        // The name that came with it (the provider's own, from its list), else the name the app has for what a program
        // on the person's plan is asked for, else the id. A subscription model is named apart from the API model of the
        // same name, so the two are never confused.
        let known = known_label(provider.id, &model_id)
            .filter(|label| !label.is_empty())
            .unwrap_or(&model_id);
        let name = match provider.id {
            _ if !given.is_empty() => given,
            "claude-code" => format!("{known} (via Claude Code)"),
            _ => known.to_owned(),
        };
        fields.insert("name".to_owned(), name.into());
        fields.insert("provider".to_owned(), provider.id.into());
        fields.insert("model".to_owned(), model_id.as_str().into());
        fields.insert(
            "baseUrl".to_owned(),
            base_url.strip_suffix('/').unwrap_or(base_url).into(),
        );
        fields.insert("hasSecret".to_owned(), (!secret.is_empty()).into());
        if let Some(effort) = input
            .get("effort")
            .filter(|effort| serde_json::from_value::<Effort>((*effort).clone()).is_ok())
        {
            fields.insert("effort".to_owned(), effort.clone());
        }
        // The model's context window as its provider listed it, which the settings form passes on. Without one the
        // runtime assumes a size by provider, which is smaller than most current models have.
        if let Some(tokens) = input
            .get("contextTokens")
            .and_then(Value::as_u64)
            .filter(|tokens| *tokens > 0)
        {
            fields.insert("contextTokens".to_owned(), tokens.into());
        }
        // What the provider's list said the model accepts, passed on the same way. A connection without it is described
        // the first time it is used.
        if let Some(said) = input
            .get("capabilities")
            .and_then(|said| {
                serde_json::from_value::<workbench_llm::Capabilities>(said.clone()).ok()
            })
            .filter(|said| !said.unknown())
        {
            fields.insert("capabilities".to_owned(), json!(said));
        }
        let model = Value::Object(fields);
        let saved = model.clone();
        self.store.update(|state| state.models.push(saved)).await?;
        if !secret.is_empty() {
            self.broker.save_key(&connection_of(&model), secret).await?;
        }
        let keyed = self.with_key(&model);
        log(
            "catalog",
            "info",
            &format!(
                "Model {} connected ({}, {})",
                text(&model, "name"),
                provider.label,
                if !secret.is_empty() {
                    "key entered".to_owned()
                } else if keyed["hasSecret"] == true {
                    format!("using the saved {} key", provider.label)
                } else if provider.needs_key {
                    "no key".to_owned()
                } else {
                    "no key needed".to_owned()
                }
            ),
        );
        if provider.needs_key && keyed["hasSecret"] != true {
            log(
                "catalog",
                "warning",
                &format!(
                    "{} has no {} API key; runs on it will be refused until one is added",
                    text(&model, "name"),
                    provider.label
                ),
            );
        }
        Ok(keyed)
    }

    /// The model an agent uses, or why it has none.
    pub(crate) fn require_model(&self, agent: &Value) -> Result<Value, CoreError> {
        self.store
            .read(|state| by_id(&state.models, text(agent, "modelConnectionId")).cloned())
            .ok_or_else(|| CoreError::new(409, "Agent has no model connection"))
    }

    /// Changes a model connection. A new key also becomes the provider's saved key.
    pub async fn update_model(&self, id: &str, patch: &Value) -> Result<Value, CoreError> {
        let secret = text(patch, "secret");
        let before = self
            .store
            .read(|state| by_id(&state.models, id).cloned())
            .ok_or_else(|| CoreError::new(404, "Model connection not found"))?;
        // Refused before anything is saved, or the model would claim a key it does not have.
        if !secret.is_empty() && runs_on_plan(text(&before, "provider")) {
            return Err(CoreError::new(
                400,
                "This model runs on your own plan through its official program and takes no API key.",
            ));
        }
        let updated = self
            .store
            .update(|state| {
                let model = by_id_mut(&mut state.models, id)?;
                for key in ["name", "provider", "baseUrl"] {
                    if let Some(value) = patch.get(key).filter(|value| value.is_string()) {
                        model[key] = value.clone();
                    }
                }
                let chosen = js::trim(text(patch, "model"));
                if !chosen.is_empty() {
                    model["model"] = chosen.into();
                }
                if !secret.is_empty() {
                    model["hasSecret"] = true.into();
                }
                match patch.get("effort") {
                    Some(Value::Null) => remove(model, "effort"),
                    Some(effort) if serde_json::from_value::<Effort>(effort.clone()).is_ok() => {
                        model["effort"] = effort.clone()
                    }
                    _ => {}
                }
                Some(model.clone())
            })
            .await?;
        let updated = updated.ok_or_else(|| CoreError::new(404, "Model connection not found"))?;
        if !secret.is_empty() {
            self.broker
                .save_key(&connection_of(&updated), secret)
                .await?;
        }
        // Never the key itself; for a key, only that one was saved and that it now serves the whole provider.
        let mut fields: Vec<&str> = ["name", "provider", "model", "baseUrl", "effort"]
            .into_iter()
            .filter(|key| before.get(*key) != updated.get(*key))
            .collect();
        if !secret.is_empty() {
            fields.push("API key");
        }
        if !fields.is_empty() {
            let shared = if secret.is_empty() {
                String::new()
            } else {
                format!(" (now the saved {} key)", text(&updated, "provider"))
            };
            log(
                "catalog",
                "info",
                &format!(
                    "Model {} updated: {}{shared}",
                    text(&updated, "name"),
                    fields.join(", ")
                ),
            );
        }
        Ok(self.with_key(&updated))
    }

    /// A local server's environment variables, from the secrets file.
    pub(crate) fn command_env(&self, id: &str) -> BTreeMap<String, String> {
        self.store
            .secret(&env_secret_key(id))
            .and_then(|saved| serde_json::from_str(&saved).ok())
            .unwrap_or_default()
    }

    /// How to reach a server. A local one must have its current command approved.
    pub(crate) fn mcp_target(&self, server: &Value) -> Result<Target, CoreError> {
        let id = text(server, "id");
        let Some(command) = server.get("command").filter(|command| !command.is_null()) else {
            let auth = if text(server, "auth").is_empty() {
                "none"
            } else {
                text(server, "auth")
            };
            return Ok(Target {
                id: id.to_owned(),
                url: text(server, "url").to_owned(),
                auth: auth.to_owned(),
                command: None,
            });
        };
        let env = self.command_env(id);
        if text(server, "approvedCommand") != command_fingerprint(command, &env) {
            return Err(CoreError::new(
                409,
                format!(
                    "The MCP server \"{}\" runs a program on this computer ({}). Allow it in Settings → MCP Servers before it can start.",
                    text(server, "name"),
                    command_line(command)
                ),
            ));
        }
        let mut variables = env;
        // The app adds a PATH that finds npx and uvx when it was started from the Dock; the rest are what the user set.
        if !cfg!(windows) && !variables.contains_key("PATH") {
            let inherited = std::env::var("PATH").unwrap_or_default();
            variables.insert(
                "PATH".to_owned(),
                format!(
                    "{inherited}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
                ),
            );
        }
        Ok(Target {
            id: id.to_owned(),
            url: text(server, "url").to_owned(),
            auth: "none".to_owned(),
            command: Some(LocalCommand {
                program: text(command, "program").to_owned(),
                args: strings(command, "args"),
                env: variables,
                cwd: Some(self.store.root().join("workspace")),
            }),
        })
    }

    /// A server with what Settings shows about a local one: its command line, and whether it may run.
    pub(crate) fn mcp_view(&self, server: &Value) -> Value {
        let mut view = server.clone();
        if let Some(command) = server.get("command").filter(|command| !command.is_null()) {
            let print = command_fingerprint(command, &self.command_env(text(server, "id")));
            view["commandLine"] = command_line(command).into();
            view["approval"] = if text(server, "approvedCommand") == print {
                "approved"
            } else {
                "needed"
            }
            .into();
            view["commandFingerprint"] = print.into();
        }
        view
    }

    /// The language models the local model server at `base_url` has; an empty list while it is not running, since the
    /// settings form polls this.
    pub async fn local_server_models(&self, base_url: &str) -> Vec<Value> {
        let url = if base_url.is_empty() {
            "http://127.0.0.1:1234"
        } else {
            base_url
        };
        workbench_llm::lmstudio::list_models(self.broker.http(), url)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|id| json!({ "id": id, "label": id }))
            .collect()
    }

    /// The models to offer for a cloud provider, with `source` saying where they are from: `provider` for those it lists
    /// for a key, `program` for a program that runs on the person's own plan and takes the names it is given, and `none`
    /// with a note when there is no key to ask with or the provider did not answer. The app keeps no list of a provider's
    /// models of its own: one goes out of date with every release. `typed_key` is a key in the form that is not saved
    /// yet, and goes to the address typed with it, as connecting the model would send it. A saved key goes only to the
    /// provider's own address or to one a connected model of that provider already uses, never to an address given only
    /// here.
    pub async fn provider_models(
        &self,
        provider_id: &str,
        base_url: &str,
        typed_key: &str,
    ) -> Result<Value, CoreError> {
        let provider = llm_provider(provider_id)
            .filter(|provider| !runs_locally(provider.id))
            .ok_or_else(|| {
                CoreError::new(
                    404,
                    "No such cloud provider. A local server lists its models at its own route.",
                )
            })?;
        if runs_on_plan(provider.id) {
            let names: Vec<Value> = PROVIDERS
                .as_array()
                .and_then(|all| all.iter().find(|item| text(item, "id") == provider.id))
                .and_then(|item| item.get("models"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            return Ok(json!({
                "models": names,
                "source": "program",
                "note": format!(
                    "{} runs on your own plan and picks the model each name stands for. Any model id it accepts can be typed in.",
                    provider.label
                ),
            }));
        }
        let none = |note: String| json!({ "models": [], "source": "none", "note": note });
        let bare = |url: &str| url.trim().trim_end_matches('/').to_owned();
        let address = if base_url.trim().is_empty() {
            bare(provider.base_url)
        } else {
            bare(base_url)
        };
        let typed_key = typed_key.trim();
        let key = if typed_key.is_empty() {
            let known = address == bare(provider.base_url)
                || self.store.read(|state| {
                    state.models.iter().any(|model| {
                        text(model, "provider") == provider.id
                            && bare(text(model, "baseUrl")) == address
                    })
                });
            if !known {
                return Err(CoreError::new(
                    400,
                    format!(
                        "The {} key is sent only to its own address or to one a connected {} model uses.",
                        provider.label, provider.label
                    ),
                ));
            }
            let Some(key) = self.broker.provider_key(provider.id) else {
                return Ok(none(format!(
                    "Enter your {} API key to list the models it can use, or type a model id.",
                    provider.label
                )));
            };
            key
        } else {
            Url::parse(&address)
                .ok()
                .filter(|url| matches!(url.scheme(), "https" | "http"))
                .ok_or_else(|| CoreError::new(400, "The address must be a web address."))?;
            typed_key.to_owned()
        };
        let listed = match workbench_llm::listing::list_models(
            self.broker.http(),
            provider.id,
            &address,
            &key,
        )
        .await
        {
            Some(Ok(listed)) if !listed.is_empty() => listed,
            Some(Ok(_)) | None => {
                return Ok(none(format!(
                    "{} listed no models for this key. A model id can still be typed in.",
                    provider.label
                )));
            }
            Some(Err(error)) => {
                log(
                    "catalog",
                    "warning",
                    &format!(
                        "Could not list {}'s models: {}",
                        provider.label, error.message
                    ),
                );
                return Ok(none(format!(
                    "{} did not list its models ({}). A model id can still be typed in.",
                    provider.label, error.message
                )));
            }
        };
        log(
            "catalog",
            "info",
            &format!(
                "{} listed {} models for {}",
                provider.label,
                listed.len(),
                if typed_key.is_empty() {
                    "its saved key"
                } else {
                    "the key in the form"
                }
            ),
        );
        let models: Vec<Value> = listed.iter().map(listed_choice).collect();
        Ok(json!({ "models": models, "source": "provider" }))
    }
}
