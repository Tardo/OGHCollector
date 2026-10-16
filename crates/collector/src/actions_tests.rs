// Copyright Alexandre D. Díaz
//! Workflows' outward nodes end to end, against a local mock of the AI and
//! forge APIs: dry-run only logs, live acts once (the ledger), `pr_open`
//! pushes an `ogh/` branch with a patch confined to the module.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use duct::cmd;
use serde_json::{json, Value};
use sqlitedb::models;
use sqlitedb::DbSqliteConnection as SqliteConnection;

use crate::actions::Forge;
use crate::analyzer::OGHCollectorAnalyzer;
use crate::anygitclient::AnyGitClient;
use crate::clients::gitlab::GitlabClient;
use crate::gitclient::GitClient;
use crate::golden::{snapshot, Fixture};
use crate::pull_requests::{self, Budget, RepoPrs};
use crate::pypi::PypiClient;
use crate::review;
use crate::workflow::{self, Deps, Engine, Workflow};

type Calls = Arc<Mutex<Vec<(String, String, Value)>>>;

/// A local HTTP server answering `respond(method, path)`, recording every
/// request (method, path, JSON body). Its base URL ends with `/`. An answer
/// with `"__status": N` goes out with that status.
async fn serve(respond: impl Fn(&str, &str) -> Value + Send + Sync + 'static) -> (String, Calls) {
    let calls: Calls = Arc::default();
    let (recorded, respond) = (calls.clone(), Arc::new(respond));
    let app = axum::Router::new().fallback(
        move |method: axum::http::Method, uri: axum::http::Uri, body: axum::body::Bytes| {
            let (calls, respond) = (recorded.clone(), respond.clone());
            async move {
                let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let path = uri.path_and_query().map_or(uri.path(), |p| p.as_str());
                calls
                    .lock()
                    .unwrap()
                    .push((method.to_string(), path.to_string(), body));
                let value = respond(method.as_str(), uri.path());
                let status = value["__status"]
                    .as_u64()
                    .and_then(|s| axum::http::StatusCode::from_u16(s as u16).ok())
                    .unwrap_or(axum::http::StatusCode::OK);
                (status, axum::Json(value))
            }
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, calls)
}

fn parse(yaml: &str) -> Workflow {
    config::Config::builder()
        .add_source(config::File::from_str(yaml, config::FileFormat::Yaml))
        .build()
        .and_then(|c| c.try_deserialize())
        .unwrap()
}

/// Events logged so far: (type, severity, message).
fn events(conn: &mut SqliteConnection, event_type: &str) -> Vec<String> {
    let prefix = format!("{event_type} ");
    let section = snapshot(conn);
    let events = section.split("## event\n").nth(1).unwrap_or_default();
    events
        .lines()
        .filter_map(|l| l.strip_prefix(&prefix))
        .map(str::to_string)
        .collect()
}

/// One job's module walk with `workflow` and the source's `actions`.
async fn walk(
    f: &Fixture,
    conn: &mut SqliteConnection,
    wf: &Workflow,
    actions: &[&str],
    forge: &Forge,
) {
    let defs = workflow::with_built_in(&HashMap::from([("w".to_string(), wf.clone())]));
    let engine = Engine::new(&["w".to_string()], &defs).unwrap();
    let (analyzer, pypi) = (OGHCollectorAnalyzer::new(&180), PypiClient::new());
    let actions: Vec<String> = actions.iter().map(|a| a.to_string()).collect();
    let workdir = f.dir.join("work");
    let deps = Deps::for_tests(&analyzer, &pypi, forge, &workdir, &actions);
    let org = models::gh_organization::add(conn, "org").unwrap();
    let repo = models::gh_repository::add(conn, &org.id, "repo").unwrap();
    let repos = f.repo_infos();
    workflow::run_modules(
        conn,
        &engine,
        &deps,
        &[String::new()],
        &repos,
        &[repo.id],
        &180,
    )
    .await;
}

#[tokio::test]
async fn ai_answers_are_asked_once_and_dry_run_only_logs() {
    let (base, calls) = serve(|_, path| match path {
        "/v1/messages" => {
            json!({"content": [{"type": "text", "text": "from claude"}], "stop_reason": "end_turn"})
        }
        _ => json!({"choices": [{"message": {"content": "LGTM"}}]}),
    })
    .await;
    let f = Fixture::new("ai");
    f.write(
        "m/__manifest__.py",
        "{'name': 'M', 'version': '18.0.1.0.0'}",
    );
    f.commit("one");
    let pool = f.db();
    let conn = &mut pool.get().unwrap();
    let wf = parse(
        &"on: module
nodes:
  - {id: review, type: ai, provider: openai, model: local, url: BASE, next: claude,
     prompt: \"Review {{ technical_name | tojson }}: {{ file('__manifest__.py') }}\"}
  - {id: claude, type: ai, provider: anthropic, model: claude-opus-5-5, url: BASE, prompt: hi, next: out}
  - {id: out, type: log, event: probe, message: '{{ ai.review }}|{{ ai.claude }}'}
"
        .replace("BASE", base.trim_end_matches('/')),
    );
    let forge = Forge::with(false, &base, None);

    // Live, twice: one call per provider, the second run reuses the answers.
    walk(&f, conn, &wf, &["ai"], &forge).await;
    walk(&f, conn, &wf, &["ai"], &forge).await;
    assert_eq!(
        events(conn, "probe"),
        ["info LGTM|from claude", "info LGTM|from claude"]
    );
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, "/chat/completions");
        let prompt = calls[0].2["messages"][0]["content"].as_str().unwrap();
        assert!(
            prompt.contains("'name': 'M'"),
            "file() read the module: {prompt}"
        );
        assert_eq!(
            (calls[1].1.as_str(), &calls[1].2["model"]),
            ("/v1/messages", &json!("claude-opus-5-5"))
        );
    }

    // Dry-run: no call, empty answers, each node logged once.
    walk(&f, conn, &wf, &[], &forge).await;
    walk(&f, conn, &wf, &[], &forge).await;
    assert_eq!(calls.lock().unwrap().len(), 2);
    let dry: Vec<String> = events(conn, "workflow")
        .into_iter()
        .filter(|e| e.starts_with("info [dry-run]"))
        .collect();
    assert_eq!(dry.len(), 2, "{dry:?}");
    assert!(
        dry[0].contains("ask local at 127.0.0.1 about 'm' in 'repo' [18.0]"),
        "{dry:?}"
    );
    assert_eq!(events(conn, "probe")[2..], ["info |", "info |"]);
}

