//! Hosted GitHub REST and GraphQL operations for `Origin`.
use super::*;

impl Origin<'_> {
    // GitHub

    /// The user's own GitHub token, from the environment, the GitHub install in Customizations, or the gh CLI.
    pub(super) async fn token(&self) -> Option<String> {
        self.token
            .get_or_init(|| async {
                let set = |name: &str| std::env::var(name).ok().filter(|token| !token.is_empty());
                if let Some(token) = set("GH_TOKEN").or_else(|| set("GITHUB_TOKEN")).or_else(|| {
                    self.kernel
                        .store
                        .secret("mcp:mkt-github:token")
                        .filter(|token| !token.is_empty())
                }) {
                    return Some(token);
                }
                let mut command = Command::new("gh");
                workbench_tools::no_window(&mut command);
                command
                    .args(["auth", "token"])
                    .stdin(Stdio::null())
                    .kill_on_drop(true);
                let output = tokio::time::timeout(Duration::from_secs(5), command.output())
                    .await
                    .ok()?
                    .ok()?;
                let token = String::from_utf8_lossy(&output.stdout);
                let token = js::trim(&token);
                (output.status.success() && !token.is_empty()).then(|| token.to_owned())
            })
            .await
            .clone()
    }

    pub(super) async fn request(
        &self,
        method: Method,
        route: &str,
        body: Option<&Value>,
        accept: &str,
    ) -> Result<String, Failure> {
        let Some(token) = self.token().await else {
            return Err("GitHub access is needed for this. Set GH_TOKEN, run `gh auth login`, or install GitHub in Customizations with a token.".to_owned());
        };
        let base = std::env::var("WORKBENCH_GITHUB_API")
            .ok()
            .filter(|base| !base.is_empty())
            .unwrap_or_else(|| "https://api.github.com".to_owned());
        let client = client_builder()
            .timeout(API_TIMEOUT)
            .build()
            .map_err(|error| error.to_string())?;
        let mut request = client
            .request(method, format!("{base}{route}"))
            .header("Accept", accept)
            .bearer_auth(token)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "Eisoptric-Origin");
        if let Some(body) = body {
            request = request
                .header("Content-Type", "application/json")
                .body(body.to_string());
        }
        let response = request
            .send_noted()
            .await
            .map_err(|error| format!("Could not reach GitHub: {}", error.without_url()))?;
        let status = response.status();
        if status.as_u16() == 204 {
            return Ok("{}".to_owned());
        }
        let text = response
            .text()
            .await
            .map_err(|error| format!("Could not reach GitHub: {}", error.without_url()))?;
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&text).map_or_else(
                |_| text.clone(),
                |parsed| {
                    let mut parts: Vec<String> = Vec::new();
                    if truthy(parsed.get("message")) {
                        parts.push(js::text(parsed.get("message")));
                    }
                    for item in items(parsed.get("errors").unwrap_or(&Value::Null)) {
                        if truthy(item.get("message")) {
                            parts.push(js::text(item.get("message")));
                        }
                    }
                    parts.join(": ")
                },
            );
            return Err(format!(
                "GitHub {}: {}",
                status.as_u16(),
                js::prefix(&message, 400)
            ));
        }
        Ok(text)
    }

    pub(super) async fn api(&self, method: Method, route: &str, body: Option<&Value>) -> Answer {
        let text = self.request(method, route, body, JSON_ACCEPT).await?;
        serde_json::from_str(&text)
            .map_err(|error| format!("GitHub's answer could not be read: {error}"))
    }

    pub(super) async fn get(&self, route: &str) -> Answer {
        self.api(Method::GET, route, None).await
    }

    pub(super) async fn graphql(&self, query: &str, variables: Value) -> Answer {
        let result = self
            .api(
                Method::POST,
                "/graphql",
                Some(&json!({ "query": query, "variables": variables })),
            )
            .await?;
        let errors = items(result.get("errors").unwrap_or(&Value::Null));
        if !errors.is_empty() {
            let messages: Vec<String> = errors
                .iter()
                .map(|item| js::text(item.get("message")))
                .collect();
            return Err(format!("GitHub: {}", messages.join("; ")));
        }
        Ok(result.get("data").cloned().unwrap_or(Value::Null))
    }

    fn repo_route(args: &Value) -> Result<String, Failure> {
        let (owner, name) = Self::repo_of(args)?;
        Ok(format!("/repos/{}/{}", encode(&owner), encode(&name)))
    }

    fn pull_number(args: &Value) -> Result<i64, Failure> {
        match number_of(args.get("number")) {
            Some(number) if number.fract() == 0.0 && number > 0.0 => Ok(number as i64),
            _ => Err("number must be a pull request number".to_owned()),
        }
    }

    pub(super) async fn get_repository(&self, args: &Value) -> Answer {
        let repo = self.get(&Self::repo_route(args)?).await?;
        Ok(fields(&[
            ("owner", at(&repo, &["owner", "login"])),
            ("name", repo.get("name")),
            ("description", repo.get("description")),
            ("defaultBranch", repo.get("default_branch")),
            ("visibility", repo.get("visibility")),
            ("archived", repo.get("archived")),
            ("cloneUrl", repo.get("clone_url")),
            ("url", repo.get("html_url")),
            ("createdAt", repo.get("created_at")),
            ("updatedAt", repo.get("pushed_at")),
        ]))
    }

    pub(super) async fn list_repositories(&self, args: &Value) -> Answer {
        let owner = arg(args, "owner");
        if owner.is_empty() {
            return Err("owner is required".to_owned());
        }
        let (size, at_page) = (page_size(args, 30.0), page(args));
        let me = self.get("/user").await?;
        let route = if js::text(me.get("login")).to_lowercase() == owner.to_lowercase() {
            format!("/user/repos?affiliation=owner&sort=updated&per_page={size}&page={at_page}")
        } else {
            format!(
                "/users/{}/repos?sort=updated&per_page={size}&page={at_page}",
                encode(owner)
            )
        };
        let repos = self.get(&route).await?;
        let filter = arg(args, "filter").to_lowercase();
        let shown: Vec<Value> = items(&repos)
            .iter()
            .filter(|repo| {
                filter.is_empty() || js::text(repo.get("name")).to_lowercase().contains(&filter)
            })
            .map(|repo| {
                fields(&[
                    ("owner", at(repo, &["owner", "login"])),
                    ("name", repo.get("name")),
                    ("visibility", repo.get("visibility")),
                    ("defaultBranch", repo.get("default_branch")),
                    ("updatedAt", repo.get("pushed_at")),
                    ("url", repo.get("html_url")),
                ])
            })
            .collect();
        Ok(
            json!({ "repositories": shown, "nextPageToken": next_page(items(&repos).len(), size, at_page) }),
        )
    }

    pub(super) async fn list_namespaces(&self, args: &Value) -> Answer {
        let (size, at_page) = (page_size(args, 30.0), page(args));
        let me = self.get("/user").await?;
        let orgs = self
            .get(&format!("/user/orgs?per_page={size}&page={at_page}"))
            .await?;
        let mut namespaces = Vec::new();
        if at_page == 1 {
            let login = me.get("login").cloned().unwrap_or(Value::Null);
            let name = if truthy(me.get("name")) {
                me["name"].clone()
            } else {
                login.clone()
            };
            namespaces.push(json!({ "slug": login, "name": name, "type": "user" }));
        }
        for org in items(&orgs) {
            namespaces.push(json!({ "slug": org.get("login"), "name": org.get("login"), "type": "organization" }));
        }
        Ok(
            json!({ "namespaces": namespaces, "nextPageToken": next_page(items(&orgs).len(), size, at_page) }),
        )
    }

    pub(super) async fn create_repository(&self, args: &Value) -> Answer {
        let name = arg(args, "name");
        if name.is_empty() {
            return Err("name is required".to_owned());
        }
        let name = plain_name(name)?;
        let me = self.get("/user").await?;
        let login = js::text(me.get("login"));
        let owner = plain_name(if arg(args, "owner").is_empty() {
            &login
        } else {
            arg(args, "owner")
        })?;
        let route = if owner.eq_ignore_ascii_case(&login) {
            "/user/repos".to_owned()
        } else {
            format!("/orgs/{}/repos", encode(&owner))
        };
        // Private by default: an agent should not publish a repository on its own.
        let repo = self
            .api(
                Method::POST,
                &route,
                Some(&json!({ "name": name, "private": true, "auto_init": true })),
            )
            .await?;
        let wanted = arg(args, "defaultBranch");
        let current = js::text(repo.get("default_branch"));
        if !wanted.is_empty() && wanted != current {
            self.api(
                Method::POST,
                &format!("/repos/{owner}/{name}/branches/{current}/rename"),
                Some(&json!({ "new_name": wanted })),
            )
            .await?;
        }
        let branch = if wanted.is_empty() {
            repo.get("default_branch").cloned()
        } else {
            Some(json!(wanted))
        };
        Ok(fields(&[
            ("owner", at(&repo, &["owner", "login"])),
            ("name", repo.get("name")),
            ("visibility", repo.get("visibility")),
            ("defaultBranch", branch.as_ref()),
            ("cloneUrl", repo.get("clone_url")),
            ("url", repo.get("html_url")),
        ]))
    }

    pub(super) async fn list_pull_requests(&self, args: &Value) -> Answer {
        let (size, at_page) = (page_size(args, 30.0), page(args));
        let (owner, _) = Self::repo_of(args)?;
        let state = arg(args, "state");
        let query = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query
                .append_pair(
                    "state",
                    if ["open", "closed", "all"].contains(&state) {
                        state
                    } else {
                        "open"
                    },
                )
                .append_pair(
                    "sort",
                    if arg(args, "sortBy") == "updated" {
                        "updated"
                    } else {
                        "created"
                    },
                )
                .append_pair(
                    "direction",
                    if arg(args, "direction") == "asc" {
                        "asc"
                    } else {
                        "desc"
                    },
                )
                .append_pair("per_page", &size.to_string())
                .append_pair("page", &at_page.to_string());
            if !arg(args, "base").is_empty() {
                query.append_pair("base", arg(args, "base"));
            }
            let head = arg(args, "head");
            if !head.is_empty() {
                let qualified = if head.contains(':') {
                    head.to_owned()
                } else {
                    format!("{owner}:{head}")
                };
                query.append_pair("head", &qualified);
            }
            query.finish()
        };
        let pulls = self
            .get(&format!("{}/pulls?{query}", Self::repo_route(args)?))
            .await?;
        let author = arg(args, "author").to_lowercase();
        let shown: Vec<Value> = items(&pulls)
            .iter()
            .filter(|pr| {
                author.is_empty() || js::text(at(pr, &["user", "login"])).to_lowercase() == author
            })
            .map(|pr| {
                let mut summary = pull(pr);
                if let Some(object) = summary.as_object_mut() {
                    object.remove("body");
                }
                summary
            })
            .collect();
        Ok(
            json!({ "pullRequests": shown, "nextPageToken": next_page(items(&pulls).len(), size, at_page) }),
        )
    }

    pub(super) async fn pull_request_read(&self, args: &Value) -> Answer {
        let number = Self::pull_number(args)?;
        let route = format!("{}/pulls/{number}", Self::repo_route(args)?);
        let (size, at_page) = (page_size(args, 30.0), page(args));
        let paging = format!("per_page={size}&page={at_page}");
        match arg(args, "view") {
            "files" => {
                let files = self.get(&format!("{route}/files?{paging}")).await?;
                let shown: Vec<Value> = items(&files)
                    .iter()
                    .map(|file| {
                        fields(&[
                            ("path", file.get("filename")),
                            ("status", file.get("status")),
                            ("additions", file.get("additions")),
                            ("deletions", file.get("deletions")),
                            ("previousPath", file.get("previous_filename")),
                            ("patch", file.get("patch")),
                        ])
                    })
                    .collect();
                Ok(
                    json!({ "files": shown, "nextPageToken": next_page(items(&files).len(), size, at_page) }),
                )
            }
            "commits" => {
                let commits = self.get(&format!("{route}/commits?{paging}")).await?;
                let shown: Vec<Value> = items(&commits)
                    .iter()
                    .map(|commit| {
                        fields(&[
                            ("sha", commit.get("sha")),
                            ("message", at(commit, &["commit", "message"])),
                            ("author", at(commit, &["commit", "author", "name"])),
                            ("date", at(commit, &["commit", "author", "date"])),
                        ])
                    })
                    .collect();
                Ok(
                    json!({ "commits": shown, "nextPageToken": next_page(items(&commits).len(), size, at_page) }),
                )
            }
            "reviews" => {
                let reviews = self.get(&format!("{route}/reviews?{paging}")).await?;
                let shown: Vec<Value> = items(&reviews)
                    .iter()
                    .map(|review| {
                        let id = json!(id_of(review.get("id")));
                        fields(&[
                            ("id", Some(&id)),
                            ("author", at(review, &["user", "login"])),
                            ("state", review.get("state")),
                            ("body", review.get("body")),
                            ("submittedAt", review.get("submitted_at")),
                            ("commitId", review.get("commit_id")),
                        ])
                    })
                    .collect();
                Ok(
                    json!({ "reviews": shown, "nextPageToken": next_page(items(&reviews).len(), size, at_page) }),
                )
            }
            "comments" => {
                let (owner, name) = Self::repo_of(args)?;
                let comments = self
                    .get(&format!(
                        "/repos/{owner}/{name}/issues/{number}/comments?{paging}"
                    ))
                    .await?;
                let shown: Vec<Value> = items(&comments)
                    .iter()
                    .map(|comment| {
                        let id = json!(id_of(comment.get("id")));
                        fields(&[
                            ("id", Some(&id)),
                            ("author", at(comment, &["user", "login"])),
                            ("body", comment.get("body")),
                            ("createdAt", comment.get("created_at")),
                            ("updatedAt", comment.get("updated_at")),
                        ])
                    })
                    .collect();
                Ok(
                    json!({ "comments": shown, "nextPageToken": next_page(items(&comments).len(), size, at_page) }),
                )
            }
            "threads" => self.read_threads(args, number).await,
            _ => Ok(json!({ "pullRequest": pull(&self.get(&route).await?) })),
        }
    }

    pub(super) async fn read_threads(&self, args: &Value, number: i64) -> Answer {
        let (owner, name) = Self::repo_of(args)?;
        let token = args
            .get("pageToken")
            .map(|token| js::text(Some(token)))
            .unwrap_or_default();
        let after = if token.is_empty() || token.chars().all(|character| character.is_ascii_digit())
        {
            Value::Null
        } else {
            json!(token)
        };
        let data = self
            .graphql(
                "query($owner:String!,$name:String!,$number:Int!,$after:String){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:50,after:$after){nodes{id isResolved isOutdated path line startLine diffSide comments(first:50){nodes{databaseId author{login} body createdAt}}} pageInfo{hasNextPage endCursor}}}}}",
                json!({ "owner": owner, "name": name, "number": number, "after": after }),
            )
            .await?;
        let threads =
            at(&data, &["repository", "pullRequest", "reviewThreads"]).unwrap_or(&Value::Null);
        let wanted: Option<Vec<String>> = args
            .get("threadIds")
            .and_then(Value::as_array)
            .map(|ids| ids.iter().map(|id| js::text(Some(id))).collect());
        let shown: Vec<Value> = items(threads.get("nodes").unwrap_or(&Value::Null))
            .iter()
            .filter(|thread| {
                wanted
                    .as_ref()
                    .is_none_or(|wanted| wanted.contains(&js::text(thread.get("id"))))
            })
            .map(|thread| {
                let comments: Vec<Value> =
                    items(at(thread, &["comments", "nodes"]).unwrap_or(&Value::Null))
                        .iter()
                        .map(|comment| {
                            let id = json!(id_of(comment.get("databaseId")));
                            fields(&[
                                ("id", Some(&id)),
                                ("author", at(comment, &["author", "login"])),
                                ("body", comment.get("body")),
                                ("createdAt", comment.get("createdAt")),
                            ])
                        })
                        .collect();
                let comments = Value::Array(comments);
                fields(&[
                    ("id", thread.get("id")),
                    ("resolved", thread.get("isResolved")),
                    ("outdated", thread.get("isOutdated")),
                    ("path", thread.get("path")),
                    ("line", thread.get("line")),
                    ("startLine", thread.get("startLine")),
                    ("side", thread.get("diffSide")),
                    ("comments", Some(&comments)),
                ])
            })
            .collect();
        let next = if truthy(at(threads, &["pageInfo", "hasNextPage"])) {
            at(threads, &["pageInfo", "endCursor"])
                .cloned()
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        Ok(json!({ "threads": shown, "nextPageToken": next }))
    }

    pub(super) async fn checks_read(&self, args: &Value) -> Answer {
        let empty = Value::Null;
        let target = args
            .get("target")
            .filter(|target| target.is_object())
            .unwrap_or(&empty);
        let base = Self::repo_route(args)?;
        let (size, at_page) = (page_size(args, 30.0), page(args));
        let run_id = arg(target, "checkRunId");
        if !run_id.is_empty() {
            let run = self
                .get(&format!("{base}/check-runs/{}", encode(run_id)))
                .await?;
            let id = json!(id_of(run.get("id")));
            let check = fields(&[
                ("id", Some(&id)),
                ("name", run.get("name")),
                ("status", run.get("status")),
                ("conclusion", run.get("conclusion")),
                ("url", run.get("html_url")),
                ("output", at(&run, &["output", "summary"])),
            ]);
            return Ok(json!({ "checkRun": check }));
        }
        let mut sha = [arg(target, "sha"), arg(args, "sha"), arg(args, "ref")]
            .into_iter()
            .find(|sha| !sha.is_empty())
            .unwrap_or("")
            .to_owned();
        let number = number_of(target.get("number")).or_else(|| number_of(args.get("number")));
        if let Some(number) = number.filter(|number| sha.is_empty() && number.fract() == 0.0) {
            let pr = self.get(&format!("{base}/pulls/{number}")).await?;
            sha = js::text(at(&pr, &["head", "sha"]));
        }
        let suite = arg(target, "checkSuiteId");
        if sha.is_empty() && suite.is_empty() {
            return Err(
                "Give a pull request number, a sha, a check suite id or a check run id".to_owned(),
            );
        }
        let route = if suite.is_empty() {
            format!("{base}/commits/{}/check-runs", encode(&sha))
        } else {
            format!("{base}/check-suites/{}/check-runs", encode(suite))
        };
        let result = self
            .get(&format!("{route}?per_page={size}&page={at_page}"))
            .await?;
        let runs: Vec<Value> = items(result.get("check_runs").unwrap_or(&Value::Null))
            .iter()
            .map(|run| {
                let id = json!(id_of(run.get("id")));
                fields(&[
                    ("id", Some(&id)),
                    ("name", run.get("name")),
                    ("status", run.get("status")),
                    ("conclusion", run.get("conclusion")),
                    ("startedAt", run.get("started_at")),
                    ("completedAt", run.get("completed_at")),
                    ("url", run.get("html_url")),
                ])
            })
            .collect();
        let total = result
            .get("total_count")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let more = at_page * size < total;
        let mut answer = json!({
            "sha": if sha.is_empty() { Value::Null } else { json!(sha) },
            "checkRuns": runs,
            "totalCount": result.get("total_count"),
        });
        if !more {
            answer["rollup"] = json!(if runs
                .iter()
                .any(|run| js::text(run.get("status")) != "completed")
            {
                "pending"
            } else if runs
                .iter()
                .any(|run| FAILED_CONCLUSIONS.contains(&js::text(run.get("conclusion")).as_str()))
            {
                "failure"
            } else if runs.is_empty() {
                "none"
            } else {
                "success"
            });
        }
        answer["nextPageToken"] = if more {
            json!((at_page + 1).to_string())
        } else {
            Value::Null
        };
        Ok(answer)
    }

    /// The end of a failed check's output: the GitHub Actions job log when the check is an Actions job (its check run id is
    /// the job id), else the check's own summary and text, plus its annotations.
    pub(super) async fn check_run_log(&self, args: &Value) -> Answer {
        static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?m)^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:.]+Z ").expect("a valid pattern")
        });
        let base = Self::repo_route(args)?;
        let id = encode(arg(args, "checkRunId"));
        let limit = number_of(args.get("limit"))
            .unwrap_or(4000.0)
            .clamp(500.0, 20_000.0) as usize;
        let run = self.get(&format!("{base}/check-runs/{id}")).await?;
        let log = match self
            .request(
                Method::GET,
                &format!("{base}/actions/jobs/{id}/logs"),
                None,
                "application/vnd.github.v3.raw",
            )
            .await
        {
            Ok(log) => log,
            Err(_) => [
                at(&run, &["output", "summary"]),
                at(&run, &["output", "text"]),
            ]
            .into_iter()
            .filter(|part| truthy(*part))
            .map(|part| js::text(part))
            .collect::<Vec<_>>()
            .join("\n\n"),
        };
        let annotations = self
            .get(&format!("{base}/check-runs/{id}/annotations?per_page=20"))
            .await
            .unwrap_or(Value::Null);
        let shown: Vec<String> = items(&annotations)
            .iter()
            .take(20)
            .map(|item| {
                format!(
                    "{}:{} {}: {}",
                    js::text(item.get("path")),
                    js::text(item.get("start_line")),
                    js::text(item.get("annotation_level")),
                    js::text(item.get("message"))
                )
            })
            .collect();
        // Timestamps at the start of each Actions log line only take room.
        let text = TIMESTAMP.replace_all(&log, "");
        let tail = if js::length(&text) > limit {
            format!("…{}", js::suffix(&text, limit))
        } else {
            text.into_owned()
        };
        let shown = json!(shown);
        let tail = json!(tail);
        Ok(fields(&[
            ("name", run.get("name")),
            ("conclusion", run.get("conclusion")),
            ("url", run.get("html_url")),
            ("annotations", Some(&shown)),
            ("log", Some(&tail)),
        ]))
    }

    pub(super) async fn parent_head(&self, args: &Value) -> Result<Option<String>, Failure> {
        let Some(parent) =
            number_of(args.get("parentPullNumber")).filter(|parent| parent.fract() == 0.0)
        else {
            return Ok(None);
        };
        let pr = self
            .get(&format!("{}/pulls/{parent}", Self::repo_route(args)?))
            .await?;
        Ok(at(&pr, &["head", "ref"]).map(|head| js::text(Some(head))))
    }

    pub(super) async fn create_pull_request(&self, args: &Value) -> Answer {
        let route = Self::repo_route(args)?;
        let base = match self.parent_head(args).await? {
            Some(head) => head,
            None if !arg(args, "base").is_empty() => arg(args, "base").to_owned(),
            None => js::text(self.get(&route).await?.get("default_branch")),
        };
        let mut body = json!({
            "title": arg(args, "title"),
            "head": arg(args, "head"),
            "base": base,
            "draft": args.get("draft") == Some(&Value::Bool(true)),
        });
        if let Some(text) = args.get("body").filter(|text| text.is_string()) {
            body["body"] = text.clone();
        }
        let pr = self
            .api(Method::POST, &format!("{route}/pulls"), Some(&body))
            .await?;
        Ok(json!({ "pullRequest": pull(&pr) }))
    }

    pub(super) async fn update_pull_request(&self, args: &Value) -> Answer {
        let route = format!(
            "{}/pulls/{}",
            Self::repo_route(args)?,
            Self::pull_number(args)?
        );
        let mut patch = Map::new();
        for key in ["title", "body"] {
            if let Some(text) = args.get(key).filter(|text| text.is_string()) {
                patch.insert(key.to_owned(), text.clone());
            }
        }
        if let Some(state) = args
            .get("state")
            .filter(|state| *state == "open" || *state == "closed")
        {
            patch.insert("state".to_owned(), state.clone());
        }
        let mut pr = if patch.is_empty() {
            self.get(&route).await?
        } else {
            self.api(Method::PATCH, &route, Some(&Value::Object(patch)))
                .await?
        };
        if let Some(draft) = args.get("draft").and_then(Value::as_bool)
            && pr.get("draft").and_then(Value::as_bool) != Some(draft)
        {
            // Draft state is only changeable through GraphQL.
            let mutation = if draft {
                "mutation($id:ID!){convertPullRequestToDraft(input:{pullRequestId:$id}){pullRequest{isDraft}}}"
            } else {
                "mutation($id:ID!){markPullRequestReadyForReview(input:{pullRequestId:$id}){pullRequest{isDraft}}}"
            };
            self.graphql(mutation, json!({ "id": pr.get("node_id") }))
                .await?;
            pr = self.get(&route).await?;
        }
        Ok(json!({ "pullRequest": pull(&pr) }))
    }

    pub(super) async fn update_pull_request_base(&self, args: &Value) -> Answer {
        let base = match self.parent_head(args).await? {
            Some(head) => head,
            None => arg(args, "base").to_owned(),
        };
        if base.is_empty() {
            return Err("Give base or parentPullNumber".to_owned());
        }
        let route = format!(
            "{}/pulls/{}",
            Self::repo_route(args)?,
            Self::pull_number(args)?
        );
        let pr = self
            .api(Method::PATCH, &route, Some(&json!({ "base": base })))
            .await?;
        Ok(json!({ "pullRequest": pull(&pr) }))
    }

    pub(super) async fn merge_pull_request(&self, args: &Value) -> Answer {
        let expected = arg(args, "expectedHeadSha");
        if expected.is_empty() {
            return Err(
                "expectedHeadSha is required, so a branch that moved is never merged by mistake"
                    .to_owned(),
            );
        }
        let route = format!(
            "{}/pulls/{}/merge",
            Self::repo_route(args)?,
            Self::pull_number(args)?
        );
        let method = if arg(args, "mergeMethod") == "squash" {
            "squash"
        } else {
            "merge"
        };
        let result = self
            .api(
                Method::PUT,
                &route,
                Some(&json!({ "sha": expected, "merge_method": method })),
            )
            .await?;
        let merged = json!(result.get("merged") == Some(&Value::Bool(true)));
        Ok(fields(&[
            ("merged", Some(&merged)),
            ("sha", result.get("sha")),
            ("message", result.get("message")),
        ]))
    }

    pub(super) async fn create_review(&self, args: &Value) -> Answer {
        let event = arg(args, "event").to_uppercase();
        let mapped = if event == "APPROVE" || event == "REQUEST_CHANGES" {
            event.as_str()
        } else {
            "COMMENT"
        };
        let mut body = json!({ "event": mapped });
        if let Some(text) = args.get("body").filter(|text| text.is_string()) {
            body["body"] = text.clone();
        }
        let route = format!(
            "{}/pulls/{}/reviews",
            Self::repo_route(args)?,
            Self::pull_number(args)?
        );
        let review = self.api(Method::POST, &route, Some(&body)).await?;
        let id = json!(id_of(review.get("id")));
        let shown = fields(&[
            ("id", Some(&id)),
            ("state", review.get("state")),
            ("body", review.get("body")),
            ("url", review.get("html_url")),
        ]);
        Ok(json!({ "review": shown }))
    }

    pub(super) async fn update_review(&self, args: &Value) -> Answer {
        let route = format!(
            "{}/pulls/{}/reviews/{}",
            Self::repo_route(args)?,
            Self::pull_number(args)?,
            encode(arg(args, "reviewId"))
        );
        let review = self
            .api(
                Method::PUT,
                &route,
                Some(&json!({ "body": arg(args, "body") })),
            )
            .await?;
        let id = json!(id_of(review.get("id")));
        let shown = fields(&[
            ("id", Some(&id)),
            ("state", review.get("state")),
            ("body", review.get("body")),
        ]);
        Ok(json!({ "review": shown }))
    }

    pub(super) async fn dismiss_review(&self, args: &Value) -> Answer {
        let route = format!(
            "{}/pulls/{}/reviews/{}/dismissals",
            Self::repo_route(args)?,
            Self::pull_number(args)?,
            encode(arg(args, "reviewId"))
        );
        let review = self
            .api(
                Method::PUT,
                &route,
                Some(&json!({ "message": arg(args, "message"), "event": "DISMISS" })),
            )
            .await?;
        let id = json!(id_of(review.get("id")));
        let shown = fields(&[("id", Some(&id)), ("state", review.get("state"))]);
        Ok(json!({ "review": shown }))
    }

    pub(super) async fn create_comment(&self, args: &Value) -> Answer {
        let route = format!(
            "{}/issues/{}/comments",
            Self::repo_route(args)?,
            Self::pull_number(args)?
        );
        let comment = self
            .api(
                Method::POST,
                &route,
                Some(&json!({ "body": arg(args, "body") })),
            )
            .await?;
        let id = json!(id_of(comment.get("id")));
        let shown = fields(&[
            ("id", Some(&id)),
            ("body", comment.get("body")),
            ("url", comment.get("html_url")),
        ]);
        Ok(json!({ "comment": shown }))
    }

    pub(super) async fn create_inline_comment(&self, args: &Value) -> Answer {
        let empty = Value::Null;
        let anchor = args
            .get("anchor")
            .filter(|anchor| anchor.is_object())
            .unwrap_or(&empty);
        let route = format!(
            "{}/pulls/{}",
            Self::repo_route(args)?,
            Self::pull_number(args)?
        );
        let pr = self.get(&route).await?;
        let kind = if arg(anchor, "kind").is_empty() {
            "line"
        } else {
            arg(anchor, "kind")
        };
        let side = if arg(anchor, "side").eq_ignore_ascii_case("left") {
            "LEFT"
        } else {
            "RIGHT"
        };
        let start = number_of(anchor.get("startLine")).unwrap_or(1.0);
        let mut body = fields(&[("commit_id", at(&pr, &["head", "sha"]))]);
        body["body"] = json!(arg(args, "body"));
        body["path"] = json!(safe_path(arg(anchor, "path"))?);
        match kind {
            "file" => body["subject_type"] = json!("file"),
            "range" => {
                body["start_line"] = number_value(start);
                body["start_side"] = json!(side);
                body["line"] = number_value(number_of(anchor.get("endLine")).unwrap_or(start));
                body["side"] = json!(side);
            }
            _ => {
                body["line"] = number_value(start);
                body["side"] = json!(side);
            }
        }
        let comment = self
            .api(Method::POST, &format!("{route}/comments"), Some(&body))
            .await?;
        let id = json!(id_of(comment.get("id")));
        let shown = fields(&[
            ("id", Some(&id)),
            ("path", comment.get("path")),
            ("line", comment.get("line")),
            ("side", comment.get("side")),
            ("url", comment.get("html_url")),
        ]);
        Ok(json!({ "inlineComment": shown }))
    }

    /// threadId is the id of the thread's first review comment (as returned by the threads view).
    pub(super) async fn reply_thread(&self, args: &Value) -> Answer {
        let given = arg(args, "threadId");
        let mut comment_id = given.to_owned();
        if given.is_empty() || !given.chars().all(|character| character.is_ascii_digit()) {
            let data = self
                .graphql(
                    "query($id:ID!){node(id:$id){... on PullRequestReviewThread{comments(first:1){nodes{databaseId}}}}}",
                    json!({ "id": given }),
                )
                .await?;
            comment_id = at(&data, &["node", "comments", "nodes"])
                .and_then(|nodes| nodes.get(0))
                .and_then(|node| node.get("databaseId"))
                .map(|id| js::text(Some(id)))
                .unwrap_or_default();
            if comment_id.is_empty() {
                return Err(format!(
                    "Review thread {} not found",
                    js::text(args.get("threadId"))
                ));
            }
        }
        let route = format!(
            "{}/pulls/{}/comments/{comment_id}/replies",
            Self::repo_route(args)?,
            Self::pull_number(args)?
        );
        let reply = self
            .api(
                Method::POST,
                &route,
                Some(&json!({ "body": arg(args, "body") })),
            )
            .await?;
        let id = json!(id_of(reply.get("id")));
        let shown = fields(&[
            ("id", Some(&id)),
            ("body", reply.get("body")),
            ("url", reply.get("html_url")),
        ]);
        Ok(json!({ "reply": shown }))
    }

    pub(super) async fn update_thread(&self, args: &Value) -> Answer {
        let resolved = args.get("resolved") == Some(&Value::Bool(true));
        let mutation = if resolved {
            "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{id isResolved}}}"
        } else {
            "mutation($id:ID!){unresolveReviewThread(input:{threadId:$id}){thread{id isResolved}}}"
        };
        let given = arg(args, "threadId");
        let data = self.graphql(mutation, json!({ "id": given })).await?;
        let thread = data
            .as_object()
            .and_then(|object| object.values().next())
            .and_then(|value| value.get("thread"));
        let id = thread
            .and_then(|thread| thread.get("id"))
            .cloned()
            .unwrap_or_else(|| json!(given));
        let state = thread
            .and_then(|thread| thread.get("isResolved"))
            .cloned()
            .unwrap_or_else(|| json!(resolved));
        Ok(json!({ "threadId": id, "resolved": state }))
    }

    pub(super) async fn update_comment(&self, args: &Value) -> Answer {
        let id = encode(arg(args, "commentId"));
        let body = json!({ "body": arg(args, "body") });
        let base = Self::repo_route(args)?;
        let comment = match self
            .api(
                Method::PATCH,
                &format!("{base}/issues/comments/{id}"),
                Some(&body),
            )
            .await
        {
            Ok(comment) => comment,
            // Not a conversation comment: try it as an inline review comment.
            Err(error) if error.contains("GitHub 404") => {
                self.api(
                    Method::PATCH,
                    &format!("{base}/pulls/comments/{id}"),
                    Some(&body),
                )
                .await?
            }
            Err(error) => return Err(error),
        };
        let id = json!(id_of(comment.get("id")));
        let shown = fields(&[("id", Some(&id)), ("body", comment.get("body"))]);
        Ok(json!({ "comment": shown }))
    }

    pub(super) async fn update_labels(&self, args: &Value) -> Answer {
        let labels: Vec<String> = items(args.get("labels").unwrap_or(&Value::Null))
            .iter()
            .map(|label| js::text(Some(label)))
            .collect();
        let route = format!(
            "{}/issues/{}/labels",
            Self::repo_route(args)?,
            Self::pull_number(args)?
        );
        let result = self
            .api(Method::PUT, &route, Some(&json!({ "labels": labels })))
            .await?;
        let names: Vec<&Value> = items(&result)
            .iter()
            .filter_map(|label| label.get("name"))
            .collect();
        Ok(json!({ "labels": names }))
    }

    pub(super) async fn create_label(&self, args: &Value) -> Answer {
        let mut body = json!({ "name": arg(args, "label") });
        if !arg(args, "color").is_empty() {
            body["color"] = json!(arg(args, "color").trim_start_matches('#'));
        }
        if let Some(text) = args.get("description").filter(|text| text.is_string()) {
            body["description"] = text.clone();
        }
        let label = self
            .api(
                Method::POST,
                &format!("{}/labels", Self::repo_route(args)?),
                Some(&body),
            )
            .await?;
        let shown = fields(&[
            ("name", label.get("name")),
            ("color", label.get("color")),
            ("description", label.get("description")),
        ]);
        Ok(json!({ "label": shown, "created": true }))
    }

    pub(super) async fn reviewers(&self, args: &Value, method: Method) -> Answer {
        let names = |key: &str| -> Vec<String> {
            items(args.get(key).unwrap_or(&Value::Null))
                .iter()
                .map(|name| js::text(Some(name)))
                .collect()
        };
        let route = format!(
            "{}/pulls/{}",
            Self::repo_route(args)?,
            Self::pull_number(args)?
        );
        let removing = method == Method::DELETE;
        let answer = self
            .api(
                method,
                &format!("{route}/requested_reviewers"),
                Some(&json!({ "reviewers": names("users"), "team_reviewers": names("groups") })),
            )
            .await?;
        let after = if removing {
            self.get(&route).await?
        } else {
            answer
        };
        let list = |key: &str, field: &str| -> Value {
            Value::Array(
                items(after.get(key).unwrap_or(&Value::Null))
                    .iter()
                    .filter_map(|item| item.get(field).cloned())
                    .collect(),
            )
        };
        Ok(
            json!({ "requestedReviewers": list("requested_reviewers", "login"), "requestedTeams": list("requested_teams", "slug") }),
        )
    }
}
