// Copyright Alexandre D. Díaz
//! Workflows: the collector itself only walks the sources (clones them, finds
//! every module folder, lists the tracked PRs/MRs); what happens to each
//! module (`on: module`) or PR (`on: pull_request`) is a graph of nodes, per
//! source (defined in `workflows.yaml` next to the collector config, see
//! `oghworkflow::layered`). Each node gets the item (its folder or PR and
//! what earlier nodes computed) and, for `if`, picks the next node.
//!
//! Conditions and templates are minijinja over `Ctx::view`. Node failures
//! only warn (that item's workflow stops, the job goes on): `error` is for
//! invalid config, which `validate` rejects before anything runs.
//!
//! Outward nodes (`ai`, `pr_comment`, `pr_edit`, `pr_review`, `pr_open`,
//! `apprise`) are dry-run (they
//! log what they would do) unless the source lists them in `actions`, and the
//! `workflow_action` ledger makes each run once per item.
//!
//! The per-module pipeline is a graph of nodes (root `workflows.yaml`, `default` = the old
//! pipeline). The core only walks (`run_modules`: `analyzer.module_folders`, sorted by name, + git
//! info + the stored row → `Found`), then each of the source's workflows runs from a fresh
//! `ModuleCtx`. Nodes: `if` (minijinja `compile_expression` over `ModuleCtx::view`),
//! `read_manifest`, `git_stats`, `analyze_code`, `security`, `migration_notes`, `store_module`
//! (module::add + snapshot/warnings/notes only for what was computed + version-mismatch event +
//! `sync_dependencies`), `osv`, `log` (minijinja template → `system_event::add`). Nodes compute
//! their prerequisites lazily. `validate` = types, per-type fields, refs, cycles, expressions,
//! `after` names; `Engine::new` also rejects two storing workflows on one source, an `after` the
//! source doesn't list or of another trigger, and `after` cycles, orders the list topologically
//! (stable: list order otherwise) and skips (warn) disabled ones and, recursively, what runs after
//! them. `run_all` per item: a workflow with `after` starts from a clone of its first dependency's
//! final `Ctx` (`inherit` fills the rest, first wins; `handed_on` clears `added_python` so `osv`
//! doesn't repeat) and is skipped (info) unless every dependency *finished* (`walk` → true: reached
//! an end without a node `Err`; an `if` without `else` is an end; the AI-limit `Err("")` isn't);
//! only contexts some later workflow needs are kept. `stamp` = `ANALYSIS_VERSION +
//! 1000*missing(security, migration_notes)` of the *storing* workflow (one per `Engine`;
//! `ANALYSIS_VERSION` if none stores — every workflow's `unchanged` compares with it), stored as
//! `module.analysis_version` (default == plain ANALYSIS_VERSION, so no re-analysis on upgrade).
//! Deletion follows the walk, not the store. Workflows travel as the 6th job arg (`-` = none); a
//! job loads definitions via `config::load_workflows(OGHCOLLECTOR_CONFIG)` (set by `run_job` from
//! `--config`). Node failures `warn`. `golden.rs` (test) pins the default workflow's DB output on a
//! deterministic git fixture (`GIT_CONFIG_GLOBAL=/dev/null`: the dev's gpg signing hangs commits);
//! `OGH_GOLDEN_RECORD=1` re-records — only when a change is *meant* to alter output.
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use fs_extra::dir::get_size;
use minijinja::Environment;
use regex::Regex;
use serde_json::{json, Value};
use sqlitedb::models::module::{CommitterActivity, ManifestInfo, Model as Module};
use sqlitedb::models::module_code_analysis::ModuleAnalysisInfo;
use sqlitedb::models::module_migration_note::MigrationConsiderationInfo;
use sqlitedb::models::module_security_warning::{SecurityWarningInfo, SEVERITY_ERROR};
use sqlitedb::models::pull_request::Model as PullRequest;
use sqlitedb::models::{self, pull_request_module, system_event, workflow_action};
use sqlitedb::DbSqliteConnection as SqliteConnection;

use oghutils::version::odoo_version_u8_to_string;

use crate::actions::{self, AiRequest, Forge, NewPr};
use crate::analyzer::{self, GitInfo, OGHCollectorAnalyzer};
use crate::gitclient::{git_run, PullRequestInfo, RepoInfo};
use crate::pypi::PypiClient;
use crate::{migration, review, security};

pub use oghworkflow::{validate, Node, Workflow, ACTIONS, DEFAULT};
// Jobs load workflows in layers (`config::load_workflows`); tests start from
// the built-ins.
#[cfg(test)]
pub use oghworkflow::with_built_in;

/// The analysis stamp stored on the modules a workflow analyzes
/// (`module.analysis_version`): the rules version, plus which analyses it
/// runs, so adding one later re-analyzes unchanged modules too. With both
/// (`default`) it's the plain rules version, as stored before workflows.
fn stamp(wf: &Workflow) -> i32 {
    let has = |kind: &str| wf.nodes.iter().any(|n| n.kind == kind);
    let missing = i32::from(!has("security")) | (i32::from(!has("migration_notes")) << 1);
    analyzer::ANALYSIS_VERSION + 1000 * missing
}

/// The workflows one job runs, in order.
pub struct Engine {
    workflows: Vec<(String, Workflow)>,
    /// The storing workflow's stamp (`ANALYSIS_VERSION` when none stores):
    /// every workflow's `unchanged` means "the stored data is current".
    stamp: i32,
}

impl Engine {
    /// `names` of `defs` (see `with_built_in`), validated. One source can
    /// store modules with one workflow only: two would keep overwriting each
    /// other's analysis stamp.
    pub fn new(names: &[String], defs: &HashMap<String, Workflow>) -> Result<Engine, String> {
        let mut listed: Vec<(&String, &Workflow)> = Vec::with_capacity(names.len());
        for name in names {
            let wf = defs
                .get(name)
                .ok_or_else(|| format!("unknown workflow '{name}'"))?;
            validate(name, wf)?;
            if listed.iter().any(|(n, _)| *n == name) {
                return Err(format!("workflow '{name}' listed twice"));
            }
            listed.push((name, wf));
        }
        // `after` is per module or PR: the same trigger, run by this source.
        for (name, wf) in &listed {
            for dep in wf.after.iter().flatten() {
                let Some((_, dep_wf)) = listed.iter().find(|(n, _)| *n == dep) else {
                    return Err(format!(
                        "workflow '{name}' runs after '{dep}', which this source doesn't run: \
                         add it to its `workflows:`"
                    ));
                };
                if dep_wf.on != wf.on {
                    return Err(format!(
                        "workflow '{name}' (on: {}) can't run after '{dep}' (on: {}): `after` \
                         is per module or per PR, and PR workflows already run after the \
                         module ones",
                        wf.on, dep_wf.on
                    ));
                }
            }
        }
        // The listed order, but each after what it names.
        let mut ordered: Vec<(&String, &Workflow)> = Vec::with_capacity(listed.len());
        while !listed.is_empty() {
            let ready = listed.iter().position(|(_, wf)| {
                wf.after
                    .iter()
                    .flatten()
                    .all(|dep| ordered.iter().any(|(n, _)| *n == dep))
            });
            let Some(at) = ready else {
                let stuck: Vec<&str> = listed.iter().map(|(n, _)| n.as_str()).collect();
                return Err(format!(
                    "workflows {}: their `after` go round in a circle",
                    stuck.join(", ")
                ));
            };
            ordered.push(listed.remove(at));
        }
        // Off, or after one that doesn't run: skipped.
        let mut workflows: Vec<(String, Workflow)> = Vec::with_capacity(ordered.len());
        for (name, wf) in ordered {
            if !wf.is_enabled() {
                log::warn!("Workflow '{name}' is disabled (enabled: false): skipped");
                continue;
            }
            if let Some(dep) = wf
                .after
                .iter()
                .flatten()
                .find(|dep| !workflows.iter().any(|(n, _)| n == *dep))
            {
                log::warn!("Workflow '{name}' runs after '{dep}', which doesn't run: skipped");
                continue;
            }
            workflows.push((name.clone(), wf.clone()));
        }
        let storing: Vec<&Workflow> = workflows
            .iter()
            .map(|(_, wf)| wf)
            .filter(|wf| wf.nodes.iter().any(|n| n.kind == "store_module"))
            .collect();
        if storing.len() > 1 {
            return Err(format!(
                "workflows {}: only one of them can have a `store_module` node",
                names.join(", ")
            ));
        }
        Ok(Engine {
            stamp: storing
                .first()
                .map_or(analyzer::ANALYSIS_VERSION, |wf| stamp(wf)),
            workflows,
        })
    }