#[tokio::test]
async fn apprise_notifies_once_and_retries_only_what_sent_nothing() {
    let (base, calls) = serve(|_, path| match path {
        "/notify/partial" => json!({"__status": 424}),
        "/notify/down" => json!({"__status": 500}),
        _ => json!({}),
    })
    .await;
    std::env::set_var("OGH_TEST_APPRISE_URLS", "json://hook.example/x");
    let f = Fixture::new("apprise");
    f.write(
        "m/__manifest__.py",
        "{'name': 'M', 'version': '18.0.1.0.0'}",
    );
    f.commit("one");
    let pool = f.db();
    let conn = &mut pool.get().unwrap();
    let wf = parse(
        &"on: module
nodes:
  - {id: ok, type: apprise, url: BASE/notify/, api_key_env: OGH_TEST_APPRISE_URLS,
     severity: error, title: '{{ technical_name }}', body: 'new in {{ repo }}', next: partial}
  - {id: partial, type: apprise, url: BASE/notify/partial, body: p, next: down}
  - {id: down, type: apprise, url: BASE/notify/down, body: d}
"
        .replace("BASE", base.trim_end_matches('/')),
    );
    let forge = Forge::with(false, &base, None);

    // Dry-run: nothing sent, what it would send logged.
    walk(&f, conn, &wf, &[], &forge).await;
    assert!(calls.lock().unwrap().is_empty());
    let dry = events(conn, "workflow");
    assert!(
        dry[0].contains(
            "[dry-run] Workflow 'w' (ok): notify 127.0.0.1 about 'm' in 'repo' [18.0]: new in repo"
        ),
        "{dry:?}"
    );

    // Live, twice: 424 (something went out) is recorded like a success, the
    // 500 (nothing did) is retried.
    walk(&f, conn, &wf, &["apprise"], &forge).await;
    walk(&f, conn, &wf, &["apprise"], &forge).await;
    let calls = calls.lock().unwrap();
    let paths: Vec<&str> = calls.iter().map(|c| c.1.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/notify/",
            "/notify/partial",
            "/notify/down",
            "/notify/down"
        ]
    );
    assert_eq!(
        calls[0].2,
        json!({"title": "m", "body": "new in repo", "type": "failure", "urls": "json://hook.example/x"})
    );
    assert_eq!(calls[1].2["type"], "info");
    assert!(calls[1].2.get("urls").is_none());
}

