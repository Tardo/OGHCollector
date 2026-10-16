// Copyright Alexandre D. Díaz
//! Orchestrates the pipeline; uses `named-lock` to prevent concurrent runs for the same org. A run
//! fails iff the `collector` target logged an error (`LOGGED_ERRORS`, counted by the
//! `ErrorCountingLogger` wrapper around env_logger; a panic hook routes panics through
//! `log::error!`): what it collected stays saved, it skips the "Scan finished" event and exits 1.
//! So every failure path must end in a `log::error!` — a silent `break`/empty `Vec` is how
//! "collected nothing, exit 0, no log" happened. `run_config` pipes each child's stderr through and
//! records each failed job (`run_and_record`, shared with `listen`) as `Scan failed for '<source>'
//! [<ver>]: <first error> (N error(s), exit code|signal)` (`system_event` collector/error), signal
//! deaths (OOM) included. A hung run used to stall every later one (Ofelia printed output only at
//! job end and `no-overlap` skipped later runs; the daemon's single worker would stall the same
//! way): nothing may block forever — `run_job` kills a job (its own process group: git too, TERM
//! first so git drops its lock files) after `JOB_TIMEOUT` (4h), `gitclient::http_client` (every
//! reqwest client) has connect/request timeouts and `git_run_env` a low-speed limit. The batch
//! parent's embeddings pass is not covered (the daemon's is: it's a `run_job` child).
mod actions;
#[cfg(test)]
mod actions_tests;
mod analyzer;
mod anygitclient;
mod clients;
mod config;
mod embeddings;
mod gitclient;
#[cfg(test)]
mod golden;
mod listen;
mod migration;
mod pull_requests;
mod pypi;
mod review;
mod security;
mod workflow;

use named_lock::NamedLock;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use analyzer::OGHCollectorAnalyzer;
use anygitclient::AnyGitClient;
use clients::github::GithubClient;
use clients::gitlab::GitlabClient;
use config::{GitType, OGHCollectorConfig};
use gitclient::{GitClient, RepoInfo};
use oghutils::version::odoo_version_u8_to_string;
use pypi::PypiClient;
use sqlitedb::models;

// The guard must stay alive for the whole run: dropping it releases the lock,
// which is why this returns it instead of letting it die inside the function.
//
// `NamedLock::create()` locks a file under `$TMPDIR`/`/tmp`, which is private
// to each container - in Docker (separate one-off invocations or a
// `docker compose exec`) a lock under `/tmp` would not serialize across containers,
// so this never actually serialized concurrent runs for the same org despite
// looking like it does. Locking inside `data/` (the volume every invocation
// shares) instead makes it a real cross-process/cross-container lock. Without
// it, two overlapping runs for the same org (e.g. a manual run overlapping
// with the daemon's job) race on the same `data/repos/<org>/<repo>` checkout:
// one run's `git reset --hard` to its branch can land mid-scan of another
// run's analyzer, which still stamps
// every module with its own run's requested Odoo version - so one version's
// branch content ends up stored under a different version's label.
fn try_lock(config: &OGHCollectorConfig) -> named_lock::NamedLockGuard {
    let source_info = config.get_source().split('/').collect::<Vec<&str>>();
    let org = source_info[0];
    fs::create_dir_all("data").expect("Can't create the data directory");
    let lock_path = format!("data/.{org}.collector.lock");
    let lock = NamedLock::with_path(&lock_path).expect("Can't create the collector lock");
    match lock.try_lock() {
        Ok(guard) => guard,
        Err(_) => {
            log::error!(
                "There is already an instance of OGHCollector working with '{org}'. Exiting..."
            );
            std::process::exit(1);
        }
    }
}

/// Errors logged by this crate so far. Every failure path (source listing,
/// API, clone/fetch, panics via the hook in `main`) already ends in a
/// `log::error!`, so a run that logged any is a failed run: it exits non-zero
/// instead of "All done", which is what makes it visible and recorded
/// (`run_and_record`).
static LOGGED_ERRORS: AtomicUsize = AtomicUsize::new(0);

