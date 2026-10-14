// Copyright Alexandre D. Díaz
//! What workflows do outside the collector: ask an AI, comment on, edit and
//! open PRs/MRs. `workflow` gates every call (the source's `actions`, else
//! dry-run) and records it (`workflow_action`), so none repeats.
use std::path::Path;
use std::time::Duration;

use reqwest::header::{ACCEPT, USER_AGENT};
use reqwest::Method;
use serde_json::{json, Value};

use crate::config;
use crate::gitclient::{git_auth_env, git_run, git_run_env};

/// The forge's write API, with its own token (never the read one): set
/// `OGHCOLLECTOR_WRITE_TOKEN_GH`/`_GL` (or the `gh_write_token`/
/// `gl_write_token` Docker secret).
pub struct Forge {
    gitlab: bool,
    api: String,
    token: Option<String>,
    client: reqwest::Client,
}

impl Forge {
    pub fn new(gitlab: bool, base_url: &str) -> Forge {
        let (api, secret) = match gitlab {
            true if !base_url.is_empty() => (base_url, "gl"),
            true => ("https://gitlab.com/api/v4/", "gl"),
            false => ("https://api.github.com/", "gh"),
        };
        let env = format!("OGHCOLLECTOR_WRITE_TOKEN_{}", secret.to_uppercase());
        Forge::with(
            gitlab,
            api,
            config::read_secret(&format!("{secret}_write_token"), &env),
        )
    }

    pub fn with(gitlab: bool, api: &str, token: Option<String>) -> Forge {
        Forge {
            gitlab,
            api: format!("{}/", api.trim_end_matches('/')),
            token,
            client: crate::gitclient::http_client(),
        }
    }

    /// git's (user, token) for pushes.
    fn git_credentials(&self) -> Result<(&str, &str), String> {
        let token = self.token.as_deref().ok_or("no write token")?;
        Ok((
            if self.gitlab {
                "oauth2"
            } else {
                "x-access-token"
            },
            token,
        ))
    }

    async fn send(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        let (_, token) = self.git_credentials()?;
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.api))
            .header(USER_AGENT, "OGHCollector")
            .bearer_auth(token);
        if !self.gitlab {
            req = req.header(ACCEPT, "application/vnd.github+json");
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        // `without_url`: a URL could carry credentials into the log.
        let res = req.send().await.map_err(|e| e.without_url().to_string())?;
        let status = res.status();
        let value: Value = res.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let why = value["message"].as_str().unwrap_or_default();
            return Err(format!("{} answered {status} {why}", self.api_host()));
        }
        Ok(value)
    }

    fn api_host(&self) -> String {
        reqwest::Url::parse(&self.api)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default()
    }

    fn project(&self, full_path: &str) -> String {
        match self.gitlab {
            true => format!("projects/{}", urlencoding::encode(full_path)),
            false => format!("repos/{full_path}"),
        }
    }

    /// Comments on a PR/MR; its URL.
    pub async fn comment(
        &self,
        full_path: &str,
        number: i64,
        body: &str,
    ) -> Result<String, String> {
        let project = self.project(full_path);
        let (path, url_key) = match self.gitlab {
            true => (format!("{project}/merge_requests/{number}/notes"), "id"),
            false => (format!("{project}/issues/{number}/comments"), "html_url"),
        };
        let res = self
            .send(Method::POST, &path, Some(json!({"body": body})))
            .await?;
        Ok(res[url_key].to_string().trim_matches('"').to_string())
    }

    /// Replaces a PR/MR description.
    pub async fn edit_description(
        &self,
        full_path: &str,
        number: i64,
        body: &str,
    ) -> Result<(), String> {
        let project = self.project(full_path);
        match self.gitlab {
            true => {
                let path = format!("{project}/merge_requests/{number}");
                self.send(Method::PUT, &path, Some(json!({"description": body})))
                    .await?
            }
            false => {
                let path = format!("{project}/pulls/{number}");
                self.send(Method::PATCH, &path, Some(json!({"body": body})))
                    .await?
            }
        };
        Ok(())
    }

    /// The open PR/MR from `branch`, if any (its URL).
    async fn find_open(&self, full_path: &str, branch: &str) -> Result<Option<String>, String> {
        let project = self.project(full_path);
        let branch_q = urlencoding::encode(branch);
        let (path, url_key) = match self.gitlab {
            true => (
                format!("{project}/merge_requests?state=opened&source_branch={branch_q}"),
                "web_url",
            ),
            false => {
                let owner = full_path.split('/').next().unwrap_or_default();
                (
                    format!("{project}/pulls?state=open&head={owner}:{branch_q}"),
                    "html_url",
                )
            }
        };
        let res = self.send(Method::GET, &path, None).await?;
        Ok(res[0][url_key].as_str().map(str::to_string))
    }

    /// Opens a PR/MR from `branch` onto `base`; its URL.
    async fn open(
        &self,
        full_path: &str,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<String, String> {
        let project = self.project(full_path);
        let (path, payload, url_key) = match self.gitlab {
            true => (
                format!("{project}/merge_requests"),
                json!({"source_branch": branch, "target_branch": base, "title": title, "description": body}),
                "web_url",
            ),
            false => (
                format!("{project}/pulls"),
                json!({"head": branch, "base": base, "title": title, "body": body}),
                "html_url",
            ),
        };
        let res = self.send(Method::POST, &path, Some(payload)).await?;
        Ok(res[url_key].as_str().unwrap_or_default().to_string())
    }
}