/// One job's PR step over a repo.
struct PrJob<'a> {
    engine: &'a Engine,
    client: &'a AnyGitClient,
    repo: &'a crate::gitclient::RepoInfo,
    gh_repo_id: i64,
    forge: &'a Forge,
    workdir: &'a std::path::Path,
}

impl PrJob<'_> {
    async fn run(&self, conn: &mut SqliteConnection, actions: &[String]) {
        let (analyzer, pypi) = (OGHCollectorAnalyzer::new(&180), PypiClient::new());
        let deps = Deps::for_tests(&analyzer, &pypi, self.forge, self.workdir, actions);
        let c = RepoPrs {
            git: self.client,
            repo: self.repo,
            gh_repo_id: self.gh_repo_id,
            branch: "18.0",
            version_odoo: 180,
            read_paths: &[String::new()],
            analyzer: &analyzer,
            workdir: self.workdir,
            full: false,
            engine: self.engine,
            deps: &deps,
        };
        let mut budget = Budget {
            analyses: 10,
            ci_lookups: 10,
        };
        pull_requests::collect(&c, conn, &mut budget).await;
    }
}

/// The PR step of a `migration` source, twice live after a dry-run, with
/// comment/edit nodes; the forge's write calls.
async fn pr_bot(gitlab: bool) -> (Vec<(String, String, Value)>, Vec<String>) {
    let (base, calls) = serve(|method, path| match (method, path) {
        ("GET", p) if p.ends_with("/merge_requests") => json!([{
            "iid": 5, "title": "[18.0][MIG] m", "sha": "abc123", "source_branch": "18.0-mig-m",
            "author": {"username": "ana"}, "draft": false, "labels": ["approved"],
            "description": "Port of m.", "detailed_merge_status": "mergeable",
            "created_at": "2026-10-01T00:00:00Z", "updated_at": "2026-10-02T00:00:00Z",
        }]),
        ("POST", _) => json!({"id": 99, "html_url": "https://github.com/org/repo/pull/5#c1"}),
        _ => json!({}),
    })
    .await;
    let f = Fixture::new(if gitlab { "prbot-gl" } else { "prbot-gh" });
    let pool = f.db();
    let conn = &mut pool.get().unwrap();
    let org = models::gh_organization::add(conn, "org").unwrap();
    let gh_repo = models::gh_repository::add(conn, &org.id, "repo").unwrap();
    let wf = parse(
        "on: pull_request
nodes:
  - {id: theirs, type: if, when: 'not pr.is_ours', then: thanks}
  - {id: thanks, type: pr_comment, body: 'Thanks @{{ pr.author }}! {{ modules[0].name }} [{{ odoo_version }}]', next: tag}
  - {id: tag, type: pr_edit, body: \"{{ pr.description }}\\n\\nReviewed by OGHCollector.\"}
",
    );
    let defs = workflow::with_built_in(&HashMap::from([("bot".to_string(), wf)]));
    let engine = Engine::new(&["bot".to_string()], &defs).unwrap();
    // The listing always comes from the GitLab mock; the forge writes as asked.
    let client = AnyGitClient::Gitlab(GitlabClient::new("read", &base));
    let forge = Forge::with(gitlab, &base, Some("write".to_string()));
    let repo = f.repo_infos().remove(0);
    let workdir = f.dir.join("work");
    let job = PrJob {
        engine: &engine,
        client: &client,
        repo: &repo,
        gh_repo_id: gh_repo.id,
        forge: &forge,
        workdir: &workdir,
    };
    job.run(conn, &[]).await; // dry-run
    let dry = events(conn, "workflow");
    assert_eq!(dry.len(), 2, "{dry:?}");
    assert!(
        dry[0].contains("comment on PR #5") && dry[0].contains("Thanks @ana! m [18.0]"),
        "{dry:?}"
    );
    assert!(calls.lock().unwrap().iter().all(|c| c.0 == "GET"));
    let live = ["pr_comment".to_string(), "pr_edit".to_string()];
    job.run(conn, &live).await;
    job.run(conn, &live).await; // nothing new: the ledger
    let writes = calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.0 != "GET")
        .cloned()
        .collect();
    (writes, events(conn, "workflow"))
}