struct ErrorCountingLogger(env_logger::Logger);

impl log::Log for ErrorCountingLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() == log::Level::Error || self.0.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if record.level() == log::Level::Error && record.target().starts_with("collector") {
            LOGGED_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
        self.0.log(record);
    }

    fn flush(&self) {
        self.0.flush();
    }
}

fn init_logging() {
    let logger =
        env_logger::Builder::from_env(env_logger::Env::new().default_filter_or("info")).build();
    // Errors always reach `log()` to be counted, even when RUST_LOG hides them.
    log::set_max_level(logger.filter().max(log::LevelFilter::Error));
    log::set_boxed_logger(Box::new(ErrorCountingLogger(logger))).expect("Logger already set");
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map_or(String::new(), |l| format!(" at {l}"));
        let msg = info.payload_as_str().unwrap_or("unknown panic");
        log::error!("Crashed{location}: {msg}");
    }));
}

/// The collector config of this run (`--config`), handed to every job it
/// starts (`OGHCOLLECTOR_CONFIG`) for the workflow definitions.
static CONFIG_PATH: OnceLock<String> = OnceLock::new();
const CONFIG_ENV: &str = "OGHCOLLECTOR_CONFIG";

fn config_path() -> &'static str {
    CONFIG_PATH.get_or_init(|| env::var(CONFIG_ENV).unwrap_or_else(|_| "collector".to_string()))
}

/// Pid (= process group) of the job `run_job` is running, 0 when none: what
/// `listen` stops on SIGTERM.
static RUNNING_JOB: AtomicI32 = AtomicI32::new(0);

/// Set by `listen` when it stops: the job it interrupts isn't a failed scan.
static STOPPING: AtomicBool = AtomicBool::new(false);

/// Runs one `collector.yaml` job as a child process, passing its log (stderr)
/// through as it comes. `Err` = why it failed: its first logged error plus
/// how it ended.
fn run_job(exe: &Path, args: &[String], timeout: Duration) -> Result<(), String> {
    let mut child = Command::new(exe)
        .args(args)
        .env(SKIP_EMBEDDINGS_ENV, "1")
        .env(CONFIG_ENV, config_path())
        .stderr(Stdio::piped())
        // Its own process group, so a timeout kills its git subprocesses too.
        // ponytail: a terminal Ctrl+C then stops the batch but not the running job.
        .process_group(0)
        .spawn()
        .map_err(|err| format!("can't start it: {err}"))?;
    RUNNING_JOB.store(child.id() as i32, Ordering::SeqCst);
    let stderr = child.stderr.take().expect("stderr is piped");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut first_error, mut errors) = (None, 0);
        // Split on bytes, not `lines()`: git output isn't always UTF-8, and
        // stopping to read would leave the child blocked on a full pipe.
        for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
            let line = String::from_utf8_lossy(&line);
            let _ = writeln!(io::stderr(), "{line}");
            // Same errors `LOGGED_ERRORS` counts (env_logger: "[<time> ERROR <target>] ").
            if line.contains(" ERROR collector") {
                errors += 1;
                first_error.get_or_insert_with(|| {
                    line.split_once("] ")
                        .map_or(line.to_string(), |(_, m)| m.to_string())
                });
            }
        }
        let _ = tx.send((first_error, errors));
    });
    // stderr closes when the job (and every subprocess) is gone.
    let mut read = rx.recv_timeout(timeout);
    let mut killed = false;
    // TERM first: git removes its lock files on it.
    for signal in [libc::SIGTERM, libc::SIGKILL] {
        if !matches!(read, Err(RecvTimeoutError::Timeout)) {
            break;
        }
        killed = true;
        // SAFETY: plain kill(2) on the group `process_group(0)` created; the
        // child isn't reaped yet, so its pid (= group id) can't be reused.
        unsafe { libc::kill(-(child.id() as i32), signal) };
        read = rx.recv_timeout(Duration::from_secs(30));
    }
    let (first_error, errors) = read.unwrap_or_default();
    let status = child.wait();
    RUNNING_JOB.store(0, Ordering::SeqCst);
    let _ = fs::remove_dir_all(job_workdir(child.id()));
    let status = status.map_err(|err| format!("can't wait for it: {err}"))?;
    if status.success() && !killed {
        return Ok(());
    }
    let ended = match (status.code(), status.signal()) {
        _ if killed => format!("timed out after {}s, killed", timeout.as_secs()),
        (Some(code), _) => format!("exit code {code}"),
        (_, Some(9)) => "killed by signal 9 (out of memory?)".to_string(),
        (_, signal) => format!("killed by signal {}", signal.unwrap_or_default()),
    };
    Err(match first_error {
        Some(err) => format!("{err} ({errors} error(s), {ended})"),
        None => format!("{ended}, see the collector log"),
    })
}