/// One question to an AI.
pub struct AiRequest<'a> {
    /// `openai` (any OpenAI-compatible API: OpenAI, Ollama, LM Studio, vLLM...)
    /// or `anthropic`.
    pub provider: &'a str,
    pub model: &'a str,
    pub url: Option<&'a str>,
    pub key: Option<String>,
    pub system: Option<String>,
    pub prompt: String,
    pub max_tokens: u32,
    pub timeout: u64,
}

impl AiRequest<'_> {
    pub fn base_url(&self) -> &str {
        let default = match self.provider {
            "anthropic" => "https://api.anthropic.com",
            _ => "https://api.openai.com/v1",
        };
        self.url.unwrap_or(default).trim_end_matches('/')
    }

    /// The host asked (for logs: never the URL itself, it may hold credentials).
    pub fn host(&self) -> String {
        reqwest::Url::parse(self.base_url())
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default()
    }
}

/// Asks; the answer's text.
pub async fn ask(req: &AiRequest<'_>) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(req.timeout))
        .build()
        .map_err(|e| e.to_string())?;
    let base = req.base_url();
    let res = match req.provider {
        "anthropic" => {
            let mut body = json!({
                "model": req.model,
                "max_tokens": req.max_tokens,
                "messages": [{"role": "user", "content": req.prompt}],
            });
            if let Some(system) = &req.system {
                body["system"] = json!(system);
            }
            let mut call = client
                .post(format!("{base}/v1/messages"))
                .header("anthropic-version", "2023-06-01")
                .json(&body);
            if let Some(key) = &req.key {
                call = call.header("x-api-key", key);
            }
            call.send().await
        }
        _ => {
            let mut messages = Vec::new();
            if let Some(system) = &req.system {
                messages.push(json!({"role": "system", "content": system}));
            }
            messages.push(json!({"role": "user", "content": req.prompt}));
            let body =
                json!({"model": req.model, "max_tokens": req.max_tokens, "messages": messages});
            let mut call = client.post(format!("{base}/chat/completions")).json(&body);
            if let Some(key) = &req.key {
                call = call.bearer_auth(key);
            }
            call.send().await
        }
    };
    let res = res.map_err(|e| format!("{}: {}", req.host(), e.without_url()))?;
    let status = res.status();
    let value: Value = res
        .json()
        .await
        .map_err(|e| format!("{}: {}", req.host(), e.without_url()))?;
    if !status.is_success() {
        let why = value["error"]["message"].as_str().unwrap_or_default();
        return Err(format!("{} answered {status} {why}", req.host()));
    }
    match req.provider {
        "anthropic" => {
            if value["stop_reason"] == "refusal" {
                let category = value["stop_details"]["category"]
                    .as_str()
                    .unwrap_or("unknown");
                return Err(format!("the model declined ({category})"));
            }
            Ok(value["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"))
        }
        _ => value["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("{}: no answer in the response", req.host())),
    }
}

/// The unified diff in a text: its first ```diff (or ```patch) block, else
/// the whole text (an AI usually fences it).
pub fn extract_patch(text: &str) -> String {
    for fence in ["```diff", "```patch"] {
        if let Some((_, rest)) = text.split_once(fence) {
            let rest = rest.split_once('\n').map_or("", |(_, r)| r);
            return rest.split("```").next().unwrap_or_default().to_string();
        }
    }
    text.to_string()
}

/// The files a patch changes, if it only edits, adds or deletes regular
/// files under `dir/`: no renames out of it, no mode changes, symlinks,
/// submodules or binaries. `git apply --check` must pass too.
fn check_patch(worktree: &Path, patch: &Path, dir: &str) -> Result<Vec<String>, String> {
    let wt = worktree.to_string_lossy();
    let patch = patch.to_string_lossy();
    let numstat =
        git_run(&wt, &["apply", "--numstat", &patch]).map_err(|e| format!("not a patch: {e}"))?;
    if numstat.trim().is_empty() {
        return Err("empty patch".to_string());
    }
    let prefix = format!("{}/", dir.trim_end_matches('/'));
    let mut files = Vec::new();
    for line in numstat.lines() {
        let mut cols = line.splitn(3, '\t');
        let (added, path) = (
            cols.next().unwrap_or_default(),
            cols.nth(1).unwrap_or_default(),
        );
        if added == "-" {
            return Err(format!("binary change in '{path}'"));
        }
        if path.contains(" => ") || !path.starts_with(&prefix) {
            return Err(format!("'{path}' is outside the module ({prefix})"));
        }
        files.push(path.to_string());
    }
    let summary = git_run(&wt, &["apply", "--summary", &patch]).map_err(|e| e.to_string())?;
    if let Some(bad) = summary.lines().find(|l| {
        ["mode change", "rename", "copy", "120000", "160000"]
            .iter()
            .any(|w| l.contains(w))
    }) {
        return Err(format!("unsupported change: {}", bad.trim()));
    }
    git_run(&wt, &["apply", "--check", &patch]).map_err(|e| format!("doesn't apply: {e}"))?;
    Ok(files)
}

/// A PR/MR to open with one commit: a patch on a module.
pub struct NewPr<'a> {
    pub clone: &'a str,
    pub repo_url: &'a str,
    pub full_path: &'a str,
    /// Version branch it targets.
    pub base: &'a str,
    /// Module folder, repo-relative: the patch may only touch it.
    pub dir: &'a str,
    /// Always `ogh/...` (see `workflow`), never an existing branch.
    pub branch: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub patch: &'a str,
}