    /// Whether any workflow runs on `trigger`.
    pub fn runs(&self, trigger: &str) -> bool {
        self.workflows.iter().any(|(_, wf)| wf.on == trigger)
    }

    /// Runs the `module` workflows over one module.
    pub async fn run_module(
        &self,
        found: &Found<'_>,
        conn: &mut SqliteConnection,
        deps: &Deps<'_>,
    ) {
        let fresh = || Ctx::Module(Box::new(ModuleCtx::new(found, self.stamp)));
        self.run_all("module", fresh, conn, deps).await;
    }

    /// Runs the `pull_request` workflows over one tracked PR/MR.
    pub async fn run_pull_request(
        &self,
        found: &FoundPr<'_>,
        conn: &mut SqliteConnection,
        deps: &Deps<'_>,
    ) {
        let fresh = || {
            Ctx::Pr(PrCtx {
                found,
                ai: BTreeMap::new(),
            })
        };
        self.run_all("pull_request", fresh, conn, deps).await;
    }

    /// Runs the `trigger` workflows over one item, in order: each from a
    /// clean context, or, one with `after`, from what those left (the first
    /// listed wins where two computed the same) and only if they finished.
    async fn run_all<'f, 'a: 'f>(
        &self,
        trigger: &str,
        fresh: impl Fn() -> Ctx<'f, 'a>,
        conn: &mut SqliteConnection,
        deps: &Deps<'_>,
    ) {
        // What finished workflows left, kept for those that run after them.
        let mut left: HashMap<&str, Ctx<'f, 'a>> = HashMap::new();
        for (name, wf) in self.workflows.iter().filter(|(_, wf)| wf.on == trigger) {
            let after = wf.after.as_deref().unwrap_or_default();
            if let Some(dep) = after.iter().find(|dep| !left.contains_key(dep.as_str())) {
                // It already said why (or stopped at a filter: then it finished).
                log::info!(
                    "Workflow '{name}' skipped for {}: '{dep}' didn't finish",
                    fresh().label()
                );
                continue;
            }
            let mut ctx = match after.split_first() {
                None => fresh(),
                Some((first, rest)) => {
                    let mut ctx = left[first.as_str()].clone();
                    for dep in rest {
                        ctx.inherit(&left[dep.as_str()]);
                    }
                    ctx.handed_on()
                }
            };
            let needed = self
                .workflows
                .iter()
                .any(|(_, w)| w.after.iter().flatten().any(|dep| dep == name));
            if self.walk(name, wf, &mut ctx, conn, deps).await && needed {
                left.insert(name, ctx);
            }
        }
    }

    /// Runs a workflow over an item; whether it finished (reached an end
    /// without a node failing; stopping at an `if` without `else` is an end).
    async fn walk(
        &self,
        name: &str,
        wf: &Workflow,
        ctx: &mut Ctx<'_, '_>,
        conn: &mut SqliteConnection,
        deps: &Deps<'_>,
    ) -> bool {
        let mut current = Some(&wf.nodes[0]);
        while let Some(node) = current {
            let next = match self.step(name, node, ctx, conn, deps).await {
                Ok(next) => next,
                Err(err) => {
                    // An empty reason was already logged once for the job.
                    if !err.is_empty() {
                        log::warn!(
                            "Workflow '{name}' stopped at '{}' for {}: {err}",
                            node.id,
                            ctx.label()
                        );
                    }
                    return false;
                }
            };
            current = next.and_then(|id| wf.nodes.iter().find(|n| n.id == id));
        }
        true
    }

    /// Runs a node; the id of the node to go to next.
    async fn step<'n>(
        &self,
        workflow: &str,
        node: &'n Node,
        ctx: &mut Ctx<'_, '_>,
        conn: &mut SqliteConnection,
        deps: &Deps<'_>,
    ) -> Result<Option<&'n str>, String> {
        let analyzer = deps.analyzer;
        let render = |ctx: &Ctx, src: &Option<String>| -> Result<String, String> {
            let src = src.as_deref().unwrap_or_default();
            ctx.env()
                .render_str(src, ctx.view())
                .map_err(|err| format!("template: {err:#}"))
        };
        match node.kind.as_str() {
            "if" => {
                let when = node.when.as_deref().unwrap_or_default();
                let env = ctx.env();
                let holds = env
                    .compile_expression(when)
                    .and_then(|expr| expr.eval(ctx.view()))
                    .map_err(|err| format!("condition: {err:#}"))?
                    .is_true();
                return Ok(if holds { &node.then } else { &node.otherwise }.as_deref());
            }
            "read_manifest" => {
                ctx.module().manifest(analyzer)?;
            }
            "git_stats" => {
                let ctx = ctx.module();
                let path = &ctx.found.path;
                ctx.committers = Some(analyzer.get_git_committers(path).unwrap_or_default());
                ctx.folder_size = Some(get_size(path).unwrap_or(0));
            }
            "analyze_code" => {
                ctx.module().analysis(analyzer);
            }
            "security" => {
                let ctx = ctx.module();
                let (found, category) = (ctx.found, ctx.manifest(analyzer)?.category.clone());
                let analysis = ctx.analysis(analyzer);
                ctx.security = Some(security::analyze_module(
                    &found.technical_name,
                    &category,
                    analysis,
                ));
            }
            "migration_notes" => {
                let ctx = ctx.module();
                let version = ctx.found.version_odoo;
                ctx.notes = Some(migration::analyze_module(version, ctx.analysis(analyzer)));
            }
            "store_module" => store_module(ctx.module(), conn, analyzer)?,
            "osv" => check_osv(ctx.module(), conn, deps.pypi).await,
            "log" => {
                let message = render(ctx, &node.message)?;
                let event = node.event.as_deref().unwrap_or("workflow");
                let severity = node.severity.as_deref().unwrap_or("info");
                system_event::add(conn, event, severity, &message).map_err(|e| e.to_string())?;
            }
            "ai" => {
                let answer = ask_ai(
                    workflow,
                    node,
                    ctx,
                    conn,
                    deps,
                    render(ctx, &node.prompt)?,
                    node.system
                        .as_ref()
                        .map(|_| render(ctx, &node.system))
                        .transpose()?,
                )
                .await?;
                ctx.ai().insert(node.id.clone(), answer);
            }
            "pr_comment" | "pr_edit" => {
                let body = render(ctx, &node.body)?;
                let comment = node.kind == "pr_comment";
                let what = if comment {
                    "comment on"
                } else {
                    "rewrite the description of"
                };
                let key = format!("{}:{workflow}/{}", ctx.item(), node.id);
                let found = ctx.pr().found;
                let (full_path, number) = (found.repo.get_full_path(), found.pr.prid);
                act(
                    conn,
                    deps,
                    &node.kind,
                    &key,
                    &format!(
                        "Workflow '{workflow}' ({}): {what} {}",
                        node.id,
                        ctx.label()
                    ),
                    &body,
                    async {
                        match comment {
                            true => deps.forge.comment(full_path, number, &body).await,
                            false => deps
                                .forge
                                .edit_description(full_path, number, &body)
                                .await
                                .map(|()| String::new()),
                        }
                    },
                )
                .await?;
            }
            "pr_review" => {
                let patch = actions::extract_patch(&render(ctx, &node.patch)?);
                let body = render(ctx, &node.body)?;
                if patch.trim().is_empty() && body.trim().is_empty() {
                    // Nothing to say (an AI found nothing): not recorded, a
                    // later head may have something.
                    return Ok(node.next.as_deref());
                }
                let found = ctx.pr().found;
                let head = found.pr.head_sha.as_deref().ok_or("the PR has no head")?;
                let plan =
                    review::plan_on(found.repo.get_clone_path(), found.branch, head, &patch)?;
                let (full_path, number) = (found.repo.get_full_path(), found.pr.prid);
                act(
                    conn,
                    deps,
                    &node.kind,
                    &format!("{}:{workflow}/{}", ctx.item(), node.id),
                    &format!(
                        "Workflow '{workflow}' ({}): review {}",
                        node.id,
                        ctx.label()
                    ),
                    &plan.describe(&body),
                    deps.forge.review(full_path, number, head, &body, &plan),
                )
                .await?;
            }
            "pr_open" => {
                let patch = actions::extract_patch(&render(ctx, &node.patch)?);
                if patch.trim().is_empty() {
                    // Nothing to change (an AI found nothing): not recorded, a
                    // later run may have something.
                    return Ok(node.next.as_deref());
                }
                let (title, body) = (render(ctx, &node.title)?, render(ctx, &node.body)?);
                let f = ctx.module().found;
                let version = odoo_version_u8_to_string(&f.version_odoo);
                // Once per module and version: a new commit isn't a reason for
                // another PR. Fixed `ogh/` branch, never an existing one.
                // ponytail: delete its `workflow_action` row to allow another.
                let branch = format!("ogh/{workflow}/{}-{version}", f.technical_name);
                let clone = f.repo.get_clone_path();
                git_run(clone, &["check-ref-format", "--branch", &branch])
                    .map_err(|err| format!("bad branch '{branch}': {err}"))?;
                let key = format!(
                    "module:{}/{}/{}@{version}:{workflow}/{}",
                    f.repo.get_org(),
                    f.repo.get_name(),
                    f.technical_name,
                    node.id
                );
                let pr = NewPr {
                    clone,
                    repo_url: f.repo.get_url(),
                    full_path: f.repo.get_full_path(),
                    base: deps.branch,
                    dir: &f.source_path,
                    branch: &branch,
                    title: &title,
                    body: &body,
                    patch: &patch,
                };
                let what = format!(
                    "Workflow '{workflow}' ({}): open a PR for {}",
                    node.id,
                    ctx.label()
                );
                let live = deps.live("pr_open");
                if workflow_action::get(conn, &key, !live).is_none() {
                    // Dry-run checks the patch too: what it says is worth reading.
                    let outcome = actions::open_pr(deps.forge, deps.workdir, &pr, live).await?;
                    record(conn, deps, &key, live, &what, &outcome);
                }
            }
            "apprise" => {
                let (title, body) = (render(ctx, &node.title)?, render(ctx, &node.body)?);
                let url = node.url.as_deref().unwrap_or_default();
                let kind = match node.severity.as_deref().unwrap_or("info") {
                    "error" => "failure",
                    severity => severity,
                };
                let what = format!(
                    "Workflow '{workflow}' ({}): notify {} about {}",
                    node.id,
                    actions::host(url),
                    ctx.label()
                );
                let key = format!("{}:{workflow}/{}", ctx.item(), node.id);
                if deps.live("apprise") && workflow_action::get(conn, &key, false).is_none() {
                    let left = deps.notifications_left.get();
                    if left == 0 {
                        // Skipped, not recorded (a later run may send it), and
                        // the workflow goes on: nothing reads a notification.
                        if !deps.notify_limit_warned.replace(true) {
                            log::warn!("No notifications left for this job ({NOTIFY_LIMIT}): the rest are skipped");
                        }
                        return Ok(node.next.as_deref());
                    }
                    deps.notifications_left.set(left - 1);
                }
                let send = async {
                    let urls = match node.api_key_env.as_deref() {
                        Some(env) => Some(
                            crate::config::read_secret(&env.to_lowercase(), env)
                                .ok_or_else(|| format!("{env} isn't set"))?,
                        ),
                        None => None,
                    };
                    actions::notify(url, urls, &title, &body, kind).await
                };
                act(conn, deps, &node.kind, &key, &what, &body, send).await?;
            }
            kind => unreachable!("node type '{kind}' passed validation"),
        }
        Ok(node.next.as_deref())
    }
}

