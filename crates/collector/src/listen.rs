// Copyright Alexandre D. Díaz
//! `oghcollector --listen`: the long-running collector. One worker runs the
//! jobs of a queue one at a time (each still its own child process, see
//! `run_job`): the whole `collector.yaml` every `OGHCOLLECTOR_SCHEDULE_HOURS`,
//! and the pushed repository/branch as soon as a GitHub/GitLab push webhook
//! arrives, ahead of the scheduled jobs still pending.
use std::collections::VecDeque;
use std::env;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::models;

use crate::config;

#[derive(Default)]
struct Queue {
    jobs: Mutex<VecDeque<Vec<String>>>,
    ready: Condvar,
}

impl Queue {
    /// Webhook jobs (`urgent`) go first, so they wait for the running job at
    /// most, not for a whole scheduled batch. Duplicates are only checked
    /// against pending jobs: a running one may have fetched before the push.
    fn push(&self, job: Vec<String>, urgent: bool) {
        let mut jobs = self.jobs.lock().unwrap();
        if urgent {
            jobs.retain(|j| *j != job);
            jobs.push_front(job);
        } else if !jobs.contains(&job) {
            jobs.push_back(job);
        }
        self.ready.notify_one();
    }
}

/// Runs the queued jobs one by one, so jobs never fight over an org lock or a
/// checkout, and the embeddings pass once the queue drains. That pass is a
/// child process too: the model stays loaded for the life of the process that
/// loads it (~800MB), which must not be this one.
fn work(queue: &Queue, pool: &sqlitedb::Pool) {
    let exe = env::current_exe().expect("Can't locate the collector binary");
    let mut collected = false;
    loop {
        let job = {
            let jobs = queue.jobs.lock().unwrap();
            let mut jobs = queue
                .ready
                .wait_while(jobs, |jobs| jobs.is_empty() && !collected)
                .unwrap();
            jobs.pop_front()
        };
        if let Some(args) = job {
            super::run_and_record(&exe, &args, pool);
            collected = true;
            continue;
        }
        collected = false;
        let args = ["--embeddings".to_string()];
        if let Err(reason) = super::run_job(&exe, &args, super::JOB_TIMEOUT) {
            log::error!("Embeddings pass failed: {reason}");
        }
    }
}

/// Queues every job of the config at each multiple of `period` since the
/// epoch (6h: 00:00, 06:00, 12:00 and 18:00 UTC), never at startup, or every
/// redeploy would start a batch. The file is re-read each time.
fn schedule(queue: &Queue, path: &str, period: u64) {
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        thread::sleep(Duration::from_secs(period - now % period));
        match config::load_jobs(path) {
            Ok(jobs) => {
                log::info!("Scheduled run: queuing {} jobs", jobs.len());
                jobs.into_iter().for_each(|job| queue.push(job, false));
            }
            Err(err) => log::error!("Scheduled run skipped: {err}"),
        }
    }
}

/// The jobs (`load_jobs` argument lists) a push to `repo`'s `branch` concerns,
/// narrowed to that repository: those of a source that is the repository
/// itself or its org/group (only direct children: a group source doesn't
/// include its subgroups), with `git` (`GH`/`GL`) and `branch` as version.
// ponytail: GitLab instances aren't told apart, a push to `grp/x` on one also
// matches a `grp` source of another (that job then fails); compare the
// base_url host with the payload's `project.web_url` if both are ever used.
fn jobs_for_push(jobs: Vec<Vec<String>>, git: &str, repo: &str, branch: &str) -> Vec<Vec<String>> {
    let parent = repo.rsplit_once('/').map(|(parent, _)| parent);
    jobs.into_iter()
        .filter(|job| {
            let source = job[0].split(':').next().unwrap_or_default();
            job[1] == branch
                && job[2].split(':').next() == Some(git)
                && (source.eq_ignore_ascii_case(repo)
                    || parent.is_some_and(|p| p.eq_ignore_ascii_case(source)))
        })
        .map(|mut job| {
            job[0] = match job[0].split_once(':') {
                Some((_, paths)) => format!("{repo}:{paths}"),
                None => repo.to_string(),
            };
            job
        })
        .collect()
}