/// Commit author of `pr_open` (`OGHCOLLECTOR_COMMIT_AUTHOR`, "Name <email>").
fn commit_author() -> (String, String) {
    let raw = std::env::var("OGHCOLLECTOR_COMMIT_AUTHOR").unwrap_or_default();
    match raw.split_once('<') {
        Some((name, email)) if !name.trim().is_empty() => (
            name.trim().to_string(),
            email.trim_end_matches('>').trim().to_string(),
        ),
        _ => (
            "OGHCollector".to_string(),
            "oghcollector@localhost".to_string(),
        ),
    }
}

/// Opens the PR (`live`), or checks the patch and says what it would do. The
/// patch is applied in a worktree of its own under `workdir`, never in the
/// shared checkout; the branch is pushed only if it doesn't exist upstream
/// (no force): one that does is an earlier attempt, whose PR is only created
/// if missing (the push worked, the API call didn't).
pub async fn open_pr(
    forge: &Forge,
    workdir: &Path,
    pr: &NewPr<'_>,
    live: bool,
) -> Result<String, String> {
    let _ = git_run(pr.clone, &["worktree", "prune"]);
    std::fs::create_dir_all(workdir).map_err(|e| e.to_string())?;
    let name = pr.branch.replace('/', "-");
    let worktree = workdir.join(&name);
    let patch_file = workdir.join(format!("{name}.patch"));
    let _ = std::fs::remove_dir_all(&worktree);
    // Templates drop a final newline, and git calls a patch without one corrupt.
    let patch = format!("{}\n", pr.patch.trim_end_matches('\n'));
    std::fs::write(&patch_file, patch).map_err(|e| e.to_string())?;
    let wt = worktree.to_string_lossy().to_string();
    let base = format!("origin/{}", pr.base);
    git_run(pr.clone, &["worktree", "add", "--detach", &wt, &base])?;
    let result = async {
        let files = check_patch(&worktree, &patch_file, pr.dir)?;
        if !live {
            return Ok(format!(
                "would open '{}' from {} changing {}",
                pr.title,
                pr.branch,
                files.join(", ")
            ));
        }
        let (user, token) = forge.git_credentials()?;
        let auth = git_auth_env(pr.repo_url, user, token);
        let remote = git_run_env(
            pr.clone,
            &["ls-remote", "--heads", "origin", pr.branch],
            &auth,
        )?;
        if remote.trim().is_empty() {
            let (name, email) = commit_author();
            git_run(&wt, &["apply", &patch_file.to_string_lossy()])?;
            git_run(&wt, &["add", "-A"])?;
            git_run(
                &wt,
                &[
                    "-c",
                    &format!("user.name={name}"),
                    "-c",
                    &format!("user.email={email}"),
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-q",
                    "-m",
                    pr.title,
                ],
            )?;
            let refspec = format!("HEAD:refs/heads/{}", pr.branch);
            git_run_env(&wt, &["push", "origin", &refspec], &auth)?;
        } else if let Some(url) = forge.find_open(pr.full_path, pr.branch).await? {
            return Ok(url);
        }
        forge
            .open(pr.full_path, pr.branch, pr.base, pr.title, pr.body)
            .await
    }
    .await;
    let _ = git_run(pr.clone, &["worktree", "remove", "--force", &wt]);
    let _ = std::fs::remove_file(&patch_file);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_come_out_of_fences() {
        let answer = "Here it is:\n```diff\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n```\nDone.";
        assert_eq!(
            extract_patch(answer),
            "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n"
        );
        assert_eq!(extract_patch("--- a/x\n"), "--- a/x\n");
    }
}