/// Records an outward action and logs it (live: done; dry-run: would do).
/// Past `DRY_RUN_EVENTS` dry-run reports in a job, the rest are left for
/// later runs (not recorded) instead of burying `/logs`.
fn record(
    conn: &mut SqliteConnection,
    deps: &Deps,
    key: &str,
    live: bool,
    what: &str,
    outcome: &str,
) {
    if !live {
        if deps.dry_runs_left.get() == 0 {
            deps.dry_runs_held.set(deps.dry_runs_held.get() + 1);
            return;
        }
        deps.dry_runs_left.set(deps.dry_runs_left.get() - 1);
    }
    let _ = workflow_action::record(conn, key, !live, Some(outcome));
    let (message, severity) = match live {
        true if outcome.is_empty() => (what.to_string(), "success"),
        true => (format!("{what}: {outcome}"), "success"),
        false => (format!("[dry-run] {what}: {}", preview(outcome)), "info"),
    };
    let _ = system_event::add(conn, "workflow", severity, &message);
}

/// The start of a text, for logs.
fn preview(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(300) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    }
}

/// Runs an outward action once per `key`: for real when the source lists
/// `kind` in `actions`, else only logs it (once) with what it'd send. A
/// failure isn't recorded, so the next run retries.
async fn act(
    conn: &mut SqliteConnection,
    deps: &Deps<'_>,
    kind: &str,
    key: &str,
    what: &str,
    sends: &str,
    live_action: impl std::future::Future<Output = Result<String, String>>,
) -> Result<(), String> {
    let live = deps.live(kind);
    if workflow_action::get(conn, key, !live).is_some() {
        return Ok(());
    }
    let outcome = match live {
        true => live_action.await?,
        false => sends.to_string(),
    };
    record(conn, deps, key, live, what, &outcome);
    Ok(())
}

/// An `ai` node: the answer, asked once per item, prompt and settings (the
/// ledger keeps it: an unchanged item never pays twice). Dry-run: logs what
/// it would ask, once, and answers "".
async fn ask_ai(
    workflow: &str,
    node: &Node,
    ctx: &Ctx<'_, '_>,
    conn: &mut SqliteConnection,
    deps: &Deps<'_>,
    prompt: String,
    system: Option<String>,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let req = AiRequest {
        provider: node.provider.as_deref().unwrap_or_default(),
        model: node.model.as_deref().unwrap_or_default(),
        url: node.url.as_deref(),
        key: node
            .api_key_env
            .as_deref()
            .and_then(|env| crate::config::read_secret(&env.to_lowercase(), env)),
        system,
        prompt,
        max_tokens: node.max_tokens.unwrap_or(16000),
        timeout: node.timeout.unwrap_or(300),
    };
    let digest = Sha256::digest(format!(
        "{}\0{}\0{}\0{}\0{}\0{}",
        req.provider,
        req.model,
        req.base_url(),
        req.max_tokens,
        req.system.as_deref().unwrap_or_default(),
        req.prompt
    ));
    let digest: String = digest.iter().take(12).map(|b| format!("{b:02x}")).collect();
    let key = format!("{}:{workflow}/{}:{digest}", ctx.item(), node.id);
    let what = format!(
        "Workflow '{workflow}' ({}): ask {} at {} about {}",
        node.id,
        req.model,
        req.host(),
        ctx.label()
    );
    if !deps.live("ai") {
        if workflow_action::get(conn, &key, true).is_none() {
            record(
                conn,
                deps,
                &key,
                false,
                &what,
                &format!("{} chars of prompt", req.prompt.len()),
            );
        }
        return Ok(String::new());
    }
    if let Some(Some(answer)) = workflow_action::get(conn, &key, false) {
        return Ok(answer);
    }
    let left = deps.ai_calls_left.get();
    if left == 0 {
        // Once per job; the items left wait for the next run.
        if !deps.ai_limit_warned.replace(true) {
            log::warn!("No AI calls left for this job (OGHCOLLECTOR_AI_LIMIT): the rest wait for the next run");
        }
        return Err(String::new());
    }
    deps.ai_calls_left.set(left - 1);
    let answer = actions::ask(&req).await?;
    let _ = workflow_action::record(conn, &key, false, Some(&answer));
    Ok(answer)
}

/// What a node needs besides the item.
pub struct Deps<'a> {
    pub analyzer: &'a OGHCollectorAnalyzer,
    pub pypi: &'a PypiClient,
    pub forge: &'a Forge,
    /// Outward node types the source lets act for real (see `ACTIONS`).
    pub actions: &'a [String],
    /// The job's branch (= Odoo version), what `pr_open` targets.
    pub branch: &'a str,
    /// The job's own work directory (`pr_open` worktrees).
    pub workdir: &'a Path,
    /// AI calls this job may still make (`OGHCOLLECTOR_AI_LIMIT`).
    pub ai_calls_left: Cell<usize>,
    pub ai_limit_warned: Cell<bool>,
    /// Notifications this job may still send (`NOTIFY_LIMIT`).
    pub notifications_left: Cell<usize>,
    pub notify_limit_warned: Cell<bool>,
    /// Dry-run reports this job may still log, and those held back.
    pub dry_runs_left: Cell<usize>,
    pub dry_runs_held: Cell<usize>,
}