/// The payload's repository path becomes a `data/repos/...` directory and a
/// CLI argument split on ':' and ','.
fn is_valid_repo_path(repo: &str) -> bool {
    repo.split('/').count() >= 2
        && repo.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        })
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

fn github_signature_ok(secret: &str, signature: &str, body: &[u8]) -> bool {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(body);
    let expected: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let signature = signature.strip_prefix("sha256=").unwrap_or_default();
    expected.as_bytes().ct_eq(signature.as_bytes()).into()
}

type Reply = (StatusCode, String);

/// Ignored events still get a 2xx: GitHub/GitLab flag (GitLab eventually
/// disables) a hook whose deliveries fail.
fn ignored(why: &str) -> Reply {
    log::debug!("Webhook ignored: {why}");
    (StatusCode::OK, format!("ignored: {why}\n"))
}

/// Queues the jobs a change on `repo`'s `branch` concerns.
fn queue_branch(queue: &Queue, path: &str, git: &str, repo: &str, branch: &str) -> Reply {
    if !is_valid_repo_path(repo) {
        return ignored("invalid repository path");
    }
    let jobs = match config::load_jobs(path) {
        Ok(jobs) => jobs_for_push(jobs, git, repo, branch),
        Err(err) => {
            log::error!("Webhook for '{repo}' [{branch}] dropped: {err}");
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("{err}\n"));
        }
    };
    if jobs.is_empty() {
        return ignored(&format!("no source collects {repo} [{branch}]"));
    }
    let mut queued = String::new();
    for job in jobs {
        log::info!("Webhook: queued {}", job.join(" "));
        queued += &format!("queued: {}\n", job.join(" "));
        queue.push(job, true);
    }
    (StatusCode::ACCEPTED, queued)
}

/// The branch a push touched: `None` for tags and deleted branches.
fn pushed_branch(git_ref: &str, deleted: bool) -> Option<String> {
    let branch = git_ref.strip_prefix("refs/heads/")?;
    (!deleted).then(|| branch.to_string())
}

/// PR actions that change what's stored (state, head, title, labels, draft).
const GITHUB_PR_ACTIONS: [&str; 9] = [
    "opened",
    "reopened",
    "synchronize",
    "closed",
    "edited",
    "ready_for_review",
    "converted_to_draft",
    "labeled",
    "unlabeled",
];

/// Finds the base branch of the open PR whose head is a commit (CI events
/// only name the commit; a fork PR's `check_suite` even lists no PR).
type HeadBranch<'a> = &'a dyn Fn(&str) -> Option<String>;

/// GitHub's hook form defaults to `application/x-www-form-urlencoded`, which
/// is still correctly signed: a 400 shows that under *Recent Deliveries*.
fn parse_json(body: &[u8]) -> Result<serde_json::Value, Reply> {
    serde_json::from_slice(body).map_err(|_| {
        let msg = "not JSON: set the hook's content type to application/json\n";
        (StatusCode::BAD_REQUEST, msg.to_string())
    })
}

fn github_hook(
    queue: &Queue,
    path: &str,
    secret: &str,
    headers: &HeaderMap,
    body: &[u8],
    head_branch: HeadBranch,
) -> Reply {
    if !github_signature_ok(secret, header(headers, "x-hub-signature-256"), body) {
        return (StatusCode::UNAUTHORIZED, "bad signature\n".to_string());
    }
    let event = header(headers, "x-github-event");
    match event {
        "ping" => return (StatusCode::OK, "pong\n".to_string()),
        "push" | "pull_request" | "check_suite" | "status" => {}
        _ => return ignored(&format!("'{event}' events aren't used")),
    }
    let payload = match parse_json(body) {
        Ok(payload) => payload,
        Err(reply) => return reply,
    };
    let str_of = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
    let branch = match event {
        "push" => pushed_branch(&str_of(&payload["ref"]), payload["deleted"] == true),
        "pull_request" if GITHUB_PR_ACTIONS.contains(&payload["action"].as_str().unwrap_or("")) => {
            Some(str_of(&payload["pull_request"]["base"]["ref"]))
        }
        "check_suite" if payload["action"] == "completed" => {
            head_branch(&str_of(&payload["check_suite"]["head_sha"]))
        }
        "status" if payload["state"] != "pending" => head_branch(&str_of(&payload["sha"])),
        _ => return ignored(&format!("'{event}' action/state not used")),
    };
    let Some(branch) = branch.filter(|b| !b.is_empty()) else {
        return ignored("no branch, or not an open PR's head");
    };
    let repo = str_of(&payload["repository"]["full_name"]);
    queue_branch(queue, path, "GH", &repo, &branch)
}