/// A job running longer is killed and recorded as failed, so a hang (none of
/// git/HTTP used to time out) can't stall the batch and, the `--listen` daemon
/// running one job at a time, every later one. Longer than the worst rate-limit
/// wait of a request (2 x 1h, see `gitclient::retry_delay`); a whole batch
/// normally takes ~1.5h.
const JOB_TIMEOUT: Duration = Duration::from_secs(4 * 3600);

/// A job's own work area for PR code (`pull_requests`), never the shared
/// clones. `run_job` removes a child's when it ends, killed ones included.
fn job_workdir(pid: u32) -> PathBuf {
    env::temp_dir().join(format!("oghcollector-{pid}"))
}

/// PRs analyzed per job at most (`OGHCOLLECTOR_PR_ANALYSIS_LIMIT`), so a first
/// run over every open PR can't hold the daemon's queue for hours: the rest
/// are analyzed by later runs, most recently active first.
fn pr_analysis_limit() -> usize {
    env::var("OGHCOLLECTOR_PR_ANALYSIS_LIMIT")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(100)
}

/// AI calls per job at most (`OGHCOLLECTOR_AI_LIMIT`): with `ai.timeout` at
/// most 600s, the default stays well within `JOB_TIMEOUT`.
fn ai_limit() -> usize {
    env::var("OGHCOLLECTOR_AI_LIMIT")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(20)
}

/// Set on the children of `run_config`, which runs the embeddings pass once
/// after all of them instead of once per job.
const SKIP_EMBEDDINGS_ENV: &str = "OGHCOLLECTOR_SKIP_EMBEDDINGS";

fn open_db() -> sqlitedb::Pool {
    let db_path = "data/data.db";
    if let Some(parent) = Path::new(db_path).parent() {
        fs::create_dir_all(parent).unwrap();
    }
    if !Path::new(db_path).exists() {
        File::create(db_path).unwrap();
    }
    let pool = sqlitedb::new_write_pool(db_path);
    sqlitedb::run_migrations(&mut pool.get().unwrap()).expect("Can't run migrations");
    pool
}

/// Runs a job (`run_job`) and records its failure in the event log. `false`
/// when it failed.
fn run_and_record(exe: &Path, args: &[String], pool: &sqlitedb::Pool) -> bool {
    log::info!("Running job: {}", args.join(" "));
    let Err(reason) = run_job(exe, args, JOB_TIMEOUT) else {
        return true;
    };
    if STOPPING.load(Ordering::SeqCst) {
        log::info!("Job interrupted: {}", args.join(" "));
        return false;
    }
    log::error!("Job failed: {}: {reason}", args.join(" "));
    let source = args[0].split(':').next().unwrap_or_default();
    let recorded = pool
        .get()
        .map_err(|err| err.to_string())
        .and_then(|mut conn| {
            models::system_event::register_failed_task_collector(
                &mut conn, source, &args[1], &reason,
            )
            .map_err(|err| err.to_string())
        });
    if let Err(err) = recorded {
        log::error!("Can't record the failure in the event log: {err}");
    }
    false
}