/// Dry-run reports one job logs at most.
pub const DRY_RUN_EVENTS: usize = 20;
/// Notifications one job sends at most: a first run over a big source would
/// otherwise flood the channel (and the job's time, one HTTP call each).
/// ponytail: a constant; an env var like `OGHCOLLECTOR_AI_LIMIT` if it needs tuning.
pub const NOTIFY_LIMIT: usize = 20;

impl<'a> Deps<'a> {
    fn live(&self, kind: &str) -> bool {
        self.actions.iter().any(|a| a == kind)
    }

    /// One line for the dry-run reports this job held back, if any.
    pub fn log_held_dry_runs(&self, conn: &mut SqliteConnection) {
        let held = self.dry_runs_held.get();
        if held > 0 {
            let message = format!(
                "[dry-run] {held} more outward workflow action(s) not reported here: later runs report them (or list them in the source's `actions`)"
            );
            let _ = system_event::add(conn, "workflow", "info", &message);
        }
    }

    /// Everything dry-run unless `actions` says otherwise.
    #[cfg(test)]
    pub fn for_tests(
        analyzer: &'a OGHCollectorAnalyzer,
        pypi: &'a PypiClient,
        forge: &'a Forge,
        workdir: &'a Path,
        actions: &'a [String],
    ) -> Deps<'a> {
        Deps {
            analyzer,
            pypi,
            forge,
            actions,
            branch: "18.0",
            workdir,
            ai_calls_left: Cell::new(20),
            ai_limit_warned: Cell::new(false),
            notifications_left: Cell::new(NOTIFY_LIMIT),
            notify_limit_warned: Cell::new(false),
            dry_runs_left: Cell::new(DRY_RUN_EVENTS),
            dry_runs_held: Cell::new(0),
        }
    }
}

/// A module folder the walk found.
pub struct Found<'a> {
    pub repo: &'a RepoInfo,
    pub path: PathBuf,
    pub technical_name: String,
    pub manifest_file: String,
    /// Repo-relative folder.
    pub source_path: String,
    pub version_odoo: u8,
    pub git: GitInfo,
    /// Its row from before this run.
    pub stored: Option<Module>,
}

/// One workflow's view of a module: what the walk found plus what its nodes
/// computed so far.
#[derive(Clone)]
pub struct ModuleCtx<'f, 'a> {
    found: &'f Found<'a>,
    stamp: i32,
    /// Same last commit and the same analyses (stamp) as stored.
    unchanged: bool,
    manifest: Option<ManifestInfo>,
    committers: Option<HashMap<String, CommitterActivity>>,
    folder_size: Option<u64>,
    analysis: Option<ModuleAnalysisInfo>,
    security: Option<Vec<SecurityWarningInfo>>,
    notes: Option<Vec<MigrationConsiderationInfo>>,
    /// Python dependencies `store_module` added (`dependency_module` id).
    added_python: Vec<(i64, String)>,
    /// `ai` node answers, by node id.
    ai: BTreeMap<String, String>,
}

impl<'f, 'a> ModuleCtx<'f, 'a> {
    fn new(found: &'f Found<'a>, stamp: i32) -> Self {
        let force = std::env::var_os("OGHCOLLECTOR_FORCE_REANALYZE").is_some();
        let hash = &found.git.last_commit_hash;
        let unchanged = !force
            && !hash.is_empty()
            && found
                .stored
                .as_ref()
                .is_some_and(|m| &m.last_commit_hash == hash && m.analysis_version == stamp);
        ModuleCtx {
            found,
            stamp,
            unchanged,
            manifest: None,
            committers: None,
            folder_size: None,
            analysis: None,
            security: None,
            notes: None,
            added_python: Vec::new(),
            ai: BTreeMap::new(),
        }
    }

    /// What `other` (another workflow's, on this module) computed and this
    /// one hasn't.
    fn inherit(&mut self, other: &ModuleCtx) {
        fn fill<T: Clone>(mine: &mut Option<T>, theirs: &Option<T>) {
            if mine.is_none() {
                mine.clone_from(theirs);
            }
        }
        fill(&mut self.manifest, &other.manifest);
        fill(&mut self.committers, &other.committers);
        fill(&mut self.folder_size, &other.folder_size);
        fill(&mut self.analysis, &other.analysis);
        fill(&mut self.security, &other.security);
        fill(&mut self.notes, &other.notes);
        for (id, answer) in &other.ai {
            self.ai.entry(id.clone()).or_insert_with(|| answer.clone());
        }
    }

    /// The manifest, read the first time it's needed.
    fn manifest(&mut self, analyzer: &OGHCollectorAnalyzer) -> Result<&ManifestInfo, String> {
        if self.manifest.is_none() {
            let f = self.found;
            let path = format!("{}/{}", f.path.display(), f.manifest_file);
            let manifest = analyzer
                .read_manifest(
                    f.repo.get_org(),
                    f.repo.get_name(),
                    &f.technical_name,
                    &path,
                )
                .map_err(|err| format!("can't read manifest '{path}': {err}"))?;
            self.manifest = Some(manifest);
        }
        Ok(self.manifest.as_ref().expect("just read"))
    }

    /// The code analysis, run the first time it's needed.
    fn analysis(&mut self, analyzer: &OGHCollectorAnalyzer) -> &ModuleAnalysisInfo {
        let path = &self.found.path;
        self.analysis
            .get_or_insert_with(|| analyzer.analyze_module_source(path))
    }

    /// What conditions and templates see. Sections no node computed are null.
    fn json(&self) -> Value {
        let f = self.found;
        let git = &f.git;
        json!({
            "technical_name": f.technical_name,
            "path": f.path,
            "source_path": f.source_path,
            "org": f.repo.get_org(),
            "repo": f.repo.get_name(),
            "odoo_version": odoo_version_u8_to_string(&f.version_odoo),
            "is_new": f.stored.is_none(),
            "unchanged": self.unchanged,
            "last_commit": {
                "hash": git.last_commit_hash,
                "author": git.last_commit_author,
                "date": git.last_commit_date,
                "name": git.last_commit_name,
            },
            "manifest": self.manifest.as_ref().map(|m| json!({
                "name": m.name,
                "version": m.version_module,
                "license": m.license,
                "category": m.category,
                "author": m.author,
                "installable": m.installable,
                "depends": m.depends,
            })),
            "security": self.security.as_ref().map(|w| {
                let grave = w.iter().filter(|w| w.severity == SEVERITY_ERROR).count();
                json!({"grave": grave, "minor": w.len() - grave, "findings": w})
            }),
            "migration": self.notes.as_ref().map(|n| json!({"count": n.len(), "notes": n})),
        })
    }
}

/// A tracked PR/MR, as the PR step of a job left it.
pub struct FoundPr<'a> {
    pub repo: &'a RepoInfo,
    pub version_odoo: u8,
    /// Base branch (the job's).
    pub branch: &'a str,
    pub info: &'a PullRequestInfo,
    /// Its row now, and from before this run (`None`: first seen).
    pub pr: PullRequest,
    pub before: Option<PullRequest>,
    /// The modules it changes, with their analysis (`full` sources).
    pub modules: Vec<pull_request_module::Model>,
}

/// One workflow's view of a PR/MR.
#[derive(Clone)]
pub struct PrCtx<'f, 'a> {
    found: &'f FoundPr<'a>,
    ai: BTreeMap<String, String>,
}

