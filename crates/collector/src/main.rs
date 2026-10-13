// Copyright Alexandre D. Díaz
mod analyzer;
mod anygitclient;
mod clients;
mod config;
mod embeddings;
mod gitclient;
mod migration;
mod pypi;
mod security;

use named_lock::NamedLock;
use regex::Regex;
use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
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
// to each container - in Docker (separate one-off invocations or Ofelia's
// scheduled exec) a lock under `/tmp` would not serialize across containers,
// so this never actually serialized concurrent runs for the same org despite
// looking like it does. Locking inside `data/` (the volume every invocation
// shares) instead makes it a real cross-process/cross-container lock. Without
// it, two overlapping runs for the same org (e.g. a manual run overlapping
// with Ofelia's refresh) race on the same `data/repos/<org>/<repo>` checkout:
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
/// instead of "All done", which is what makes it visible (Ofelia only prints a
/// job's output when it ends) and recorded (`run_config`).
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

/// Runs one `collector.yaml` job as a child process, passing its log (stderr)
/// through as it comes. `Err` = why it failed: its first logged error plus
/// how it ended.
fn run_job(exe: &Path, args: &[String], timeout: Duration) -> Result<(), String> {
    let mut child = Command::new(exe)
        .args(args)
        .env(SKIP_EMBEDDINGS_ENV, "1")
        .stderr(Stdio::piped())
        // Its own process group, so a timeout kills its git subprocesses too.
        // ponytail: a terminal Ctrl+C then stops the batch but not the running job.
        .process_group(0)
        .spawn()
        .map_err(|err| format!("can't start it: {err}"))?;
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
    let status = child
        .wait()
        .map_err(|err| format!("can't wait for it: {err}"))?;
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
/// git/HTTP used to time out) can't stall the batch and, through Ofelia's
/// `no-overlap`, every later scheduled run. Longer than the worst rate-limit
/// wait of a request (2 x 1h, see `gitclient::retry_delay`); a whole batch
/// normally takes ~1.5h.
const JOB_TIMEOUT: Duration = Duration::from_secs(4 * 3600);

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
        log::info!("Running job: {}", args.join(" "));
        let Err(reason) = run_job(&exe, args, JOB_TIMEOUT) else {
            continue;
        };
        log::error!("Job failed: {}: {reason}", args.join(" "));
        failed.push(args[..2].join(" "));
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
    }
    embeddings::update_embeddings(&mut pool.get().unwrap());
    // The failed jobs again in the last line: Ofelia keeps a bounded tail of
    // a long run's output.
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
            "Usage: oghcollector [--config <file>] [--check]\n       oghcollector <source>[:<paths>] <version> [<git> [<workspaces>|all]]"
        );
        std::process::exit(2);
    };
    if args.len() == 1 || args[1].starts_with("--") {
        let (mut path, mut check) = ("collector", false);
        let mut flags = args[1..].iter();
        while let Some(flag) = flags.next() {
            match flag.as_str() {
                "--check" => check = true,
                "--config" => path = flags.next().unwrap_or_else(|| usage()),
                _ => usage(),
            }
        }
        return run_config(path, check);
    }
    if args.len() < 3 {
        usage();
    }
    let config = OGHCollectorConfig::new(&args);

    let _lock_guard = try_lock(&config);

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

    log::info!("Scanning '{}' repos for migration PRs...", repo_infos.len());
    for repo_info in &repo_infos {
        let gh_org = models::gh_organization::add(&mut conn, repo_info.get_org()).unwrap();
        let gh_repo =
            models::gh_repository::add(&mut conn, &gh_org.id, repo_info.get_name()).unwrap();
        models::gh_repository::set_url(&mut conn, &gh_repo.id, repo_info.get_url()).unwrap();
        if let Some(workspaces) = config.get_workspaces() {
            models::gh_repository_workspace::set(&mut conn, &gh_repo.id, workspaces).unwrap();
        }
        let Some(migration_prs) = git_client
            .get_open_migration_pull_requests(repo_info.get_full_path(), config.get_branch())
            .await
        else {
            // Unknown (already logged as an error), not "all closed": keep
            // the stored PRs instead of closing every one of them.
            continue;
        };
        let mut prids: Vec<i64> = Vec::with_capacity(migration_prs.len());
        for pr in &migration_prs {
            prids.push(pr.number);
            models::pull_request::add(
                &mut conn,
                &pr.title,
                &pr.module_technical_name,
                &pr.number,
                odoo_ver,
                &gh_repo.id,
                pr.created_at.as_deref(),
                pr.ci_status.as_deref(),
                pr.last_message_at.as_deref(),
            )
            .unwrap();
        }
        let outdated =
            models::pull_request::find_outdated(&mut conn, &gh_repo.id, odoo_ver, &prids)
                .unwrap_or_default();
        let mut merged_prids: Vec<i64> = Vec::with_capacity(outdated.len());
        for pr in &outdated {
            if let Some(true) = git_client
                .is_pull_request_merged(repo_info.get_full_path(), &pr.prid)
                .await
            {
                merged_prids.push(pr.prid);
            }
        }
        let _ = models::pull_request::delete_outdated(
            &mut conn,
            &gh_repo.id,
            odoo_ver,
            &prids,
            &merged_prids,
        );
    }

    log::info!("Analyzing '{}' repos...", repo_infos.len());
    let analyzer = OGHCollectorAnalyzer::new(odoo_ver);
    let manifest_infos = analyzer.get_module_info(&mut conn, config.get_read_paths(), &repo_infos);
    let manifest_count = &manifest_infos.len();
    if manifest_count.gt(&0) {
        log::info!("Saving '{}' repos info...", manifest_infos.len());
        let mut module_ids_by_repo: HashMap<i64, Vec<i64>> = HashMap::new();
        let dep_type_module = models::dependency_type::get_by_name(&mut conn, "module")
            .expect("Can't find the module dependency type");
        let dep_type_python = models::dependency_type::get_by_name(&mut conn, "python")
            .expect("Can't find the python dependency type");
        let dep_type_bin = models::dependency_type::get_by_name(&mut conn, "bin")
            .expect("Can't find the bin dependency type");
        let re = Regex::new(r"^([^><=]+).+?([^><=]+)$").unwrap();
        for manifest in manifest_infos {
            let mut new_module_info = manifest.clone();
            new_module_info.version_odoo = *odoo_ver; // It is forced because some modules do not have this data correctly.
            let new_module = models::module::add(&mut conn, &new_module_info).unwrap();
            let module_ids = module_ids_by_repo
                .entry(new_module.gh_repository_id)
                .or_default();
            module_ids.push(new_module.id);

            // The analyzer already skipped re-parsing this module's source
            // (see OGHCollectorAnalyzer::get_module_info) when its last
            // commit hash matched what's already stored, so `analysis` here
            // is empty - replacing stored data with it would wipe it.
            if !new_module_info.source_unchanged {
                // Resolve (or start) the history entry for this manifest version,
                // then replace the module's code analysis (views touched, models
                // defined/extended with their fields and public methods, and
                // every other record it touches - access groups, record rules,
                // access rights, ...) scoped to that version, on every run,
                // independent of whether any manifest field changed. A prior
                // version's snapshot is left untouched - only its own
                // module_version_id gets wiped/rebuilt.
                let module_version = models::module_version::get_or_create(
                    &mut conn,
                    &new_module.id,
                    &new_module_info.version_module,
                )
                .unwrap();
                models::module_view::replace_for_module(
                    &mut conn,
                    &new_module.id,
                    &module_version.id,
                    &new_module_info.analysis.views,
                )
                .unwrap();
                models::module_model::replace_for_module(
                    &mut conn,
                    &new_module.id,
                    &module_version.id,
                    &new_module_info.analysis.models,
                )
                .unwrap();
                models::module_record::replace_for_module(
                    &mut conn,
                    &new_module.id,
                    &module_version.id,
                    &new_module_info.analysis.records,
                )
                .unwrap();
                models::module_controller::replace_for_module(
                    &mut conn,
                    &new_module.id,
                    &module_version.id,
                    &new_module_info.analysis.controllers,
                )
                .unwrap();

                // Static security checks (ACLs, record rules, controllers and
                // code-level sinks, see collector::security). Every finding is
                // stored; minor ones also leave a system_event log line, but
                // only when new for this version snapshot, so re-analysis
                // (a new commit, an ANALYSIS_VERSION bump) doesn't re-log
                // findings already reported.
                let sec_warnings = security::analyze_module(
                    &new_module.technical_name,
                    &new_module_info.category,
                    &new_module_info.analysis,
                );
                let known: std::collections::HashSet<(String, Option<String>)> =
                    models::module_security_warning::get_by_module_version_id(
                        &mut conn,
                        &module_version.id,
                    )
                    .into_iter()
                    .map(|w| (w.code, w.xml_id))
                    .collect();
                for w in sec_warnings.iter().filter(|w| {
                    w.severity != models::module_security_warning::SEVERITY_ERROR
                        && !known.contains(&(w.code.clone(), w.xml_id.clone()))
                }) {
                    let _ = models::system_event::register_security_warning(
                        &mut conn,
                        &new_module.technical_name,
                        &new_module.name,
                        &odoo_ver_str,
                        w.xml_id.as_deref(),
                        &w.message,
                    );
                }
                models::module_security_warning::replace_for_module(
                    &mut conn,
                    &new_module.id,
                    &module_version.id,
                    &sec_warnings,
                )
                .unwrap();

                // "What to check before/after upgrading this module" findings
                // over the same analysis pass (see collector::migration).
                let mig_notes = migration::analyze_module(*odoo_ver, &new_module_info.analysis);
                models::module_migration_note::replace_for_module(
                    &mut conn,
                    &new_module.id,
                    &module_version.id,
                    &mig_notes,
                )
                .unwrap();
                let _ = models::module::set_analysis_version(
                    &mut conn,
                    &new_module.id,
                    analyzer::ANALYSIS_VERSION,
                );
            }

            // Check Odoo Version
            if manifest.version_odoo.ne(odoo_ver) && manifest.installable {
                let repo_name =
                    models::gh_repository::get_by_id(&mut conn, &new_module.gh_repository_id)
                        .map(|r| r.name)
                        .unwrap_or_default();
                let _ = models::system_event::register_problem_module_version(
                    &mut conn,
                    &new_module.technical_name,
                    &new_module.name,
                    &repo_name,
                    odoo_version_u8_to_string(&manifest.version_odoo).as_str(),
                    &odoo_ver_str,
                );
            }

            // Add Odoo deps.
            let module_depends = models::dependency_module::get_names(
                &mut conn,
                &new_module.id,
                &dep_type_module.id,
            );
            let module_depends_to_remove: Vec<&String> = module_depends
                .iter()
                .filter(|item| !manifest.depends.contains(item))
                .collect();
            let module_depends_to_add: Vec<&String> = manifest
                .depends
                .iter()
                .filter(|item| !module_depends.contains(&item.to_string()))
                .collect();
            for module_depend_name in module_depends_to_remove {
                let module_depend_id_opt = models::dependency::get_by_name(
                    &mut conn,
                    &dep_type_module.id,
                    module_depend_name,
                );
                if let Some(module_depend_id) = module_depend_id_opt {
                    let _ = models::dependency_module::delete_by_module_id_dependency_id(
                        &mut conn,
                        &new_module.id,
                        &module_depend_id.id,
                    );
                    let _ = models::system_event::register_delete_module_dependency(
                        &mut conn,
                        &module_depend_id.name,
                        "Odoo",
                        &new_module.technical_name,
                        &new_module.name,
                        odoo_version_u8_to_string(&(new_module.version_odoo as u8)).as_str(),
                    );
                }
            }
            for module_depend_name in module_depends_to_add {
                models::dependency_module::add(
                    &mut conn,
                    &dep_type_module.id,
                    module_depend_name,
                    &new_module.id,
                )
                .unwrap();
            }

            // Add python deps.
            let module_depends_python = models::dependency_module::get_names(
                &mut conn,
                &new_module.id,
                &dep_type_python.id,
            );
            let module_depends_python_to_remove: Vec<&String> = module_depends_python
                .iter()
                .filter(|item| !manifest.external_depends_python.contains(item))
                .collect();
            let module_depends_python_to_add: Vec<&String> = manifest
                .external_depends_python
                .iter()
                .filter(|item| !module_depends_python.contains(&item.to_string()))
                .collect();
            for module_depends_python_name in module_depends_python_to_remove {
                let module_depend_python_id_opt = models::dependency::get_by_name(
                    &mut conn,
                    &dep_type_python.id,
                    module_depends_python_name,
                );
                if let Some(module_depend_id) = module_depend_python_id_opt {
                    let _ = models::dependency_module::delete_by_module_id_dependency_id(
                        &mut conn,
                        &new_module.id,
                        &module_depend_id.id,
                    );
                    let _ = models::system_event::register_delete_module_dependency(
                        &mut conn,
                        &module_depend_id.name,
                        "Python",
                        &new_module.technical_name,
                        &new_module.name,
                        odoo_version_u8_to_string(&(new_module.version_odoo as u8)).as_str(),
                    );
                }
            }
            for module_depends_python_name in module_depends_python_to_add {
                let dep_mod = models::dependency_module::add(
                    &mut conn,
                    &dep_type_python.id,
                    module_depends_python_name,
                    &new_module.id,
                )
                .unwrap();
                // Check OSV
                if module_depends_python_name.contains("==")
                    || module_depends_python_name.contains("<")
                {
                    let Some(caps) = re.captures(module_depends_python_name) else {
                        log::warn!(
                            "Can't parse python dependency '{module_depends_python_name}'. Skipping OSV check..."
                        );
                        continue;
                    };
                    let package_name = caps
                        .get(1)
                        .map_or(String::new(), |m| m.as_str().trim().to_string());
                    let mut package_ver = caps
                        .get(2)
                        .map_or(String::new(), |m| m.as_str().trim().to_string());
                    if !module_depends_python_name.contains("<=")
                        && module_depends_python_name.contains("<")
                    {
                        // A PyPI/network hiccup only skips this dep's OSV check,
                        // never the whole collector run.
                        let package_ver_opt = match pypi_client
                            .get_nearest_version(&package_name, &package_ver)
                            .await
                        {
                            Ok(res) => res,
                            Err(err) => {
                                log::warn!(
                                    "Can't query PyPI for '{package_name}': {err}. Skipping OSV check..."
                                );
                                continue;
                            }
                        };
                        let Some(nearest_ver) = package_ver_opt else {
                            log::info!(
                                "No valid release version found for '{}': '{}' ({}). Skipping...",
                                module_depends_python_name,
                                package_name,
                                package_ver
                            );
                            continue;
                        };
                        package_ver = nearest_ver;
                    }
                    let package_info = match pypi_client
                        .get_package_info(&package_name, Some(&package_ver))
                        .await
                    {
                        Ok(res) => res,
                        Err(err) => {
                            log::warn!(
                                "Can't query PyPI for '{package_name}': {err}. Skipping OSV check..."
                            );
                            continue;
                        }
                    };
                    let vulns_opt = package_info["vulnerabilities"].as_array();
                    if let Some(vulns) = vulns_opt {
                        for vuln in vulns {
                            let Some(vuln_id) = vuln["id"].as_str() else {
                                continue;
                            };
                            let fixed_in: String = vuln["fixed_in"]
                                .as_array()
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|x| x.as_str())
                                        .collect::<Vec<&str>>()
                                        .join(", ")
                                })
                                .unwrap_or_default();
                            models::dependency_osv::add(
                                &mut conn,
                                &dep_mod.id,
                                vuln_id,
                                vuln["details"].as_str().unwrap_or(""),
                                fixed_in.as_str(),
                            )
                            .unwrap();
                        }
                    }
                }
            }

            // Add bin deps.
            let module_depends_bin =
                models::dependency_module::get_names(&mut conn, &new_module.id, &dep_type_bin.id);

            let module_depends_bin_to_remove: Vec<&String> = module_depends_bin
                .iter()
                .filter(|item| !manifest.external_depends_bin.contains(item))
                .collect();
            let module_depends_bin_to_add: Vec<&String> = manifest
                .external_depends_bin
                .iter()
                .filter(|item| !module_depends_bin.contains(&item.to_string()))
                .collect();
            for module_depends_bin_name in module_depends_bin_to_remove {
                let module_depend_bin_id_opt = models::dependency::get_by_name(
                    &mut conn,
                    &dep_type_bin.id,
                    module_depends_bin_name,
                );
                if let Some(module_depend_id) = module_depend_bin_id_opt {
                    let _ = models::dependency_module::delete_by_module_id_dependency_id(
                        &mut conn,
                        &new_module.id,
                        &module_depend_id.id,
                    );
                    let _ = models::system_event::register_delete_module_dependency(
                        &mut conn,
                        &module_depend_id.name,
                        "Bin",
                        &new_module.technical_name,
                        &new_module.name,
                        odoo_version_u8_to_string(&(new_module.version_odoo as u8)).as_str(),
                    );
                }
            }
            for module_depends_bin_name in module_depends_bin_to_add {
                models::dependency_module::add(
                    &mut conn,
                    &dep_type_bin.id,
                    module_depends_bin_name,
                    &new_module.id,
                )
                .unwrap();
            }
        }

        log::info!("Removing outdated modules info...");
        for (key, value) in module_ids_by_repo {
            if !value.is_empty() {
                models::module::delete_outdated(&mut conn, &key, config.get_version_odoo(), &value)
                    .unwrap();
            }
        }
    } else {
        log::info!("Nothing to do!");
    }

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
