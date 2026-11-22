//! MCP server, RAG collection, and shared connection lifecycle operations for `Kernel`.
use super::*;

impl Kernel {
    /// The connections of one kind. Models say whether they have a key.
    pub async fn list_connections(&self, kind: &str) -> Result<Vec<Value>, CoreError> {
        match kind {
            "models" => Ok(self.list_models()),
            "mcp" => {
                self.ensure_builtins().await?;
                let servers = self.store.read(|state| state.mcp.clone());
                Ok(servers.iter().map(|server| self.mcp_view(server)).collect())
            }
            "rag" => Ok(self.store.read(|state| state.rag.clone())),
            _ => Err(CoreError::new(404, "Unknown connection kind")),
        }
    }

    /// Adds an MCP server by URL, or a local one, which waits until the user has allowed its exact command.
    pub async fn create_mcp(&self, input: &Value) -> Result<Value, CoreError> {
        let id = new_id("mcp")?;
        let tools = routes(&id, input);
        if let Some(command) = input.get("command").filter(|command| !command.is_null()) {
            let (command, env) = parse_command(command)?;
            if !env.is_empty() {
                self.store
                    .put_secret(&env_secret_key(&id), &json!(env).to_string())
                    .await?;
            }
            let given = js::trim(text(input, "name"));
            let program = text(&command, "program");
            let name = if given.is_empty() {
                program.rsplit(['/', '\\']).next().unwrap_or(program)
            } else {
                given
            };
            let connection = json!({ "id": id, "name": name, "url": command_address(&command), "command": command, "tools": tools, "origin": "local" });
            let saved = connection.clone();
            self.store.update(|state| state.mcp.push(saved)).await?;
            log(
                "catalog",
                "info",
                &format!(
                    "Local MCP server {name} added ({}); it waits for approval before it starts",
                    command_line(&command)
                ),
            );
            return Ok(self.mcp_view(&connection));
        }
        let url = mcp_endpoint(text(input, "url"))?;
        let connection = json!({ "id": id, "name": text(input, "name"), "url": url, "tools": tools, "origin": "local" });
        let saved = connection.clone();
        self.store.update(|state| state.mcp.push(saved)).await?;
        log(
            "catalog",
            "info",
            &format!(
                "MCP server {} added at {} with {} tools",
                text(&connection, "name"),
                host_of(&url),
                connection["tools"].as_array().map_or(0, Vec::len)
            ),
        );
        Ok(connection)
    }

    /// Allows a local server's command to run. `fingerprint` is the one Settings showed the user; if the command changed since,
    /// nothing is allowed and the user is asked to look again.
    pub async fn approve_mcp(&self, id: &str, fingerprint: &str) -> Result<Value, CoreError> {
        let server = self
            .store
            .read(|state| by_id(&state.mcp, id).cloned())
            .ok_or_else(|| CoreError::new(404, "MCP connection not found"))?;
        let command = server
            .get("command")
            .filter(|command| !command.is_null())
            .ok_or_else(|| CoreError::new(400, "Only local MCP servers need approval"))?;
        let current = command_fingerprint(command, &self.command_env(id));
        if fingerprint != current {
            return Err(CoreError::new(
                409,
                "The command changed since you looked at it. Review it again before allowing it.",
            ));
        }
        let updated = self
            .store
            .update(|state| {
                let item = by_id_mut(&mut state.mcp, id)?;
                item["approvedCommand"] = current.as_str().into();
                Some(item.clone())
            })
            .await?
            .unwrap_or(server.clone());
        log(
            "catalog",
            "info",
            &format!(
                "Allowed the local MCP server {} to run: {}",
                text(&server, "name"),
                command_line(command)
            ),
        );
        Ok(self.mcp_view(&updated))
    }

    /// Changes an MCP server. A new command (or new variables) starts over: the old approval no longer applies.
    pub async fn update_mcp(&self, id: &str, patch: &Value) -> Result<Value, CoreError> {
        if id == GENERAL_SKILLS_ID
            && ["name", "url", "tools"]
                .iter()
                .any(|key| patch.get(*key).is_some_and(|value| !value.is_null()))
        {
            return Err(CoreError::new(403, "General Skills is built in"));
        }
        let url = patch
            .get("url")
            .filter(|url| !url.is_null())
            .map(|url| mcp_endpoint(url.as_str().unwrap_or("")))
            .transpose()?;
        let changed = patch
            .get("command")
            .filter(|command| !command.is_null())
            .map(parse_command)
            .transpose()?;
        if let Some((_, env)) = &changed {
            let local = self.store.read(|state| {
                by_id(&state.mcp, id).is_some_and(|server| {
                    server
                        .get("command")
                        .is_some_and(|command| !command.is_null())
                })
            });
            if !local {
                return Err(CoreError::new(
                    400,
                    "This MCP server is reached by its URL; add a local server separately.",
                ));
            }
            if env.is_empty() {
                self.store.delete_secret(&env_secret_key(id)).await?;
            } else {
                self.store
                    .put_secret(&env_secret_key(id), &json!(env).to_string())
                    .await?;
            }
            self.mcp.close(id);
        }
        let updated = self
            .store
            .update(|state| {
                let current = by_id_mut(&mut state.mcp, id)?;
                if let Some(name) = patch.get("name").filter(|name| name.is_string()) {
                    current["name"] = name.clone();
                }
                if let Some(url) = url {
                    current["url"] = url.into();
                }
                if let Some((command, _)) = changed {
                    current["url"] = command_address(&command).into();
                    current["command"] = command;
                    remove(current, "approvedCommand");
                }
                if let Some(disabled) = patch
                    .get("disabledTools")
                    .filter(|disabled| disabled.is_array())
                {
                    current["disabledTools"] = disabled.clone();
                }
                if patch.get("tools").is_some_and(Value::is_array) {
                    current["tools"] = routes(id, patch).into();
                }
                Some(current.clone())
            })
            .await?
            .ok_or_else(|| CoreError::new(404, "MCP connection not found"))?;
        log(
            "catalog",
            "info",
            &format!("MCP server {} updated", text(&updated, "name")),
        );
        Ok(self.mcp_view(&updated))
    }