impl PrCtx<'_, '_> {
    fn json(&self) -> Value {
        let f = self.found;
        let (pr, info) = (&f.pr, f.info);
        let ci_changed = f
            .before
            .as_ref()
            .is_some_and(|b| b.ci_status != pr.ci_status)
            && matches!(pr.ci_status.as_deref(), Some("success" | "failure"));
        let analyses: Vec<(&str, Option<Value>)> = f
            .modules
            .iter()
            .map(|m| {
                let analysis = m
                    .analysis
                    .as_deref()
                    .and_then(|a| serde_json::from_str(a).ok());
                (m.module_technical_name.as_str(), analysis)
            })
            .collect();
        let (security, migration) = pr_problems(&analyses);
        let modules: Vec<Value> = f
            .modules
            .iter()
            .zip(&analyses)
            .map(|(m, (_, analysis))| {
                json!({"name": m.module_technical_name, "port": m.is_new, "analysis": analysis})
            })
            .collect();
        json!({
            "org": f.repo.get_org(),
            "repo": f.repo.get_name(),
            "odoo_version": odoo_version_u8_to_string(&f.version_odoo),
            "pr": {
                "number": pr.prid,
                "title": pr.name,
                "author": pr.author,
                "labels": pr.label_names(),
                "draft": pr.is_draft,
                "ci_status": pr.ci_status,
                "head_sha": pr.head_sha,
                "head_ref": info.head_ref,
                "url": oghutils::git::pull_request_url(f.repo.get_url(), pr.prid),
                "description": info.description,
                "is_new": f.before.is_none(),
                "head_changed": f.before.as_ref().is_none_or(|b| b.head_sha != pr.head_sha),
                "ci_changed": ci_changed,
                // Opened by `pr_open`: its webhooks come back here.
                "is_ours": info.head_ref.starts_with("ogh/"),
                // `modules[].analysis`, `security` and `migration` are of this
                // head: false while it waits for its analysis (a new push past
                // the job's analysis limit, a failed fetch, a `migration`
                // source), when they're an older head's or none.
                "analyzed": pr.head_sha.is_some()
                    && pr.analyzed_sha == pr.head_sha
                    && pr.analysis_version == analyzer::ANALYSIS_VERSION,
            },
            "modules": modules,
            "security": security,
            "migration": migration,
        })
    }
}

/// A PR's problems over its analyzed modules, as a module workflow sees its
/// own (`security.grave|minor|findings`, `migration.count|notes`): the
/// findings it brings (not in the merged module) and its modules' migration
/// notes, each tagged with its `module`.
fn pr_problems(analyses: &[(&str, Option<Value>)]) -> (Value, Value) {
    let (mut findings, mut notes) = (Vec::new(), Vec::new());
    for (module, analysis) in analyses {
        let Some(analysis) = analysis else { continue };
        let tagged = |item: &Value| {
            let mut item = item.clone();
            item["module"] = json!(module);
            item
        };
        let list = |key: &str| analysis[key].as_array().cloned().unwrap_or_default();
        findings.extend(
            list("security")
                .iter()
                .filter(|w| w["new"] == true)
                .map(tagged),
        );
        notes.extend(list("migration_notes").iter().map(tagged));
    }
    let grave = findings.iter().filter(|w| w["severity"] == "error").count();
    (
        json!({"grave": grave, "minor": findings.len() - grave, "findings": findings}),
        json!({"count": notes.len(), "notes": notes}),
    )
}

/// What a workflow runs over.
#[derive(Clone)]
pub enum Ctx<'f, 'a> {
    Module(Box<ModuleCtx<'f, 'a>>),
    Pr(PrCtx<'f, 'a>),
}

impl<'f, 'a> Ctx<'f, 'a> {
    /// What `other` (another workflow's, on the same item) computed and this
    /// one hasn't (`Engine::new` keeps `after` within a trigger).
    fn inherit(&mut self, other: &Ctx) {
        match (self, other) {
            (Ctx::Module(ctx), Ctx::Module(other)) => ctx.inherit(other),
            (Ctx::Pr(ctx), Ctx::Pr(other)) => {
                for (id, answer) in &other.ai {
                    ctx.ai.entry(id.clone()).or_insert_with(|| answer.clone());
                }
            }
            _ => unreachable!("`after` across triggers"),
        }
    }

    /// A finished workflow's context, for one that runs after it: what its
    /// `store_module` added is its own (`osv` would report it again).
    fn handed_on(mut self) -> Self {
        if let Ctx::Module(ctx) = &mut self {
            ctx.added_python.clear();
        }
        self
    }

    /// `validate` keeps module nodes out of PR workflows.
    fn module(&mut self) -> &mut ModuleCtx<'f, 'a> {
        match self {
            Ctx::Module(ctx) => ctx,
            Ctx::Pr(_) => unreachable!("a module node in a pull_request workflow"),
        }
    }

    fn pr(&self) -> &PrCtx<'f, 'a> {
        match self {
            Ctx::Pr(ctx) => ctx,
            Ctx::Module(_) => unreachable!("a PR node in a module workflow"),
        }
    }

    fn ai(&mut self) -> &mut BTreeMap<String, String> {
        match self {
            Ctx::Module(ctx) => &mut ctx.ai,
            Ctx::Pr(ctx) => &mut ctx.ai,
        }
    }

    /// What conditions and templates see (`ai.<node id>`: answers so far).
    pub fn view(&self) -> minijinja::Value {
        let (mut view, ai) = match self {
            Ctx::Module(ctx) => (ctx.json(), &ctx.ai),
            Ctx::Pr(ctx) => (ctx.json(), &ctx.ai),
        };
        view["ai"] = json!(ai);
        minijinja::Value::from_serialize(&view)
    }

    /// Templates' environment: `file("rel/path")` (a file of the module) or
    /// `diff()` (the PR's changes).
    fn env(&self) -> minijinja::Environment<'static> {
        let mut env = Environment::new();
        match self {
            Ctx::Module(ctx) => {
                let dir = ctx.found.path.clone();
                env.add_function("file", move |rel: String| read_file(&dir, &rel));
            }
            Ctx::Pr(ctx) => {
                let f = ctx.found;
                let clone = f.repo.get_clone_path().to_string();
                let range = format!(
                    "origin/{}...{}",
                    f.branch,
                    f.pr.head_sha.as_deref().unwrap_or("HEAD")
                );
                env.add_function("diff", move || pr_diff(&clone, &range));
            }
        }
        env
    }

    /// The item an action acts on, for the ledger.
    fn item(&self) -> String {
        match self {
            Ctx::Module(ctx) => {
                let f = ctx.found;
                format!(
                    "module:{}/{}/{}@{}#{}",
                    f.repo.get_org(),
                    f.repo.get_name(),
                    f.technical_name,
                    odoo_version_u8_to_string(&f.version_odoo),
                    f.git.last_commit_hash
                )
            }
            Ctx::Pr(ctx) => {
                let f = ctx.found;
                let sha = f.pr.head_sha.as_deref().unwrap_or_default();
                format!(
                    "pr:{}/{}#{}@{sha}",
                    f.repo.get_org(),
                    f.repo.get_name(),
                    f.pr.prid
                )
            }
        }
    }

    /// The item, for people.
    fn label(&self) -> String {
        match self {
            Ctx::Module(ctx) => {
                let f = ctx.found;
                let version = odoo_version_u8_to_string(&f.version_odoo);
                format!(
                    "'{}' in '{}' [{version}]",
                    f.technical_name,
                    f.repo.get_name()
                )
            }
            Ctx::Pr(ctx) => {
                let f = ctx.found;
                let version = odoo_version_u8_to_string(&f.version_odoo);
                format!(
                    "PR #{} \"{}\" in '{}' [{version}]",
                    f.pr.prid,
                    f.pr.name,
                    f.repo.get_name()
                )
            }
        }
    }
}

const MAX_FILE_BYTES: u64 = 100_000;
const MAX_DIFF_CHARS: usize = 200_000;