#[tokio::test]
async fn pull_request_workflows_comment_and_edit_once_on_gitlab() {
    let (writes, _) = pr_bot(true).await;
    assert_eq!(writes.len(), 2, "{writes:?}");
    assert_eq!(
        (writes[0].0.as_str(), writes[0].1.as_str()),
        ("POST", "/projects/org%2Frepo/merge_requests/5/notes")
    );
    assert_eq!(writes[0].2["body"], "Thanks @ana! m [18.0]");
    assert_eq!(
        (writes[1].0.as_str(), writes[1].1.as_str()),
        ("PUT", "/projects/org%2Frepo/merge_requests/5")
    );
    assert_eq!(
        writes[1].2["description"],
        "Port of m.\n\nReviewed by OGHCollector."
    );
}

#[tokio::test]
async fn pull_request_workflows_comment_and_edit_once_on_github() {
    let (writes, events) = pr_bot(false).await;
    assert_eq!(writes.len(), 2, "{writes:?}");
    assert_eq!(
        (writes[0].0.as_str(), writes[0].1.as_str()),
        ("POST", "/repos/org/repo/issues/5/comments")
    );
    assert_eq!(writes[0].2["body"], "Thanks @ana! m [18.0]");
    assert_eq!(
        (writes[1].0.as_str(), writes[1].1.as_str()),
        ("PATCH", "/repos/org/repo/pulls/5")
    );
    assert_eq!(
        writes[1].2["body"],
        "Port of m.\n\nReviewed by OGHCollector."
    );
    assert!(
        events.iter().any(|e| e.ends_with("pull/5#c1")),
        "{events:?}"
    );
}

/// An origin with modules `m` and `other`, the collector's clone of it, and
/// two patches (one on `m`, one on `other`).
fn open_fixture(name: &str) -> (Fixture, std::path::PathBuf, String, String) {
    let f = Fixture::new(name);
    f.write(
        "m/__manifest__.py",
        "{'name': 'M', 'version': '18.0.1.0.0'}\n",
    );
    f.write(
        "other/__manifest__.py",
        "{'name': 'O', 'version': '18.0.1.0.0'}\n",
    );
    f.commit("one");
    git(&f.dir, &["clone", "-q", "--bare", "repo", "origin.git"]);
    let patch_for = |file: &str| {
        f.write(file, "{'name': 'X', 'version': '18.0.1.1.0'}\n");
        let patch = git(&f.repo, &["diff"]);
        git(&f.repo, &["checkout", "-q", "--", "."]);
        patch
    };
    let (good, outside) = (
        patch_for("m/__manifest__.py"),
        patch_for("other/__manifest__.py"),
    );
    std::fs::remove_dir_all(&f.repo).unwrap();
    git(
        &f.dir,
        &["clone", "-q", "--branch", "18.0", "origin.git", "repo"],
    );
    let origin = f.dir.join("origin.git");
    (f, origin, good, outside)
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    cmd("git", args)
        .dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stderr_null()
        .read()
        .unwrap()
}

/// A module workflow opening a PR with `patch` on module `m`.
fn opener(patch: &str) -> Workflow {
    let mut wf = parse(
        "on: module
nodes:
  - {id: only_m, type: if, when: \"technical_name == 'm'\", then: fix}
  - {id: fix, type: pr_open, title: 'Fix {{ technical_name }}', body: Automated., patch: PATCH}
",
    );
    wf.nodes[1].patch = Some(patch.to_string());
    wf
}

#[tokio::test]
async fn pr_open_pushes_a_confined_patch_once() {
    let (base, calls) = serve(|method, _| match method {
        "GET" => json!([]),
        _ => json!({"web_url": "https://git.example/org/repo/-/merge_requests/1"}),
    })
    .await;
    let (f, origin, good, outside) = open_fixture("open");
    let pool = f.db();
    let conn = &mut pool.get().unwrap();
    let forge = Forge::with(true, &base, Some("write".to_string()));
    let branches = || git(&origin, &["branch", "--list", "ogh/*"]);

    // Touching another module: refused, nothing recorded or pushed.
    walk(&f, conn, &opener(&outside), &["pr_open"], &forge).await;
    assert!(events(conn, "workflow").is_empty());

    // Dry-run: checks the patch, says what it'd do, pushes nothing.
    walk(&f, conn, &opener(&good), &[], &forge).await;
    let dry = events(conn, "workflow");
    assert!(
        dry[0].contains("would open 'Fix m' from ogh/w/m-18.0 changing m/__manifest__.py"),
        "{dry:?}"
    );
    assert_eq!(branches(), "");

    // Live, twice: one branch, one MR.
    walk(&f, conn, &opener(&good), &["pr_open"], &forge).await;
    walk(&f, conn, &opener(&good), &["pr_open"], &forge).await;
    assert_eq!(branches().trim(), "ogh/w/m-18.0");
    let pushed = git(&origin, &["show", "ogh/w/m-18.0:m/__manifest__.py"]);
    assert!(pushed.contains("18.0.1.1.0"));
    let creates: Vec<Value> = calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.0 == "POST")
        .map(|c| c.2.clone())
        .collect();
    assert_eq!(creates.len(), 1);
    assert_eq!(creates[0]["source_branch"], "ogh/w/m-18.0");
    assert_eq!(creates[0]["target_branch"], "18.0");
    let live = events(conn, "workflow");
    assert!(
        live.last().unwrap().ends_with("merge_requests/1"),
        "{live:?}"
    );
}