fn gitlab_hook(
    queue: &Queue,
    path: &str,
    secret: &str,
    headers: &HeaderMap,
    body: &[u8],
    head_branch: HeadBranch,
) -> Reply {
    let token = header(headers, "x-gitlab-token").as_bytes();
    if !bool::from(token.ct_eq(secret.as_bytes())) {
        return (StatusCode::UNAUTHORIZED, "bad token\n".to_string());
    }
    let payload = match parse_json(body) {
        Ok(payload) => payload,
        Err(reply) => return reply,
    };
    let str_of = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
    let attrs = &payload["object_attributes"];
    let branch = match payload["object_kind"].as_str().unwrap_or_default() {
        "push" => {
            let deleted = payload["after"]
                .as_str()
                .is_some_and(|sha| sha.bytes().all(|c| c == b'0'));
            pushed_branch(&str_of(&payload["ref"]), deleted)
        }
        "merge_request" => Some(str_of(&attrs["target_branch"])),
        "pipeline"
            if !matches!(
                attrs["status"].as_str(),
                Some("pending" | "running" | "created")
            ) =>
        {
            // Merged-results pipelines run on a merge commit, not the head.
            head_branch(&str_of(&attrs["sha"])).or_else(|| {
                payload["merge_request"]["target_branch"]
                    .as_str()
                    .map(str::to_string)
            })
        }
        kind => return ignored(&format!("'{kind}' events/states aren't used")),
    };
    let Some(branch) = branch.filter(|b| !b.is_empty()) else {
        return ignored("no branch, or not an open MR's head");
    };
    let repo = str_of(&payload["project"]["path_with_namespace"]);
    queue_branch(queue, path, "GL", &repo, &branch)
}

/// SIGTERM (`docker stop`) / Ctrl+C: TERMs the running job's process group,
/// so git drops its lock files instead of being SIGKILLed with the container,
/// and waits for it. Holding the queue lock keeps the worker from starting
/// anything else meanwhile.
fn stop(queue: &Queue) -> ! {
    let _jobs = queue.jobs.lock().unwrap();
    super::STOPPING.store(true, Ordering::SeqCst);
    let pid = super::RUNNING_JOB.load(Ordering::SeqCst);
    if pid > 0 {
        log::info!("Stopping the running job...");
        // SAFETY: plain kill(2) on the job's group, see `run_job`.
        unsafe { libc::kill(-pid, libc::SIGTERM) };
        let started = Instant::now();
        while super::RUNNING_JOB.load(Ordering::SeqCst) == pid
            && started.elapsed() < Duration::from_secs(25)
        {
            thread::sleep(Duration::from_millis(200));
        }
    }
    log::info!("Bye!");
    std::process::exit(0);
}

