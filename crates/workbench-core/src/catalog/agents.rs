//! Agent CRUD operations for `Kernel`.
use super::*;

impl Kernel {
    pub async fn list_agents(&self) -> Result<Vec<Value>, CoreError> {
        self.ensure_builtins().await?;
        Ok(self.store.read(|state| state.agents.clone()))
    }

    /// Creates an agent. Without a list of its own it gets every server there is, except the research tools, which are asked for.
    pub async fn create_agent(&self, input: &Value) -> Result<Value, CoreError> {
        self.ensure_builtins().await?;
        let servers = if input.get("mcpConnectionIds").is_some_and(Value::is_array) {
            strings(input, "mcpConnectionIds")
        } else {
            self.store.read(|state| {
                state
                    .mcp
                    .iter()
                    .map(|item| text(item, "id").to_owned())
                    .filter(|id| id != GENERAL_SKILLS_ID && id != RESEARCH_ID)
                    .collect()
            })
        };
        let agent = json!({
            "id": new_id("agent")?,
            "name": text(input, "name"),
            "description": or(input, "description", json!("")),
            "instructions": or(input, "instructions", json!("")),
            "autonomy": or(input, "autonomy", json!("execute_when_allowed")),
            "temperature": or(input, "temperature", json!(0.2)),
            "maxOutput": or(input, "maxOutput", json!(4000)),
            "status": "active",
            "modelConnectionId": or(input, "modelConnectionId", Value::Null),
            "mcpConnectionIds": with_general_skills(servers),
            "ragConnectionIds": or(input, "ragConnectionIds", json!([])),
        });
        let saved = agent.clone();
        self.store.update(|state| state.agents.push(saved)).await?;
        let model = self.model_of(&agent);
        let on = model.map_or(" with no model".to_owned(), |model| {
            format!(" on {}", text(&model, "name"))
        });
        log(
            "catalog",
            "info",
            &format!("Agent \"{}\" created{on}", text(&agent, "name")),
        );
        Ok(agent)
    }

    /// A model connection with what its provider says of the model: what it accepts and, when the connection has none,
    /// its context window. A model connected from the provider's list has it already; for any other, Anthropic is asked
    /// once, the first time the model is used, and the answer is saved on the connection, so the request code needs no
    /// rule about a model's name. A provider that cannot be asked
    /// (no key, no such list at a custom address, no network) leaves the connection as it is until the runtime starts
    /// again.
    pub(crate) async fn described(&self, model: &Value) -> Value {
        let connection = connection_of(model);
        if connection.provider != "claude"
            || model.get("capabilities").is_some()
            || !locked(&self.described).insert(connection.id.clone())
        {
            return model.clone();
        }
        let Some(key) = self
            .broker
            .key_for(&connection)
            .filter(|key| !key.is_empty())
        else {
            return model.clone();
        };
        let address = if connection.base_url.is_empty() {
            llm_provider("claude").map_or("", |provider| provider.base_url)
        } else {
            connection.base_url.as_str()
        };
        let said = match workbench_llm::listing::describe_model(
            self.broker.http(),
            "claude",
            address,
            &key,
            &connection.model,
        )
        .await
        {
            Some(Ok(said)) => said,
            Some(Err(error)) => {
                log(
                    "catalog",
                    "info",
                    &format!(
                        "Claude did not say what {} accepts ({}); it is treated as a current model",
                        connection.name,
                        error
                            .status
                            .map_or("no answer".to_owned(), |status| status.to_string())
                    ),
                );
                return model.clone();
            }
            None => return model.clone(),
        };
        let mut described = model.clone();
        described["capabilities"] = json!(said.capabilities);
        if described.get("contextTokens").is_none()
            && let Some(tokens) = said.context_tokens
        {
            described["contextTokens"] = tokens.into();
        }
        let (capabilities, window) = (
            described["capabilities"].clone(),
            described.get("contextTokens").cloned(),
        );
        let id = connection.id.clone();
        let saved = self
            .store
            .update(|state| {
                if let Some(target) = by_id_mut(&mut state.models, &id) {
                    target["capabilities"] = capabilities;
                    if let Some(window) = window {
                        target["contextTokens"] = window;
                    }
                }
            })
            .await;
        if let Err(error) = saved {
            log(
                "catalog",
                "warning",
                &format!(
                    "Could not save what Claude says of {}: {error}",
                    connection.name
                ),
            );
        }
        described
    }

    /// Changes an agent. Its permission rules are edited through /permissions, not here.
    pub async fn update_agent(&self, id: &str, patch: &Value) -> Result<Value, CoreError> {
        let before = self.get_agent(id)?;
        let changed = self
            .store
            .update(|state| {
                let agent = by_id_mut(&mut state.agents, id)?;
                let fields = [
                    "name",
                    "description",
                    "instructions",
                    "autonomy",
                    "temperature",
                    "maxOutput",
                    "status",
                    "ragConnectionIds",
                ];
                for key in fields {
                    if let Some(value) = patch.get(key).filter(|value| !value.is_null()) {
                        agent[key] = value.clone();
                    }
                }
                // Null clears the model; only a missing field keeps it.
                if let Some(model) = patch.get("modelConnectionId") {
                    agent["modelConnectionId"] = model.clone();
                }
                if patch.get("mcpConnectionIds").is_some_and(Value::is_array) {
                    agent["mcpConnectionIds"] =
                        with_general_skills(strings(patch, "mcpConnectionIds")).into();
                }
                Some(agent.clone())
            })
            .await?;
        let agent = changed.ok_or_else(|| CoreError::new(404, "Agent not found"))?;
        // Field names only: instructions and descriptions can be long or private. Nothing changed, nothing to say.
        let names: Vec<&str> = agent
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, value)| before.get(key.as_str()) != Some(*value))
            .map(|(key, _)| key.as_str())
            .collect();
        if !names.is_empty() {
            log(
                "catalog",
                "info",
                &format!(
                    "Agent \"{}\" updated: {}",
                    text(&agent, "name"),
                    names.join(", ")
                ),
            );
        }
        Ok(agent)
    }

    /// Deletes an agent with its runs and chats.
    pub async fn delete_agent(&self, id: &str) -> Result<(), CoreError> {
        let agent = self.get_agent(id)?;
        let runs = self.store.read(|state| {
            state
                .runs
                .iter()
                .filter(|item| text(item, "agentId") == id)
                .count()
        });
        self.store
            .update(|state| {
                state.agents.retain(|item| text(item, "id") != id);
                state.runs.retain(|item| text(item, "agentId") != id);
                state.threads.retain(|item| text(item, "agentId") != id);
            })
            .await?;
        log(
            "catalog",
            "info",
            &format!(
                "Agent \"{}\" deleted with {runs} runs",
                text(&agent, "name")
            ),
        );
        Ok(())
    }
}