/// `file()`: a file of the module, never outside it (`..`, absolute paths
/// and symlinks pointing away are all refused once resolved).
fn read_file(dir: &Path, rel: &str) -> Result<String, minijinja::Error> {
    let fail = |msg: String| minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, msg);
    let base = dir.canonicalize().map_err(|e| fail(e.to_string()))?;
    let target = base
        .join(rel)
        .canonicalize()
        .map_err(|e| fail(format!("file('{rel}'): {e}")))?;
    if rel.split(['/', '\\']).any(|p| p == "..") || !target.starts_with(&base) || !target.is_file()
    {
        return Err(fail(format!("file('{rel}'): not a file of the module")));
    }
    let size = target.metadata().map_err(|e| fail(e.to_string()))?.len();
    if size > MAX_FILE_BYTES {
        return Err(fail(format!(
            "file('{rel}'): {size} bytes, more than {MAX_FILE_BYTES}"
        )));
    }
    let bytes = std::fs::read(&target).map_err(|e| fail(e.to_string()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `diff()`: the PR's changes against its base. An error when they can't be
/// read (head not fetched): an empty diff would get reviewed, and recorded.
fn pr_diff(clone: &str, range: &str) -> Result<String, minijinja::Error> {
    let mut diff = git_run(clone, &["diff", "--no-renames", range]).map_err(|err| {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("diff(): {err}"),
        )
    })?;
    if let Some((at, _)) = diff.char_indices().nth(MAX_DIFF_CHARS) {
        diff.truncate(at);
        diff.push_str("\n[... truncated]");
    }
    Ok(diff)
}

/// Stores the module as the analyzer saw it, and whatever the earlier nodes
/// computed: the code analysis snapshot, security findings and migration
/// notes each only when computed (not computed: the stored ones stay), then
/// the dependencies. Logs the same events the collector always did.
fn store_module(
    ctx: &mut ModuleCtx,
    conn: &mut SqliteConnection,
    analyzer: &OGHCollectorAnalyzer,
) -> Result<(), String> {
    let manifest = ctx.manifest(analyzer)?.clone();
    let f = ctx.found;
    let odoo_ver_str = odoo_version_u8_to_string(&f.version_odoo);
    let mut info = manifest.clone();
    // Forced: some manifests carry a wrong one.
    info.version_odoo = f.version_odoo;
    info.source_path = f.source_path.clone();
    info.folder_size = ctx
        .folder_size
        .unwrap_or_else(|| f.stored.as_ref().map_or(0, |m| m.folder_size as u64));
    info.last_commit_hash = f.git.last_commit_hash.clone();
    info.last_commit_author = f.git.last_commit_author.clone();
    info.last_commit_name = f.git.last_commit_name.clone();
    info.last_commit_date = f.git.last_commit_date.clone();
    info.last_commit_partof = f.git.last_commit_partof.clone();
    info.committers = ctx.committers.clone().unwrap_or_default();
    let module = models::module::add(conn, &info).unwrap();

    if let Some(analysis) = &ctx.analysis {
        // Only this version_module's snapshot is rebuilt; earlier ones stay.
        let snapshot =
            models::module_version::get_or_create(conn, &module.id, &info.version_module).unwrap();
        let (id, version_id) = (&module.id, &snapshot.id);
        models::module_view::replace_for_module(conn, id, version_id, &analysis.views).unwrap();
        models::module_model::replace_for_module(conn, id, version_id, &analysis.models).unwrap();
        models::module_record::replace_for_module(conn, id, version_id, &analysis.records).unwrap();
        models::module_controller::replace_for_module(conn, id, version_id, &analysis.controllers)
            .unwrap();
        if let Some(warnings) = &ctx.security {
            // Every finding is stored; minor ones also leave an event, only
            // when new for this version snapshot (re-analysis doesn't re-log).
            let known: HashSet<(String, Option<String>)> =
                models::module_security_warning::get_by_module_version_id(conn, version_id)
                    .into_iter()
                    .map(|w| (w.code, w.xml_id))
                    .collect();
            for w in warnings.iter().filter(|w| {
                w.severity != SEVERITY_ERROR && !known.contains(&(w.code.clone(), w.xml_id.clone()))
            }) {
                let _ = system_event::register_security_warning(
                    conn,
                    &module.technical_name,
                    &module.name,
                    &odoo_ver_str,
                    w.xml_id.as_deref(),
                    &w.message,
                );
            }
            models::module_security_warning::replace_for_module(conn, id, version_id, warnings)
                .unwrap();
        }
        if let Some(notes) = &ctx.notes {
            models::module_migration_note::replace_for_module(conn, id, version_id, notes).unwrap();
        }
        let _ = models::module::set_analysis_version(conn, id, ctx.stamp);
    }

    if manifest.version_odoo != f.version_odoo && manifest.installable {
        let _ = system_event::register_problem_module_version(
            conn,
            &module.technical_name,
            &module.name,
            f.repo.get_name(),
            &odoo_version_u8_to_string(&manifest.version_odoo),
            &odoo_ver_str,
        );
    }

    sync_dependencies(conn, &module, "module", "Odoo", &manifest.depends);
    ctx.added_python = sync_dependencies(
        conn,
        &module,
        "python",
        "Python",
        &manifest.external_depends_python,
    );
    sync_dependencies(conn, &module, "bin", "Bin", &manifest.external_depends_bin);
    Ok(())
}

/// Makes a module's dependencies of one type the manifest's, logging what
/// goes; returns what was added (`dependency_module` id, name).
fn sync_dependencies(
    conn: &mut SqliteConnection,
    module: &Module,
    dep_type: &str,
    label: &str,
    wanted: &[String],
) -> Vec<(i64, String)> {
    let dep_type = models::dependency_type::get_by_name(conn, dep_type)
        .unwrap_or_else(|| panic!("Can't find the {dep_type} dependency type"));
    let stored = models::dependency_module::get_names(conn, &module.id, &dep_type.id);
    for name in stored.iter().filter(|name| !wanted.contains(name)) {
        let Some(dep) = models::dependency::get_by_name(conn, &dep_type.id, name) else {
            continue;
        };
        let _ =
            models::dependency_module::delete_by_module_id_dependency_id(conn, &module.id, &dep.id);
        let _ = system_event::register_delete_module_dependency(
            conn,
            &dep.name,
            label,
            &module.technical_name,
            &module.name,
            &odoo_version_u8_to_string(&(module.version_odoo as u8)),
        );
    }
    wanted
        .iter()
        .filter(|name| !stored.contains(name))
        .map(|name| {
            let dep = models::dependency_module::add(conn, &dep_type.id, name, &module.id).unwrap();
            (dep.id, name.clone())
        })
        .collect()
}

/// Known vulnerabilities (PyPI/OSV) of the pinned Python dependencies
/// `store_module` just added. A PyPI hiccup only skips that dependency.
async fn check_osv(ctx: &ModuleCtx<'_, '_>, conn: &mut SqliteConnection, pypi: &PypiClient) {
    static PIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([^><=]+).+?([^><=]+)$").unwrap());
    for (dep_id, requirement) in &ctx.added_python {
        if !requirement.contains("==") && !requirement.contains('<') {
            continue;
        }
        let Some(caps) = PIN.captures(requirement) else {
            log::warn!("Can't parse python dependency '{requirement}'. Skipping OSV check...");
            continue;
        };
        let package = caps.get(1).map_or("", |m| m.as_str()).trim().to_string();
        let mut version = caps.get(2).map_or("", |m| m.as_str()).trim().to_string();
        if !requirement.contains("<=") && requirement.contains('<') {
            match pypi.get_nearest_version(&package, &version).await {
                Ok(Some(nearest)) => version = nearest,
                Ok(None) => {
                    log::info!(
                        "No valid release version found for '{requirement}': '{package}' ({version}). Skipping..."
                    );
                    continue;
                }
                Err(err) => {
                    log::warn!("Can't query PyPI for '{package}': {err}. Skipping OSV check...");
                    continue;
                }
            }
        }
        let info = match pypi.get_package_info(&package, Some(&version)).await {
            Ok(info) => info,
            Err(err) => {
                log::warn!("Can't query PyPI for '{package}': {err}. Skipping OSV check...");
                continue;
            }
        };
        for vuln in info["vulnerabilities"].as_array().into_iter().flatten() {
            let Some(vuln_id) = vuln["id"].as_str() else {
                continue;
            };
            let fixed_in = vuln["fixed_in"]
                .as_array()
                .map(|v| {
                    v.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            models::dependency_osv::add(
                conn,
                dep_id,
                vuln_id,
                vuln["details"].as_str().unwrap_or(""),
                &fixed_in,
            )
            .unwrap();
        }
    }
}

/// The walk: every module folder of the repos' read paths, through the
/// engine. Then the stored modules of a version that the walk no longer
/// finds go (only what was walked counts, not what a workflow stored: a
/// conditional `store_module` must not delete what it skips). Returns how
/// many modules were walked.
#[allow(clippy::too_many_arguments)]
pub async fn run_modules(
    conn: &mut SqliteConnection,
    engine: &Engine,
    deps: &Deps<'_>,
    read_paths: &[String],
    repo_infos: &[RepoInfo],
    gh_repo_ids: &[i64],
    version_odoo: &u8,
) -> usize {
    log::info!("Analyzing '{}' repos...", repo_infos.len());
    let mut walked = 0;
    for (repo, gh_repo_id) in repo_infos.iter().zip(gh_repo_ids) {
        let mut kept: Vec<i64> = Vec::new();
        for (path, technical_name, manifest_file) in deps.analyzer.module_folders(repo, read_paths)
        {
            let stored = |conn: &mut SqliteConnection| {
                models::module::get_by_technical_name_odoo_version_organization_name_repository_name(
                    conn,
                    &technical_name,
                    version_odoo,
                    repo.get_org(),
                    repo.get_name(),
                )
                .into_iter()
                .next()
            };
            let found = Found {
                repo,
                source_path: path
                    .strip_prefix(repo.get_clone_path())
                    .map(|p| p.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"))
                    .unwrap_or_default(),
                git: deps.analyzer.get_git_info(&path).unwrap_or_default(),
                stored: stored(conn),
                technical_name: technical_name.clone(),
                manifest_file,
                version_odoo: *version_odoo,
                path,
            };
            engine.run_module(&found, conn, deps).await;
            walked += 1;
            kept.extend(stored(conn).map(|m| m.id));
        }
        models::module::delete_outdated(conn, gh_repo_id, version_odoo, &kept).unwrap();
    }
    walked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn parse(yaml: &str) -> Workflow {
        config::Config::builder()
            .add_source(config::File::from_str(yaml, config::FileFormat::Yaml))
            .build()
            .and_then(|c| c.try_deserialize())
            .unwrap()
    }

    #[test]
    fn one_source_stores_with_one_workflow() {
        let built_in = with_built_in(&HashMap::new());
        // Two storing workflows on one source would fight over the stamp.
        let defs = with_built_in(&HashMap::from([(
            "copy".to_string(),
            built_in[DEFAULT].clone(),
        )]));
        let names = ["default".to_string(), "copy".to_string()];
        assert!(Engine::new(&names, &defs).is_err());
    }

    /// The default workflow with a `log` probe first, printing what its
    /// `if` decides on; runs it once over the fixture.
    async fn probe_run(
        f: &crate::golden::Fixture,
        conn: &mut SqliteConnection,
        defs: &HashMap<String, Workflow>,
        names: &[&str],
    ) {
        let analyzer = OGHCollectorAnalyzer::new(&180);
        let pypi = PypiClient::new();
        let forge = Forge::with(false, "http://127.0.0.1:9", None);
        let workdir = f.dir.join("work");
        let deps = Deps::for_tests(&analyzer, &pypi, &forge, &workdir, &[]);
        let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        let engine = Engine::new(&names, defs).unwrap();
        let org = models::gh_organization::add(conn, "org").unwrap();
        let repo = models::gh_repository::add(conn, &org.id, "repo").unwrap();
        run_modules(
            conn,
            &engine,
            &deps,
            &[String::new()],
            &f.repo_infos(),
            &[repo.id],
            &180,
        )
        .await;
    }

    fn probes(conn: &mut SqliteConnection) -> Vec<String> {
        crate::golden::snapshot(conn)
            .lines()
            .filter_map(|l| l.strip_prefix("probe info "))
            .map(str::to_string)
            .collect()
    }

    #[tokio::test]
    async fn workflows_run_after_the_ones_they_name_and_start_from_them() {
        let f = crate::golden::Fixture::new("after");
        crate::golden::first_commit(&f);
        let pool = f.db();
        let conn = &mut pool.get().unwrap();
        let wf = |yaml: &str| parse(&format!("on: module\n{yaml}"));
        let probe = |text: &str| {
            format!("[{{id: p, type: log, event: probe, message: '{text} {{{{ technical_name }}}}: {{{{ migration.count }}}}'}}]")
        };
        let defs = with_built_in(&HashMap::from([
            (
                "notes".into(),
                wf("nodes: [{id: n, type: migration_notes}]"),
            ),
            (
                "report".into(),
                wf(&format!("after: [notes]\nnodes: {}", probe("after"))),
            ),
            ("alone".into(), wf(&format!("nodes: {}", probe("alone")))),
            // Fails (no such file): what runs after it doesn't.
            (
                "broken".into(),
                wf("nodes: [{id: b, type: log, message: \"{{ file('nope') }}\"}]"),
            ),
            (
                "after_broken".into(),
                wf(&format!("after: [broken]\nnodes: {}", probe("never"))),
            ),
            // Stops at a filter: that's an end, it finished.
            (
                "filter".into(),
                wf("nodes: [{id: f, type: if, when: 'false', then: o}, {id: o, type: osv}]"),
            ),
            (
                "after_filter".into(),
                wf(&format!("after: [filter]\nnodes: {}", probe("filtered"))),
            ),
        ]));
        // Listed before what it runs after: it still comes second, and sees
        // the notes `notes` computed; `alone` starts from nothing.
        let names = [
            "report",
            "alone",
            "notes",
            "after_broken",
            "broken",
            "after_filter",
            "filter",
        ];
        probe_run(&f, conn, &defs, &names).await;
        let got = probes(conn);
        assert!(got.contains(&"after mod_a: 1".to_string()), "{got:?}");
        assert!(got.contains(&"after mod_b: 0".to_string()), "{got:?}");
        assert!(
            got.iter()
                .any(|p| p.starts_with("alone mod_a:") && !p.ends_with('1')),
            "{got:?}"
        );
        assert!(
            got.iter().any(|p| p.starts_with("filtered mod_a")),
            "{got:?}"
        );
        assert!(!got.iter().any(|p| p.starts_with("never")), "{got:?}");
    }

    #[tokio::test]
    async fn a_dependent_computes_what_its_dependency_skipped() {
        let f = crate::golden::Fixture::new("after-unchanged");
        crate::golden::first_commit(&f);
        let pool = f.db();
        let conn = &mut pool.get().unwrap();
        // On an unchanged module `default` only stores the manifest: a node
        // that needs the findings computes them then, and reuses them else.
        let grave = parse(
            "on: module\nafter: [default]\nnodes: [{id: s, type: security, next: p}, \
             {id: p, type: log, event: probe, message: '{{ technical_name }} {{ unchanged }} {{ security.grave }}'}]",
        );
        let defs = with_built_in(&HashMap::from([("grave".to_string(), grave)]));
        probe_run(&f, conn, &defs, &["grave", "default"]).await;
        probe_run(&f, conn, &defs, &["grave", "default"]).await;
        let got = probes(conn);
        assert!(got.contains(&"mod_a false 1".to_string()), "{got:?}");
        assert!(got.contains(&"mod_a true 1".to_string()), "{got:?}");
    }

    #[test]
    fn after_stays_within_a_source_and_a_trigger() {
        let wf = |yaml: &str| parse(yaml);
        let defs = with_built_in(&HashMap::from([
            (
                "mine".into(),
                wf("on: module\nafter: [default]\nnodes: [{id: o, type: osv}]"),
            ),
            (
                "pr".into(),
                wf("on: pull_request\nafter: [default]\nnodes: [{id: l, type: log, message: x}]"),
            ),
            (
                "a".into(),
                wf("on: module\nafter: [b]\nnodes: [{id: o, type: osv}]"),
            ),
            (
                "b".into(),
                wf("on: module\nafter: [a]\nnodes: [{id: o, type: osv}]"),
            ),
            (
                "off".into(),
                wf("on: module\nenabled: false\nnodes: [{id: o, type: osv}]"),
            ),
            (
                "after_off".into(),
                wf("on: module\nafter: [off]\nnodes: [{id: o, type: osv}]"),
            ),
        ]));
        let engine = |names: &[&str]| {
            let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
            Engine::new(&names, &defs)
        };
        let err = |names: &[&str]| engine(names).err().unwrap_or_default();
        assert!(err(&["mine"]).contains("which this source doesn't run"));
        assert!(err(&["pr", "default"]).contains("can't run after"));
        assert!(err(&["a", "b"]).contains("circle"));
        assert!(engine(&["mine", "default"]).is_ok());
        // Off, and so is what runs after it.
        assert!(!engine(&["after_off", "off"]).unwrap().runs("module"));
    }

    #[tokio::test]
    async fn unchanged_follows_commits_and_the_analysis_stamp() {
        let f = crate::golden::Fixture::new("unchanged");
        f.write(
            "m/__manifest__.py",
            "{'name': 'M', 'version': '18.0.1.0.0'}",
        );
        f.commit("one");
        let pool = f.db();
        let conn = &mut pool.get().unwrap();
        let mut probed = with_built_in(&HashMap::new())[DEFAULT].clone();
        probed.nodes.insert(
            0,
            parse(
                "on: module\nnodes: [{id: probe, type: log, event: probe, next: changed, \
             message: '{{ technical_name }} new={{ is_new }} unchanged={{ unchanged }}'}]",
            )
            .nodes
            .remove(0),
        );
        let defs = with_built_in(&HashMap::from([("probed".to_string(), probed)]));
        probe_run(&f, conn, &defs, &["probed"]).await;
        probe_run(&f, conn, &defs, &["probed"]).await;
        // Analyzed with other rules: done again, though no commit came.
        let stored =
            models::module::get_by_technical_name_odoo_version_organization_name_repository_name(
                conn, "m", &180, "org", "repo",
            );
        models::module::set_analysis_version(conn, &stored[0].id, analyzer::ANALYSIS_VERSION - 1)
            .unwrap();
        probe_run(&f, conn, &defs, &["probed"]).await;
        f.write("m/models.py", "x = 1\n");
        f.commit("two");
        probe_run(&f, conn, &defs, &["probed"]).await;
        probe_run(&f, conn, &defs, &["probed"]).await;
        let expect = [
            "new=true unchanged=false",
            "new=false unchanged=true",
            "new=false unchanged=false",
            "new=false unchanged=false",
            "new=false unchanged=true",
        ];
        assert_eq!(probes(conn), expect.map(|e| format!("m {e}")));
    }

    #[tokio::test]
    async fn a_workflow_that_does_not_store_sees_the_stored_data_current() {
        let f = crate::golden::Fixture::new("sidecar");
        f.write(
            "m/__manifest__.py",
            "{'name': 'M', 'version': '18.0.1.0.0'}",
        );
        f.commit("one");
        let pool = f.db();
        let conn = &mut pool.get().unwrap();
        // No store_module nor analyses: its own stamp would never match.
        let probe = parse(
            "on: module\nnodes: [{id: p, type: log, event: probe, \
             message: '{{ technical_name }} unchanged={{ unchanged }}'}]",
        );
        let defs = with_built_in(&HashMap::from([("probe".to_string(), probe)]));
        probe_run(&f, conn, &defs, &["default", "probe"]).await;
        probe_run(&f, conn, &defs, &["default", "probe"]).await;
        assert_eq!(probes(conn), ["m unchanged=false", "m unchanged=true"]);
    }

    #[tokio::test]
    async fn modules_a_workflow_skips_are_not_deleted() {
        let f = crate::golden::Fixture::new("skips");
        f.write(
            "a/__manifest__.py",
            "{'name': 'A', 'version': '18.0.1.0.0'}",
        );
        f.commit("one");
        let pool = f.db();
        let conn = &mut pool.get().unwrap();
        // Stores new modules only: the second run stores nothing, yet `a` stays.
        let new_only = parse(
            "on: module\nnodes: [{id: n, type: if, when: is_new, then: s}, {id: s, type: store_module}]",
        );
        let defs = with_built_in(&HashMap::from([("new_only".to_string(), new_only)]));
        probe_run(&f, conn, &defs, &["new_only"]).await;
        probe_run(&f, conn, &defs, &["new_only"]).await;
        let count = |conn: &mut SqliteConnection| {
            models::module::get_by_technical_name_odoo_version_organization_name_repository_name(
                conn, "a", &180, "org", "repo",
            )
            .len()
        };
        assert_eq!(count(conn), 1);
        // Gone from the repo: deleted, whatever the workflow does.
        fs::remove_dir_all(f.repo.join("a")).unwrap();
        f.write(
            "b/__manifest__.py",
            "{'name': 'B', 'version': '18.0.1.0.0'}",
        );
        f.commit("two");
        probe_run(&f, conn, &defs, &["new_only"]).await;
        assert_eq!(count(conn), 0);
    }

    /// The demo PR review: what it reacts to (a PR's own grave findings or
    /// its modules' migration notes), and what it asks and writes.
    #[test]
    fn the_demo_reviews_prs_with_grave_findings_or_migration_notes() {
        let finding = |code: &str, severity: &str, new: bool| {
            json!({"code": code, "severity": severity, "new": new, "file": "models/m.py",
                   "xml_id": null, "message": format!("{code} here")})
        };
        let note = json!({"severity": "warning", "code": "NAME_GET", "message": "name_get is gone",
                          "context": null, "file": "models/m.py", "line": 12});
        let (security, migration) = pr_problems(&[
            (
                "m",
                Some(
                    json!({"security": [finding("SQL", "error", true), finding("EVAL", "error", false),
                                         finding("ACL", "warning", true)],
                            "migration_notes": [note]}),
                ),
            ),
            ("other", None),
        ]);
        // Only what the PR brings: EVAL was already merged.
        assert_eq!(
            (&security["grave"], &security["minor"]),
            (&json!(1), &json!(1))
        );
        assert_eq!(security["findings"][0]["module"], "m");
        assert_eq!(migration["count"], 1);
        // The shipped workflows file (repo root, mounted in Docker): off.
        let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/../../workflows");
        let demo = &oghworkflow::file_workflows(shipped).unwrap()["ai_pr_review"];
        assert!(!demo.is_enabled());
        assert_eq!(validate("ai_pr_review", demo), Ok(()));
        // Named by a source, it still doesn't run.
        let defs = HashMap::from([("ai_pr_review".to_string(), demo.clone())]);
        let engine = Engine::new(&["ai_pr_review".to_string()], &defs).unwrap();
        assert!(!engine.runs("pull_request"));
        let mut env = Environment::new();
        env.add_function("diff", || "--- a/m/models/m.py".to_string());
        let view = |security: &Value, migration: &Value, answer: &str| {
            minijinja::Value::from_serialize(json!({
                "org": "org", "repo": "repo", "odoo_version": "18.0",
                "pr": {"number": 5, "title": "[18.0][MIG] m", "is_ours": false},
                "security": security, "migration": migration, "ai": {"ask": answer},
            }))
        };
        let holds = |view: &minijinja::Value| {
            let when = demo.nodes[1].when.as_deref().unwrap();
            env.compile_expression(when)
                .unwrap()
                .eval(view.clone())
                .unwrap()
                .is_true()
        };
        // Only PRs not its own, once their head is analyzed.
        let gate = |pr: Value| {
            let view = minijinja::Value::from_serialize(json!({"pr": pr}));
            let when = demo.nodes[0].when.as_deref().unwrap();
            env.compile_expression(when)
                .unwrap()
                .eval(view)
                .unwrap()
                .is_true()
        };
        assert!(gate(json!({"is_ours": false, "analyzed": true})));
        assert!(!gate(json!({"is_ours": false, "analyzed": false})));
        assert!(!gate(json!({"is_ours": true, "analyzed": true})));
        let none = json!({"grave": 0, "minor": 0, "findings": []});
        let no_notes = json!({"count": 0, "notes": []});
        assert!(holds(&view(&security, &no_notes, "")));
        assert!(holds(&view(&none, &migration, "")));
        assert!(!holds(&view(&none, &no_notes, "")));
        let full = view(&security, &migration, "--- a/m/models/m.py");
        let render = |src: &Option<String>| {
            env.render_str(src.as_deref().unwrap(), full.clone())
                .unwrap()
        };
        let prompt = render(&demo.nodes[2].prompt);
        assert!(
            prompt.contains("- m: SQL in models/m.py: SQL here\n"),
            "{prompt}"
        );
        assert!(!prompt.contains("ACL"), "{prompt}");
        assert!(
            prompt.contains("- m: NAME_GET in models/m.py:12: name_get is gone"),
            "{prompt}"
        );
        assert!(prompt.contains("--- a/m/models/m.py"), "{prompt}");
        let body = render(&demo.nodes[3].body);
        assert!(
            body.contains("- **SQL** in `m` models/m.py: SQL here"),
            "{body}"
        );
        assert!(
            body.contains("- migration: `m` models/m.py:12: name_get is gone"),
            "{body}"
        );
        assert!(body.contains("written by an AI"), "{body}");
    }

    #[test]
    fn diff_fails_rather_than_reviewing_nothing() {
        let f = crate::golden::Fixture::new("diff");
        f.write("a", "x\n");
        f.commit("one");
        let clone = f.repo.to_string_lossy().to_string();
        assert!(pr_diff(
            &clone,
            "origin/18.0...0123456789abcdef0123456789abcdef01234567"
        )
        .is_err());
        assert_eq!(pr_diff(&clone, "origin/18.0...HEAD").unwrap(), "");
    }

    #[test]
    fn the_stamp_tracks_which_analyses_run() {
        let built_in = with_built_in(&HashMap::new());
        assert_eq!(stamp(&built_in[DEFAULT]), analyzer::ANALYSIS_VERSION);
        let security_only = parse("on: module\nnodes: [{id: s, type: security}]");
        assert_ne!(stamp(&security_only), analyzer::ANALYSIS_VERSION);
    }
}