    /// Removes an MCP server from every agent, with its saved environment.
    pub async fn delete_mcp_server(&self, id: &str) -> Result<(), CoreError> {
        if [GENERAL_SKILLS_ID, COMPUTER_USE_ID, ORIGIN_ID].contains(&id) {
            return Err(CoreError::new(403, "Built-in server cannot be removed"));
        }
        let server = self
            .store
            .read(|state| by_id(&state.mcp, id).cloned())
            .ok_or_else(|| CoreError::new(404, "MCP server not found"))?;
        self.store
            .update(|state| {
                state.mcp.retain(|item| text(item, "id") != id);
                for agent in &mut state.agents {
                    let kept: Vec<String> = strings(agent, "mcpConnectionIds")
                        .into_iter()
                        .filter(|item| item != id)
                        .collect();
                    agent["mcpConnectionIds"] = kept.into();
                }
            })
            .await?;
        // Its session (a local server's process) and any saved sign-in or environment go with it.
        self.mcp.forget(id).await?;
        self.store.delete_secret(&env_secret_key(id)).await?;
        log(
            "catalog",
            "info",
            &format!(
                "MCP server {} ({}) removed and detached from all agents",
                text(&server, "name"),
                host_of(text(&server, "url"))
            ),
        );
        Ok(())
    }

    /// Removes a connection the agents using it can do without: a model's move to `move_to` ("none" leaves them with no
    /// model, which they say when asked to run), and a server or index is taken off their lists. Done only when asked,
    /// since `delete_connection` alone refuses; an agent may no longer have a screen of its own to change it on.
    pub async fn release_connection(
        &self,
        kind: &str,
        id: &str,
        move_to: Option<&str>,
    ) -> Result<(), CoreError> {
        if !matches!(kind, "models" | "mcp" | "rag") {
            return Err(CoreError::new(404, "Unknown connection kind"));
        }
        let target = if kind == "models" {
            let asked = move_to.ok_or_else(|| {
                CoreError::new(400, "Say which model its agents move to, or \"none\".")
            })?;
            if asked == id {
                return Err(CoreError::new(
                    400,
                    "Its agents cannot move to the model being removed.",
                ));
            }
            if asked != "none"
                && !self
                    .store
                    .read(|state| state.models.iter().any(|model| text(model, "id") == asked))
            {
                return Err(CoreError::new(404, "No such model to move its agents to."));
            }
            (asked != "none").then(|| asked.to_owned())
        } else {
            None
        };
        let moved = self
            .store
            .update(|state| {
                let mut moved = 0;
                for agent in &mut state.agents {
                    match kind {
                        "models" if text(agent, "modelConnectionId") == id => {
                            agent["modelConnectionId"] = json!(target);
                            moved += 1;
                        }
                        "models" => {}
                        _ => {
                            let key = if kind == "mcp" {
                                "mcpConnectionIds"
                            } else {
                                "ragConnectionIds"
                            };
                            let kept: Vec<String> = strings(agent, key)
                                .into_iter()
                                .filter(|item| item != id)
                                .collect();
                            if kept.len() != strings(agent, key).len() {
                                agent[key] = json!(kept);
                                moved += 1;
                            }
                        }
                    }
                }
                moved
            })
            .await?;
        if moved > 0 {
            log(
                "catalog",
                "info",
                &format!("{moved} agents released a {kind} connection that is being removed"),
            );
        }
        self.delete_connection(kind, id).await
    }