#[tokio::test]
async fn pr_open_finishes_an_earlier_attempt_on_github() {
    let (base, calls) = serve(|method, _| match method {
        "GET" => json!([]),
        _ => json!({"html_url": "https://github.com/org/repo/pull/7"}),
    })
    .await;
    let (f, origin, good, _) = open_fixture("open-gh");
    // A run pushed the branch and died before opening the PR.
    git(
        &f.repo,
        &["push", "-q", "origin", "HEAD:refs/heads/ogh/w/m-18.0"],
    );
    let before = git(&origin, &["rev-parse", "ogh/w/m-18.0"]);
    let pool = f.db();
    let conn = &mut pool.get().unwrap();
    let forge = Forge::with(false, &base, Some("write".to_string()));
    walk(&f, conn, &opener(&good), &["pr_open"], &forge).await;
    // Not pushed over: only the missing PR, after looking for an open one.
    assert_eq!(git(&origin, &["rev-parse", "ogh/w/m-18.0"]), before);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0].0, "GET");
    assert!(
        calls[0]
            .1
            .starts_with("/repos/org/repo/pulls?state=open&head=org:ogh%2Fw%2Fm-18.0"),
        "{calls:?}"
    );
    assert_eq!(
        (calls[1].0.as_str(), calls[1].1.as_str()),
        ("POST", "/repos/org/repo/pulls")
    );
    assert_eq!(
        (&calls[1].2["head"], &calls[1].2["base"]),
        (&json!("ogh/w/m-18.0"), &json!("18.0"))
    );
}

/// A plan with a two-line suggestion (its last line the PR's own), a
/// one-line one on a line the PR leaves alone, and a change outside the diff.
fn review_plan() -> review::Plan {
    let suggestion = |start, end, old_end, text: &str| review::Suggestion {
        path: "m/x.py".to_string(),
        start,
        end,
        lines: vec!["fixed".to_string()],
        old_end,
        text: text.to_string(),
    };
    review::Plan {
        suggestions: vec![
            suggestion(4, 5, None, "@@ first\n"),
            suggestion(7, 7, Some(6), "@@ second\n"),
        ],
        leftovers: vec!["@@ outside\n".to_string()],
    }
}

#[tokio::test]
async fn reviews_suggest_on_github_and_fall_back_to_text() {
    let posts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = posts.clone();
    let (base, calls) = serve(move |_, _| {
        // The first review is refused (a line outside the diff), the second
        // (text only) goes through.
        match seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
            0 => json!({"__status": 422, "message": "Unprocessable Entity"}),
            _ => json!({"html_url": "https://github.com/org/repo/pull/5#review-1"}),
        }
    })
    .await;
    let forge = Forge::with(false, &base, Some("write".to_string()));
    let plan = review_plan();
    let outcome = forge
        .review("org/repo", 5, "abc123", "Two fixes.", &plan)
        .await
        .unwrap();
    assert!(outcome.ends_with("#review-1 (its lines refused: suggestions sent as text)"));
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0].1, "/repos/org/repo/pulls/5/reviews");
    let first = &calls[0].2;
    assert_eq!(
        (&first["event"], &first["commit_id"]),
        (&json!("COMMENT"), &json!("abc123"))
    );
    assert_eq!(
        first["comments"][0],
        json!({"path": "m/x.py", "start_line": 4, "start_side": "RIGHT", "line": 5, "side": "RIGHT",
               "body": "```suggestion\nfixed\n```"})
    );
    assert_eq!(first["comments"][1]["line"], 7);
    assert!(first["comments"][1].get("start_line").is_none());
    assert!(first["body"].as_str().unwrap().contains("@@ outside"));
    // Then all of it as text, no line comments.
    let second = &calls[1].2;
    assert_eq!(second["comments"], json!([]));
    let text = second["body"].as_str().unwrap();
    assert!(text.starts_with("Two fixes."), "{text}");
    assert!(text.contains("@@ outside\n@@ first\n@@ second\n"), "{text}");
}