pub async fn run(path: &str) {
    if let Err(err) = config::load_jobs(path) {
        eprintln!("{err}");
        std::process::exit(2);
    }
    let queue = Arc::new(Queue::default());
    let pool = super::open_db();
    let (worker_queue, worker_pool) = (queue.clone(), pool.clone());
    thread::spawn(move || work(&worker_queue, &worker_pool));

    let hours: u64 = env::var("OGHCOLLECTOR_SCHEDULE_HOURS")
        .map(|h| {
            h.trim()
                .parse()
                .expect("OGHCOLLECTOR_SCHEDULE_HOURS: hours, 0 = off")
        })
        .unwrap_or(6);
    if hours > 0 {
        log::info!("Collecting every source of '{path}' every {hours}h");
        let (queue, path) = (queue.clone(), path.to_string());
        thread::spawn(move || schedule(&queue, &path, hours * 3600));
    }

    // A hook without a secret is never served: a valid delivery starts clones.
    let mut router = Router::new();
    let mut hooks = Vec::new();
    for (git, route) in [("GH", "/hooks/github"), ("GL", "/hooks/gitlab")] {
        let file = format!("{}_webhook_secret", git.to_lowercase());
        let Some(secret) =
            config::read_secret(&file, &format!("OGHCOLLECTOR_WEBHOOK_SECRET_{git}"))
        else {
            continue;
        };
        let (queue, path, pool) = (queue.clone(), path.to_string(), pool.clone());
        let handler = move |headers: HeaderMap, body: Bytes| async move {
            let head_branch = |sha: &str| {
                let mut conn = pool.get().ok()?;
                let version = models::pull_request::get_version_by_head_sha(&mut conn, sha)?;
                Some(odoo_version_u8_to_string(&(version as u8)))
            };
            match git {
                "GH" => github_hook(&queue, &path, &secret, &headers, &body, &head_branch),
                _ => gitlab_hook(&queue, &path, &secret, &headers, &body, &head_branch),
            }
        };
        router = router.route(route, post(handler));
        hooks.push(route);
    }
    let server = async {
        if hooks.is_empty() {
            log::info!("No webhook secret set, webhooks are off");
            return std::future::pending().await;
        }
        let addr = env::var("OGHCOLLECTOR_WEBHOOK_BIND_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:8082".to_string());
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .unwrap_or_else(|err| panic!("Can't listen on {addr}: {err}"));
        log::info!("Webhooks listening on http://{addr}{{{}}}", hooks.join(","));
        // axum's 2MB default would 413 a push touching thousands of files.
        // `layer` after the routes: it only wraps those already added.
        axum::serve(listener, router.layer(DefaultBodyLimit::max(25 << 20))).await
    };
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("Can't handle SIGTERM");
    tokio::select! {
        res = server => if let Err(err) = res {
            log::error!("Webhook listener failed: {err}");
        },
        _ = sigterm.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    stop(&queue);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(origin: &str, ver: &str, git: &str) -> Vec<String> {
        [origin, ver, git, "all", "migration", "default", "-"]
            .map(str::to_string)
            .to_vec()
    }

    #[test]
    fn a_push_runs_its_repository_of_each_matching_source() {
        let jobs = vec![
            job("odoo/odoo:/addons,/odoo/addons", "18.0", "GH"),
            job("OCA", "17.0", "GH"),
            job("OCA", "18.0", "GH"),
            job("grp/sub", "18.0", "GL:https://git.acme.dev/api/v4/"),
        ];
        let push = |git, repo, branch| jobs_for_push(jobs.clone(), git, repo, branch);
        assert_eq!(
            push("GH", "OCA/web", "18.0"),
            [job("OCA/web", "18.0", "GH")]
        );
        assert_eq!(
            push("GH", "odoo/odoo", "18.0"),
            [job("odoo/odoo:/addons,/odoo/addons", "18.0", "GH")]
        );
        assert_eq!(push("GH", "oca/web", "18.0")[0][0], "oca/web");
        assert_eq!(
            push("GL", "grp/sub/proj", "18.0"),
            [job(
                "grp/sub/proj",
                "18.0",
                "GL:https://git.acme.dev/api/v4/"
            )]
        );
        // Not a version branch, another git, a subgroup's project.
        assert!(push("GH", "OCA/web", "18.0-mig-web_foo").is_empty());
        assert!(push("GL", "OCA/web", "18.0").is_empty());
        assert!(push("GL", "grp/sub/deeper/proj", "18.0").is_empty());
        assert!(push("GH", "Acme/web", "18.0").is_empty());
    }

    #[test]
    fn webhooks_are_authenticated() {
        // The example on GitHub's "Validating webhook deliveries" page.
        let (secret, body) = ("It's a Secret to Everybody", b"Hello, World!");
        let sig = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        assert!(github_signature_ok(secret, sig, body));
        assert!(!github_signature_ok(secret, sig, b"Hello, World?"));
        assert!(!github_signature_ok("other", sig, body));
        assert!(!github_signature_ok(secret, "", body));

        // A correctly signed form-encoded delivery isn't silently ignored.
        let queue = Queue::default();
        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "push".parse().unwrap());
        headers.insert("x-hub-signature-256", sig.parse().unwrap());
        let reply = github_hook(&queue, "-", secret, &headers, body, &|_| None);
        assert_eq!(reply.0, StatusCode::BAD_REQUEST);

        let mut headers = HeaderMap::new();
        headers.insert("x-gitlab-token", "wrong".parse().unwrap());
        let reply = gitlab_hook(&queue, "-", "right", &headers, b"{}", &|_| None);
        assert_eq!(reply.0, StatusCode::UNAUTHORIZED);
        headers.insert("x-gitlab-token", "right".parse().unwrap());
        assert_eq!(
            gitlab_hook(&queue, "-", "right", &headers, b"{}", &|_| None).0,
            StatusCode::OK
        );
        assert!(queue.jobs.lock().unwrap().is_empty());
    }

    #[test]
    fn pull_request_and_ci_events_queue_their_base_branch() {
        let dir = env::temp_dir().join(format!("ogh-hooks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("collector.yaml");
        std::fs::write(&path, "sources:\n- {source: OCA, versions: [18.0]}\n").unwrap();
        let path = path.to_str().unwrap();
        let secret = "s";
        let deliver = |event: &str, body: serde_json::Value| {
            let body = body.to_string();
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
            mac.update(body.as_bytes());
            let sig: String = mac
                .finalize()
                .into_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let mut headers = HeaderMap::new();
            headers.insert("x-github-event", event.parse().unwrap());
            headers.insert(
                "x-hub-signature-256",
                format!("sha256={sig}").parse().unwrap(),
            );
            let queue = Queue::default();
            let known = |sha: &str| (sha == "abc").then(|| "18.0".to_string());
            let status = github_hook(&queue, path, secret, &headers, body.as_bytes(), &known).0;
            let jobs: Vec<Vec<String>> = queue.jobs.lock().unwrap().iter().cloned().collect();
            (status, jobs)
        };
        let repo = serde_json::json!({"full_name": "OCA/web"});
        let queued = (StatusCode::ACCEPTED, vec![job("OCA/web", "18.0", "GH")]);
        let pr = |action: &str| {
            serde_json::json!({"action": action, "repository": repo,
                               "pull_request": {"base": {"ref": "18.0"}}})
        };
        assert_eq!(deliver("pull_request", pr("opened")), queued);
        assert_eq!(deliver("pull_request", pr("assigned")).0, StatusCode::OK);
        let suite = |sha: &str| {
            serde_json::json!({"action": "completed", "repository": repo,
                               "check_suite": {"head_sha": sha, "pull_requests": []}})
        };
        assert_eq!(deliver("check_suite", suite("abc")), queued);
        assert_eq!(
            deliver("check_suite", suite("zzz")),
            (StatusCode::OK, vec![])
        );
        let status =
            |state: &str| serde_json::json!({"state": state, "sha": "abc", "repository": repo});
        assert_eq!(deliver("status", status("success")), queued);
        assert_eq!(deliver("status", status("pending")).0, StatusCode::OK);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn repo_paths_are_safe() {
        assert!(is_valid_repo_path("OCA/web"));
        assert!(is_valid_repo_path("grp/sub/my.proj_2-x"));
        for bad in ["web", "OCA/..", "OCA/a:b", "OCA/a,b", "OCA//web", "OCA/a b"] {
            assert!(!is_valid_repo_path(bad), "{bad}");
        }
    }
}