/// `oghcollector [--config <file>] [--check]`: runs every job of the
/// collector config (default `./collector.yaml`) as its own single-run child
/// process. The single-run path panics/exits freely (busy lock, missing token,
/// DB unwraps), so a child keeps that to its own job and the rest of the batch
/// still runs. `--check` only validates and prints the jobs, one per line.
fn run_config(path: &str, check: bool) {
    let jobs = config::load_jobs(path).unwrap_or_else(|err| {
        eprintln!("{err}");
        std::process::exit(2);
    });
    if check {
        for args in &jobs {
            println!("{}", args.join(" "));
        }
        return;
    }
    let exe = env::current_exe().expect("Can't locate the collector binary");
    let pool = open_db();
    let mut failed: Vec<String> = Vec::new();
    for args in &jobs {
        if !run_and_record(&exe, args, &pool) {
            failed.push(args[..2].join(" "));
        }
    }
    embeddings::update_embeddings(&mut pool.get().unwrap());
    // The failed jobs again in the last line, where a long log is read.
    if !failed.is_empty() {
        log::error!(
            "{} of {} jobs failed: {}",
            failed.len(),
            jobs.len(),
            failed.join(", ")
        );
        std::process::exit(1);
    }
    log::info!("All done. Bye!");
}

#[tokio::main]
async fn main() {
    init_logging();
    let args: Vec<String> = env::args().collect();
    let usage = || -> ! {
        eprintln!(
            "Usage: oghcollector [--config <file>] [--check|--listen]\n       oghcollector --embeddings\n       oghcollector <source>[:<paths>] <version> [<git> [<workspaces>|all [migration|full [<workflows>|-]]]]"
        );
        std::process::exit(2);
    };
    if args.len() == 1 || args[1].starts_with("--") {
        let (mut path, mut check, mut listen) = ("collector", false, false);
        let mut flags = args[1..].iter();
        while let Some(flag) = flags.next() {
            match flag.as_str() {
                "--check" => check = true,
                "--listen" => listen = true,
                "--config" => path = flags.next().unwrap_or_else(|| usage()),
                // Only the embeddings pass: what `listen` runs after its jobs.
                "--embeddings" => {
                    return embeddings::update_embeddings(&mut open_db().get().unwrap())
                }
                _ => usage(),
            }
        }
        let _ = CONFIG_PATH.set(path.to_string());
        if listen {
            return listen::run(path).await;
        }
        return run_config(path, check);
    }
    if args.len() < 3 {
        usage();
    }
    let config = OGHCollectorConfig::new(&args);

    let _lock_guard = try_lock(&config);

    // Before any work: a bad workflow fails the job right away.
    let engine = config::load_workflows(config_path())
        .and_then(|defs| workflow::Engine::new(config.get_workflows(), &defs))
        .unwrap_or_else(|err| {
            log::error!("{err}");
            std::process::exit(1);
        });

    let git_client = match config.get_git_type() {
        GitType::Github => {
            AnyGitClient::Github(GithubClient::new(config.get_token(), config.get_base_url()))
        }
        GitType::Gitlab => {
            AnyGitClient::Gitlab(GitlabClient::new(config.get_token(), config.get_base_url()))
        }
    };
    let pypi_client = PypiClient::new();

    let pool = open_db();
    let mut conn = pool.get().unwrap();

    let odoo_ver = config.get_version_odoo();
    let odoo_ver_str = odoo_version_u8_to_string(odoo_ver);
    let start_time = Instant::now();
    let _ = models::system_event::register_started_task_collector(
        &mut conn,
        config.get_source(),
        &odoo_ver_str,
    );
    log::info!("Cloning/Updating ({})...", odoo_ver_str);
    let repo_infos: Vec<RepoInfo> = git_client
        .clone_source(
            config.get_source(),
            config.get_branch(),
            config.get_repos_path(),
        )
        .await;

    let mut gh_repo_ids: Vec<i64> = Vec::with_capacity(repo_infos.len());
    for repo_info in &repo_infos {
        let gh_org = models::gh_organization::add(&mut conn, repo_info.get_org()).unwrap();
        let gh_repo =
            models::gh_repository::add(&mut conn, &gh_org.id, repo_info.get_name()).unwrap();
        models::gh_repository::set_url(&mut conn, &gh_repo.id, repo_info.get_url()).unwrap();
        if let Some(workspaces) = config.get_workspaces() {
            models::gh_repository_workspace::set(&mut conn, &gh_repo.id, workspaces).unwrap();
        }
        gh_repo_ids.push(gh_repo.id);
    }

    let analyzer = OGHCollectorAnalyzer::new(odoo_ver);
    let workdir = job_workdir(std::process::id());
    let forge = actions::Forge::new(
        matches!(config.get_git_type(), GitType::Gitlab),
        config.get_base_url(),
    );
    let deps = workflow::Deps {
        analyzer: &analyzer,
        pypi: &pypi_client,
        forge: &forge,
        actions: config.get_actions(),
        branch: config.get_branch(),
        workdir: &workdir,
        ai_calls_left: std::cell::Cell::new(ai_limit()),
        ai_limit_warned: std::cell::Cell::new(false),
        notifications_left: std::cell::Cell::new(workflow::NOTIFY_LIMIT),
        notify_limit_warned: std::cell::Cell::new(false),
        dry_runs_left: std::cell::Cell::new(workflow::DRY_RUN_EVENTS),
        dry_runs_held: std::cell::Cell::new(0),
    };
    let manifest_count = workflow::run_modules(
        &mut conn,
        &engine,
        &deps,
        config.get_read_paths(),
        &repo_infos,
        &gh_repo_ids,
        odoo_ver,
    )
    .await;

    log::info!("Scanning '{}' repos for pull requests...", repo_infos.len());
    let mut budget = pull_requests::Budget {
        analyses: pr_analysis_limit(),
        ci_lookups: pull_requests::CI_LOOKUPS_PER_JOB,
    };
    for (repo_info, gh_repo_id) in repo_infos.iter().zip(gh_repo_ids) {
        let prs = pull_requests::RepoPrs {
            git: &git_client,
            repo: repo_info,
            gh_repo_id,
            branch: config.get_branch(),
            version_odoo: *odoo_ver,
            read_paths: config.get_read_paths(),
            analyzer: &analyzer,
            workdir: &workdir,
            full: config.get_full_pull_requests(),
            engine: &engine,
            deps: &deps,
        };
        pull_requests::collect(&prs, &mut conn, &mut budget).await;
    }
    let _ = fs::remove_dir_all(&workdir);
    deps.log_held_dry_runs(&mut conn);

    // What was collected stays saved either way; a failed run is recorded by
    // `run_config` (with its first error) instead of as finished.
    let errors = LOGGED_ERRORS.load(Ordering::Relaxed);
    if errors == 0 {
        let _ = models::system_event::register_finished_task_collector(
            &mut conn,
            &start_time.elapsed().as_secs().to_string(),
            &manifest_count.to_string(),
            config.get_source(),
            &odoo_ver_str,
        );
    }

    if env::var_os(SKIP_EMBEDDINGS_ENV).is_none() {
        embeddings::update_embeddings(&mut conn);
    }

    if errors > 0 {
        // warn: `run_job` counts the error lines, this one only sums them up.
        log::warn!("Finished with {errors} error(s), see above");
        std::process::exit(1);
    }
    log::info!("All done. Bye!");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_job_kills_a_hung_job_and_its_subprocesses() {
        let started = Instant::now();
        // The background sleep holds stderr open too: only killing the whole
        // group ends the job before the 30s grace periods.
        let args = ["-c".to_string(), "sleep 60 & sleep 60".to_string()];
        let err = run_job(Path::new("sh"), &args, Duration::from_secs(1)).unwrap_err();
        assert!(err.starts_with("timed out after 1s"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