#[tokio::test]
async fn reviews_suggest_on_gitlab_one_thread_each() {
    let threads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = threads.clone();
    let (base, calls) = serve(move |method, path| match (method, path) {
        ("GET", _) => {
            json!({"diff_refs": {"base_sha": "b", "start_sha": "s", "head_sha": "abc123"}})
        }
        // The second thread is refused: it goes in the note.
        ("POST", p) if p.ends_with("/discussions") => {
            match seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                1 => json!({"__status": 400, "message": "line_code can't be blank"}),
                _ => json!({"id": "t"}),
            }
        }
        _ => json!({"id": 9}),
    })
    .await;
    let forge = Forge::with(true, &base, Some("write".to_string()));
    let plan = review_plan();
    let outcome = forge
        .review("org/repo", 5, "abc123", "Two fixes.", &plan)
        .await
        .unwrap();
    assert_eq!(outcome, "1 suggestion(s) on !5, 1 sent as text");
    let calls = calls.lock().unwrap();
    let paths: Vec<&str> = calls.iter().map(|c| c.1.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/projects/org%2Frepo/merge_requests/5",
            "/projects/org%2Frepo/merge_requests/5/discussions",
            "/projects/org%2Frepo/merge_requests/5/discussions",
            "/projects/org%2Frepo/merge_requests/5/notes",
        ]
    );
    let first = &calls[1].2;
    assert_eq!(first["body"], "```suggestion:-1+0\nfixed\n```");
    assert_eq!(
        first["position"],
        json!({"position_type": "text", "base_sha": "b", "start_sha": "s", "head_sha": "abc123",
               "old_path": "m/x.py", "new_path": "m/x.py", "new_line": 5})
    );
    // A line the PR leaves alone: anchored by its old number too.
    assert_eq!(calls[2].2["position"]["old_line"], 6);
    let note = calls[3].2["body"].as_str().unwrap();
    assert!(note.contains("@@ outside\n@@ second\n"), "{note}");
    assert!(!note.contains("@@ first"), "{note}");
}

#[tokio::test]
async fn reviews_wait_for_a_gitlab_mr_that_moved_on() {
    let (base, calls) = serve(|_, _| json!({"diff_refs": {"head_sha": "newer"}})).await;
    let forge = Forge::with(true, &base, Some("write".to_string()));
    let err = forge
        .review("org/repo", 5, "abc123", "Fix.", &review_plan())
        .await
        .unwrap_err();
    assert!(err.contains("moved on"), "{err}");
    // Nothing written: the next run reviews its new head.
    assert!(calls.lock().unwrap().iter().all(|c| c.0 == "GET"));
}

#[test]
fn review_plans_anchor_on_the_pr_head() {
    let f = Fixture::new("review-plan");
    f.write("m/x.py", "a\nb\nc\nd\ne\nf\ng\n");
    f.commit("base");
    f.git(&["checkout", "-q", "-b", "pr"]);
    f.write("m/x.py", "a\nb\nc\nD\ne\nf\ng\n");
    f.git(&["add", "-A"]);
    f.git(&["commit", "-q", "-m", "pr"]);
    let head = git(&f.repo, &["rev-parse", "HEAD"]);
    let clone = f.repo.to_string_lossy();
    // An AI's diff: bad numbers, against what the PR wrote (D → DD).
    let patch = "--- a/m/x.py\n+++ b/m/x.py\n@@ -40,3 +40,3 @@\n c\n-D\n+DD\n e\n";
    let plan = review::plan_on(&clone, "18.0", &head, patch).unwrap();
    assert!(plan.leftovers.is_empty(), "{plan:?}");
    let s = &plan.suggestions[0];
    assert_eq!(
        (s.path.as_str(), s.start, s.end, s.lines.clone()),
        ("m/x.py", 4, 4, vec!["DD".to_string()])
    );
    let missing = review::plan_on(&clone, "18.0", &"0".repeat(40), patch);
    assert!(missing.unwrap_err().contains("isn't in the clone"));
}