    /// Removes a connection no agent uses.
    pub async fn delete_connection(&self, kind: &str, id: &str) -> Result<(), CoreError> {
        if !matches!(kind, "models" | "mcp" | "rag") {
            return Err(CoreError::new(404, "Unknown connection kind"));
        }
        let used = self.store.read(|state| {
            state.agents.iter().any(|agent| match kind {
                "models" => text(agent, "modelConnectionId") == id,
                "mcp" => strings(agent, "mcpConnectionIds")
                    .iter()
                    .any(|item| item == id),
                _ => strings(agent, "ragConnectionIds")
                    .iter()
                    .any(|item| item == id),
            })
        });
        if used {
            return Err(CoreError::new(409, "Connection is still used by an agent"));
        }
        self.store
            .update(|state| {
                let list = match kind {
                    "models" => &mut state.models,
                    "mcp" => &mut state.mcp,
                    _ => &mut state.rag,
                };
                list.retain(|item| text(item, "id") != id);
                // The background model is cleared rather than left pointing at a model that is gone.
                if kind == "models"
                    && state
                        .settings
                        .get("completionModelId")
                        .and_then(Value::as_str)
                        == Some(id)
                {
                    state
                        .settings
                        .insert("completionModelId".to_owned(), Value::Null);
                }
            })
            .await?;
        // The provider's shared key stays, so other models of that provider keep working.
        if kind == "models" {
            self.store.delete_secret(id).await?;
        }
        Ok(())
    }

    /// Asks a server what tools it has now. The built-in ones are their own list.
    /// A remote server's tools. One that answers that it needs a sign-in, and says where (OAuth), is signed in to in the
    /// browser, as the user asked to connect it; from then on it is an OAuth server. The browser then comes back to
    /// /oauth/callback, and the next discovery lists the tools.
    async fn listed_tools(&self, id: &str, server: &Value) -> Result<Vec<Value>, CoreError> {
        let target = self.mcp_target(server)?;
        let first = self.mcp.list_tools(&target).await;
        let needs_sign_in = matches!(&first, Err(McpError::Failed(message)) if message == "The server requires sign-in");
        if !(needs_sign_in && target.auth == "none" && target.command.is_none()) {
            return first.map_err(Into::into);
        }
        let signed = Target {
            auth: "oauth".to_owned(),
            ..target
        };
        let second = self.mcp.list_tools(&signed).await;
        if matches!(&second, Ok(_) | Err(McpError::SignIn(_))) {
            self.store
                .update(|state| {
                    if let Some(found) = by_id_mut(&mut state.mcp, id) {
                        found["auth"] = "oauth".into();
                    }
                })
                .await?;
        }
        match second {
            Err(McpError::SignIn(_)) => Err(CoreError::new(
                409,
                "The server needs you to sign in: finish it in the browser page that opened, then connect again.",
            )),
            Err(McpError::Failed(_)) => first.map_err(Into::into),
            Ok(tools) => Ok(tools),
        }
    }

    pub async fn discover_mcp(&self, id: &str) -> Result<Value, CoreError> {
        if id == GENERAL_SKILLS_ID {
            return Err(CoreError::new(403, "General Skills is built in"));
        }
        let server = self
            .store
            .read(|state| by_id(&state.mcp, id).cloned())
            .ok_or_else(|| CoreError::new(404, "MCP connection not found"))?;
        let tools = if [COMPUTER_USE_ID, ORIGIN_ID, CUSTOMIZATIONS_ID].contains(&id) {
            builtin_server(id)["tools"].clone()
        } else {
            let listed = self.listed_tools(id, &server).await?;
            let market = text(&server, "marketplaceId");
            let routed: Vec<Value> = listed
                .into_iter()
                .map(|mut tool| {
                    tool["serverId"] = id.into();
                    if !market.is_empty() {
                        let remote = text(&tool, "toolId").to_owned();
                        tool["toolId"] = namespaced_tool(market, &remote).into();
                        tool["remoteName"] = remote.into();
                    }
                    tool
                })
                .collect();
            if routed.is_empty() {
                log(
                    "catalog",
                    "warning",
                    &format!(
                        "MCP server {} at {} returned no tools",
                        text(&server, "name"),
                        host_of(text(&server, "url"))
                    ),
                );
            }
            routed.into()
        };
        Ok(self
            .store
            .update(|state| {
                let current = by_id_mut(&mut state.mcp, id)?;
                current["tools"] = tools;
                Some(current.clone())
            })
            .await?
            .unwrap_or(Value::Null))
    }

    pub async fn create_rag(&self, input: &Value) -> Result<Value, CoreError> {
        let connection =
            json!({ "id": new_id("rag")?, "name": text(input, "name"), "url": text(input, "url") });
        let saved = connection.clone();
        self.store.update(|state| state.rag.push(saved)).await?;
        log(
            "catalog",
            "info",
            &format!(
                "Knowledge source {} added at {}",
                text(&connection, "name"),
                host_of(text(&connection, "url"))
            ),
        );
        Ok(connection)
    }

    pub async fn update_rag(&self, id: &str, patch: &Value) -> Result<Value, CoreError> {
        self.store
            .update(|state| {
                let current = by_id_mut(&mut state.rag, id)?;
                for key in ["name", "url"] {
                    if let Some(value) = patch.get(key).filter(|value| value.is_string()) {
                        current[key] = value.clone();
                    }
                }
                Some(current.clone())
            })
            .await?
            .ok_or_else(|| CoreError::new(404, "RAG connection not found"))
    }
}
