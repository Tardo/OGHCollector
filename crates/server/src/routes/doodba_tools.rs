// Copyright Alexandre D. Díaz
use actix_multipart::form::{text::Text, MultipartForm};
use actix_web::{get, post, web, Error as AWError, HttpRequest, HttpResponse, Responder, Result};
use array_tool::vec::Uniq;
use diesel::sqlite::SqliteConnection;
use minijinja::context;
use oghutils::license::LicenseReport;
use oghutils::version::{odoo_version_string_to_u8, odoo_version_u8_to_string};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

use sqlitedb::{models, Pool};

use crate::config::SERVER_CONFIG;
use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::{get_minijinja_context, normalize_python_dep};

// These endpoints are unauthenticated and do all their work inside
// web::block holding one of the few read-only DB connections (pool default
// 15). An uncapped module list or a non-numeric version is a DoS/robustness
// lever, so reject both at the trust boundary before any DB work. The cap is
// `doodba_max_modules` in server.yaml (default 500).
fn validate_request(mods: &[Text<String>], versions: &[&str]) -> Result<(), AWError> {
    validate_module_count(mods.len())?;
    // Empty is allowed (an optional upper-bound version left blank); a
    // non-empty value must parse, since odoo_version_string_to_u8 unwraps.
    for v in versions.iter().filter(|v| !v.is_empty()) {
        crate::utils::odoo_version(v)?;
    }
    Ok(())
}

/// Shared module-count cap (`doodba_max_modules`) for every endpoint that
/// walks the dependency closure of a caller-supplied module list.
pub fn validate_module_count(count: usize) -> Result<(), AWError> {
    let max = *SERVER_CONFIG.get_doodba_max_modules();
    if count > max {
        return Err(actix_web::error::ErrorBadRequest(format!(
            "too many modules: {count} (max {max})"
        )));
    }
    Ok(())
}

#[derive(MultipartForm)]
struct ConverterForm {
    modules: Vec<Text<String>>,
    odoo_version: Text<String>,
}

#[derive(MultipartForm)]
struct DepResolverForm {
    modules: Vec<Text<String>>,
    odoo_version: Text<String>,
}

#[get("/doodba/converter")]
pub async fn route_doodba_converter(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    tmpl_env.render(
        "pages/doodba_tools/converter.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "doodba_converter",
                DOODBA_MAX_MODULES => *SERVER_CONFIG.get_doodba_max_modules(),
            )
        ),
    )
}

fn get_doodba_addons(
    conn: &mut SqliteConnection,
    mods: &[Text<String>],
    odoo_version: &str,
) -> Vec<sqlitedb::models::module::ModuleRepositoryInfo> {
    let odoo_ver = odoo_version_string_to_u8(odoo_version);
    let modules: Vec<String> = mods.iter().map(|x| x.as_str().to_string()).collect();
    models::module::get_module_repository(conn, &odoo_ver, modules.as_slice())
}

#[post("/doodba/converter/addons")]
pub async fn route_doodba_converter_addons(
    pool: web::Data<Pool>,
    form: MultipartForm<ConverterForm>,
) -> Result<HttpResponse, AWError> {
    validate_request(&form.modules, &[form.odoo_version.as_str()])?;
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_doodba_addons(&mut conn, &form.modules, &form.odoo_version)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[get("/doodba/dependency-resolver")]
pub async fn route_doodba_dependency_resolver(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    tmpl_env.render(
        "pages/doodba_tools/dependency_resolver.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "doodba_dep_resolver",
                DOODBA_MAX_MODULES => *SERVER_CONFIG.get_doodba_max_modules(),
            )
        ),
    )
}

// Same shape as ModuleDependencyInfoResponse plus `repos` (repo name ->
// organization) and `repo_urls` (repo name -> clone URL), needed to emit a
// doodba repos.yaml (git remotes), and the license review of the whole set.
#[derive(Debug, Serialize)]
pub struct DoodbaAddonsResponse {
    pub odoo: HashMap<String, Vec<String>>,
    pub pip: Vec<String>,
    pub bin: Vec<String>,
    pub repos: HashMap<String, String>,
    pub repo_urls: HashMap<String, String>,
    pub license_review: LicenseReport,
}

// One walk over the union of the requested modules' closures. Groups by repo
// name only (doodba's addons.yaml/repos.yaml are keyed that way), requested
// modules first, then their dependencies.
fn get_doodba_addons_full(
    conn: &mut SqliteConnection,
    mods: &[Text<String>],
    odoo_version: &str,
) -> DoodbaAddonsResponse {
    let odoo_ver = odoo_version_string_to_u8(odoo_version);
    let modules: Vec<String> = mods.iter().map(|x| x.as_str().to_string()).collect();
    let modules = models::module::get_by_technical_name_odoo_version(conn, &modules, &odoo_ver);
    let closure = models::dependency::get_full_dependency_info_many(conn, &modules);
    let root_ids: Vec<i64> = modules.iter().map(|m| m.id).collect();

    let roots = root_ids.iter().map(|id| {
        let node = &closure.nodes[id];
        (
            node.organization.clone(),
            node.repository.clone(),
            vec![node.technical_name.clone()],
        )
    });
    let deps = closure.odoo.iter().map(|(key, names)| {
        let (org, repo) = key.split_once('/').unwrap_or(("", key));
        (org.to_string(), repo.to_string(), names.clone())
    });
    let mut odoo: HashMap<String, Vec<String>> = HashMap::new();
    let stored_urls = models::gh_repository::clone_urls(conn);
    let mut repos: HashMap<String, String> = HashMap::new();
    let mut repo_urls: HashMap<String, String> = HashMap::new();
    for (org, repo, names) in roots.chain(deps) {
        repo_urls.entry(repo.clone()).or_insert_with(|| {
            let stored = stored_urls
                .get(&format!("{org}/{repo}"))
                .map(String::as_str);
            oghutils::git::clone_url(stored, &org, &repo)
        });
        repos.entry(repo.clone()).or_insert(org);
        let group = odoo.entry(repo).or_default();
        for name in names {
            if !group.contains(&name) {
                group.push(name);
            }
        }
    }

    DoodbaAddonsResponse {
        odoo,
        pip: closure
            .pip
            .iter()
            .cloned()
            .map(normalize_python_dep)
            .collect::<Vec<_>>()
            .unique(),
        bin: closure.bin.clone(),
        repos,
        repo_urls,
        license_review: closure.license_review(&root_ids),
    }
}

#[post("/doodba/dependency-resolver/addons")]
pub async fn route_doodba_dependency_resolver_addons(
    pool: web::Data<Pool>,
    form: MultipartForm<DepResolverForm>,
) -> Result<HttpResponse, AWError> {
    validate_request(&form.modules, &[form.odoo_version.as_str()])?;
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_doodba_addons_full(&mut conn, &form.modules, &form.odoo_version)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[derive(MultipartForm)]
struct MigrationPlanForm {
    modules: Vec<Text<String>>,
    from_version: Text<String>,
    to_version: Option<Text<String>>,
}

// A module that isn't merged yet at a given target version, but has an open
// migration PR/MR already visible upstream (see [[pull_request]] model).
#[derive(Debug, Serialize, Clone)]
pub struct MigrationPendingModuleInfo {
    pub technical_name: String,
    pub repository_name: String,
    pub organization: String,
    pub prid: i64,
    pub title: String,
    pub url: String,
    /// Ref to fetch the PR/MR head from (`refs/pull/N/head` on GitHub,
    /// `refs/merge-requests/N/head` on GitLab), for repos.yaml merges.
    pub merge_ref: String,
    pub age_days: Option<i64>,
    pub ci_status: Option<String>,
}

// A module the system has seen merged (any version) or PR'd (any version)
// before, but that isn't merged nor pending in this specific target version -
// a real blocker, as opposed to `unknown` below.
#[derive(Debug, Serialize, Clone)]
pub struct MigrationMissingModuleInfo {
    pub technical_name: String,
    pub repository_name: String,
    pub organization: String,
}

#[derive(Debug, Serialize)]
pub struct MigrationPlanStepResponse {
    pub version: String,
    pub merged: Vec<models::module::ModuleRepositoryInfo>,
    pub pending: Vec<MigrationPendingModuleInfo>,
    pub missing: Vec<MigrationMissingModuleInfo>,
    // Repo name -> clone URL of every repo above, for repos.yaml remotes.
    pub repo_urls: HashMap<String, String>,
    // Requested modules the system has never tracked anywhere (no merged row,
    // no PR/MR, in any version) - reported separately from `missing` since we
    // genuinely don't know whether they exist for this version or not.
    pub unknown: Vec<String>,
}

#[get("/doodba/migration")]
pub async fn route_doodba_migration_plan(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    tmpl_env.render(
        "pages/doodba_tools/migration_plan.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "doodba_migration_plan",
                DOODBA_MAX_MODULES => *SERVER_CONFIG.get_doodba_max_modules(),
            )
        ),
    )
}

/// The open migration PR of each module most likely to merge. Also used by
/// `/migration` over a workspace pool, where a PR of a repository outside
/// the workspace has no repository row: skipped, not unwrapped.
pub(crate) fn get_pending_modules(
    conn: &mut SqliteConnection,
    technical_names: &[String],
    version_odoo: &u8,
) -> Vec<MigrationPendingModuleInfo> {
    let mut prs = models::pull_request::get_by_technical_names_odoo_version(
        conn,
        technical_names,
        version_odoo,
    );
    // Several open ports of one module: offer the one most likely to merge.
    prs.sort_by_key(|(_, pr)| pr.preference_key());
    let mut seen = HashSet::new();
    prs.into_iter()
        .filter_map(|(module, pr)| {
            let repo = models::gh_repository::get_by_id(conn, &pr.gh_repository_id)?;
            let org = models::gh_organization::get_by_id(conn, &repo.gh_organization_id)?;
            let git = oghutils::git::clone_url(repo.url.as_deref(), &org.name, &repo.name);
            Some(MigrationPendingModuleInfo {
                url: oghutils::git::pull_request_url(&git, pr.prid),
                merge_ref: oghutils::git::pull_request_ref(&git, pr.prid),
                age_days: models::pull_request::days_since(pr.created_at.as_deref()),
                ci_status: pr.ci_status,
                technical_name: module,
                repository_name: repo.name,
                organization: org.name,
                prid: pr.prid,
                title: pr.name,
            })
        })
        .filter(|info| seen.insert(info.technical_name.clone()))
        .collect()
}

/// For each Odoo version between `from_version` and `to_version` (inclusive,
/// only versions actually present in the DB), classifies every requested module
/// as merged / pending-PR / missing / unknown. `from_version` itself is included
/// as a baseline step so the current addons.yaml can be sanity-checked too.
///
/// Odoo dependency graphs shift between versions (a dependency can appear,
/// disappear, or get absorbed into another module), so the transitive Odoo
/// dependency closure of the *originally requested* modules is re-resolved
/// fresh for every step from that step's own DB state - never carried over
/// from a previous step's closure. Closure modules are always merged at the
/// step they were discovered in (dependency rows only link modules within
/// the same `version_odoo`), so they only ever land in `merged`, feeding
/// into that step's `addons.yaml`; only the originally requested modules can
/// end up `pending`/`missing`/`unknown`.
fn get_migration_plan(
    conn: &mut SqliteConnection,
    mods: &[Text<String>],
    from_version: &str,
    to_version: Option<&str>,
) -> Vec<MigrationPlanStepResponse> {
    let modules: Vec<String> = mods.iter().map(|x| x.as_str().to_string()).collect();
    let from_ver = odoo_version_string_to_u8(from_version) as i32;
    let to_ver = to_version
        .filter(|v| !v.is_empty())
        .map(|v| odoo_version_string_to_u8(v) as i32);

    let mut steps: Vec<i32> = models::module::get_odoo_versions(conn)
        .into_iter()
        .filter(|v| *v >= from_ver && to_ver.is_none_or(|to| *v <= to))
        .collect();
    steps.sort_unstable();

    // "Registered" = the system has seen this technical_name somewhere before
    // (merged in any version, or PR'd for any version) - regardless of the
    // step being evaluated. Modules outside this set are never flagged as
    // missing: we have no data on them (private/unindexed repo, typo, ...),
    // so a version comparison can't say whether they exist there or not.
    let mut registered: HashMap<String, (String, String)> =
        models::module::get_repository_org_by_technical_names(conn, &modules)
            .into_iter()
            .map(|info| {
                (
                    info.technical_name,
                    (info.repository_name, info.organization),
                )
            })
            .collect();
    for (technical_name, repository_name, organization) in
        models::pull_request::get_repository_org_by_technical_names(conn, &modules)
    {
        registered
            .entry(technical_name)
            .or_insert((repository_name, organization));
    }

    // Seeded once from migrations (see sqlitedb::models::mod::tests::
    // test_dependency_type_seeded) - always present.
    let module_dep_type_id = models::dependency_type::get_by_name(conn, "module")
        .unwrap()
        .id;
    let stored_urls = models::gh_repository::clone_urls(conn);

    steps
        .into_iter()
        .map(|version_odoo| {
            let version_u8 = version_odoo as u8;

            // Requested modules actually merged at this version seed the
            // dependency walk; their transitive Odoo deps at this version
            // join the set to classify, so a required-but-unlisted module
            // still ends up in this step's addons.yaml/repos.yaml. A
            // declared dependency with no module row here yet is a real
            // migration blocker - collected separately since it can't be
            // recursed into, and isn't in `registered` (built from the
            // originally requested list only).
            let seed_modules =
                models::module::get_by_technical_name_odoo_version(conn, &modules, &version_u8);
            let mut expanded_modules = modules.clone();
            let mut dep_blocker_names: Vec<String> = Vec::new();
            for seed in &seed_modules {
                let deps = models::dependency::get_full_dependency_info_with_unresolved(
                    conn,
                    seed,
                    &module_dep_type_id,
                );
                for (repo_key, names) in &deps.resolved.odoo {
                    // Odoo core ships in the base doodba image already - never
                    // let it leak into the generated addons.yaml, same as it's
                    // already excluded from repos.yaml.
                    if repo_key == "odoo/odoo" {
                        continue;
                    }
                    for name in names {
                        if !expanded_modules.contains(name) {
                            expanded_modules.push(name.clone());
                        }
                    }
                }
                for name in deps.unresolved {
                    if !expanded_modules.contains(&name) && !dep_blocker_names.contains(&name) {
                        dep_blocker_names.push(name);
                    }
                }
            }

            let merged =
                models::module::get_module_repository(conn, &version_u8, &expanded_modules);
            let merged_names: HashSet<&str> =
                merged.iter().map(|m| m.technical_name.as_str()).collect();
            let remaining: Vec<String> = modules
                .iter()
                .filter(|m| !merged_names.contains(m.as_str()))
                .cloned()
                .collect();
            let mut pending = get_pending_modules(conn, &remaining, &version_u8);
            let pending_names: HashSet<&str> =
                pending.iter().map(|p| p.technical_name.as_str()).collect();
            let mut missing = Vec::new();
            let mut unknown = Vec::new();
            for technical_name in remaining
                .into_iter()
                .filter(|m| !pending_names.contains(m.as_str()))
            {
                match registered.get(&technical_name) {
                    Some((repository_name, organization)) => {
                        missing.push(MigrationMissingModuleInfo {
                            technical_name,
                            repository_name: repository_name.clone(),
                            organization: organization.clone(),
                        });
                    }
                    None => unknown.push(technical_name),
                }
            }

            if !dep_blocker_names.is_empty() {
                let dep_pending = get_pending_modules(conn, &dep_blocker_names, &version_u8);
                let dep_pending_names: HashSet<&str> = dep_pending
                    .iter()
                    .map(|p| p.technical_name.as_str())
                    .collect();
                dep_blocker_names.retain(|n| !dep_pending_names.contains(n.as_str()));
                pending.extend(dep_pending);

                let mut dep_registered: HashMap<String, (String, String)> =
                    models::module::get_repository_org_by_technical_names(conn, &dep_blocker_names)
                        .into_iter()
                        .map(|info| {
                            (
                                info.technical_name,
                                (info.repository_name, info.organization),
                            )
                        })
                        .collect();
                for (technical_name, repository_name, organization) in
                    models::pull_request::get_repository_org_by_technical_names(
                        conn,
                        &dep_blocker_names,
                    )
                {
                    dep_registered
                        .entry(technical_name)
                        .or_insert((repository_name, organization));
                }
                for technical_name in dep_blocker_names {
                    match dep_registered.get(&technical_name) {
                        Some((repository_name, organization)) => {
                            missing.push(MigrationMissingModuleInfo {
                                technical_name,
                                repository_name: repository_name.clone(),
                                organization: organization.clone(),
                            });
                        }
                        None => unknown.push(technical_name),
                    }
                }
            }

            let repo_urls = merged
                .iter()
                .map(|m| (&m.organization, &m.repository_name))
                .chain(
                    pending
                        .iter()
                        .map(|p| (&p.organization, &p.repository_name)),
                )
                .chain(
                    missing
                        .iter()
                        .map(|m| (&m.organization, &m.repository_name)),
                )
                .map(|(org, repo)| {
                    let stored = stored_urls
                        .get(&format!("{org}/{repo}"))
                        .map(String::as_str);
                    (repo.clone(), oghutils::git::clone_url(stored, org, repo))
                })
                .collect();

            MigrationPlanStepResponse {
                version: odoo_version_u8_to_string(&version_u8),
                merged,
                pending,
                missing,
                repo_urls,
                unknown,
            }
        })
        .collect()
}

#[post("/doodba/migration/plan")]
pub async fn route_doodba_migration_plan_addons(
    pool: web::Data<Pool>,
    form: MultipartForm<MigrationPlanForm>,
) -> Result<HttpResponse, AWError> {
    let mut versions = vec![form.from_version.as_str()];
    if let Some(to) = &form.to_version {
        versions.push(to.as_str());
    }
    validate_request(&form.modules, &versions)?;
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_migration_plan(
            &mut conn,
            &form.modules,
            &form.from_version,
            form.to_version.as_ref().map(|t| t.as_str()),
        )
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::Connection;
    use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
    use sqlitedb::models::{
        dependency_module, dependency_type, gh_organization, gh_repository, module, pull_request,
        pull_request_module,
    };

    const MIGRATIONS: EmbeddedMigrations = embed_migrations!("../../migrations");

    #[test]
    fn test_validate_request_caps_modules_and_rejects_bad_version() {
        let max = *SERVER_CONFIG.get_doodba_max_modules();
        let mods =
            |n: usize| -> Vec<Text<String>> { (0..n).map(|_| Text("a".to_string())).collect() };
        let ok = mods(max);
        assert!(validate_request(&ok, &["17.0"]).is_ok());
        assert!(validate_request(&ok, &[""]).is_ok()); // blank upper bound is fine

        assert!(validate_request(&mods(max + 1), &["17.0"]).is_err());

        assert!(validate_request(&ok, &["abc"]).is_err());
        assert!(validate_request(&ok, &["17.0", "not-a-version"]).is_err());

        // Whatever passes validation must convert downstream too.
        assert!(validate_request(&ok, &[" 17.0"]).is_ok());
        let mut conn = setup_db();
        let _ = get_doodba_addons(&mut conn, &ok, " 17.0");
        let _ = get_migration_plan(&mut conn, &ok, " 17.0", Some(" 18.0"));
    }

    fn setup_db() -> SqliteConnection {
        let mut conn = SqliteConnection::establish(":memory:").unwrap();
        conn.run_pending_migrations(MIGRATIONS).unwrap();
        conn
    }

    fn make_module(
        conn: &mut SqliteConnection,
        tech_name: &str,
        git_org: &str,
        git_repo: &str,
        version_odoo: u8,
    ) -> module::Model {
        module::add(
            conn,
            &module::ManifestInfo {
                source_path: tech_name.to_string(),
                technical_name: tech_name.to_string(),
                version_odoo,
                name: tech_name.to_string(),
                version_module: "1.0.0".to_string(),
                description: String::new(),
                installation: String::new(),
                usage: String::new(),
                icon: String::new(),
                author: String::new(),
                website: String::new(),
                license: String::new(),
                category: String::new(),
                auto_install: false,
                application: false,
                installable: true,
                maintainer: String::new(),
                git_org: git_org.to_string(),
                git_repo: git_repo.to_string(),
                depends: vec![],
                external_depends_python: vec![],
                external_depends_bin: vec![],
                folder_size: 0,
                last_commit_hash: "abc".to_string(),
                last_commit_author: String::new(),
                last_commit_date: "2024-01-01".to_string(),
                last_commit_name: String::new(),
                last_commit_partof: String::new(),
                committers: HashMap::new(),
                analysis: Default::default(),
                source_unchanged: false,
            },
        )
        .unwrap()
    }

    // Records that `from` depends on `to` (both at the same version_odoo).
    fn link_module_dep(conn: &mut SqliteConnection, from: &module::Model, to: &module::Model) {
        let dep_type = dependency_type::get_by_name(conn, "module").unwrap();
        dependency_module::add(conn, &dep_type.id, &to.technical_name, &from.id).unwrap();
    }

    // Declares a dependency by name only, regardless of whether a module row
    // for it exists at `from`'s version - covers the "unported dependency"
    // blocker case, which `link_module_dep` can't express (it requires an
    // actual `module::Model` at the same version).
    fn declare_dep_by_name(conn: &mut SqliteConnection, from: &module::Model, dep_name: &str) {
        let dep_type = dependency_type::get_by_name(conn, "module").unwrap();
        dependency_module::add(conn, &dep_type.id, dep_name, &from.id).unwrap();
    }

    fn make_pending_pr(
        conn: &mut SqliteConnection,
        tech_name: &str,
        git_org: &str,
        git_repo: &str,
        version_odoo: u8,
        prid: i64,
    ) {
        let org = gh_organization::add(conn, git_org).unwrap();
        let repo = gh_repository::add(conn, &org.id, git_repo).unwrap();
        let name = format!("[{version_odoo}.0] {tech_name}: migration");
        let (_, pr) = pull_request::add(
            conn,
            &pull_request::NewPullRequest {
                name: &name,
                version_odoo: version_odoo as i32,
                prid,
                gh_repository_id: repo.id,
                created_at: None,
                ci_status: None,
                last_message_at: None,
                head_sha: None,
                author: None,
                is_draft: false,
                labels: None,
            },
        )
        .unwrap();
        pull_request_module::replace_for_pull_request(
            conn,
            pr.id,
            &[pull_request_module::NewPullRequestModule {
                pull_request_id: pr.id,
                module_technical_name: tech_name,
                is_new: true,
                analysis: None,
            }],
        )
        .unwrap();
    }

    // Covers the three-way split this whole feature is about: a module
    // merged elsewhere but not in the target version (and no open PR) is a
    // real "missing" blocker with repo/org attached; a module the system has
    // never tracked anywhere is reported separately as "unknown" and must
    // NOT be treated as missing, since we simply don't know if it exists.
    #[test]
    fn test_migration_plan_splits_missing_registered_from_unknown() {
        let mut conn = setup_db();
        // version_odoo is stored as version*10 (see odoo_version_string_to_u8).
        make_module(&mut conn, "mod_merged", "OCA", "repoA", 170);
        make_module(&mut conn, "mod_registered_missing", "OCA", "repoB", 160);
        make_pending_pr(&mut conn, "mod_pending", "OCA", "repoC", 170, 123);

        let modules = vec![
            Text("mod_merged".to_string()),
            Text("mod_registered_missing".to_string()),
            Text("mod_pending".to_string()),
            Text("mod_never_seen".to_string()),
        ];

        let steps = get_migration_plan(&mut conn, &modules, "17.0", Some("17.0"));
        assert_eq!(steps.len(), 1);
        let step = &steps[0];

        assert_eq!(
            step.merged
                .iter()
                .map(|m| m.technical_name.as_str())
                .collect::<Vec<_>>(),
            vec!["mod_merged"]
        );
        assert_eq!(
            step.pending
                .iter()
                .map(|p| p.technical_name.as_str())
                .collect::<Vec<_>>(),
            vec!["mod_pending"]
        );
        assert_eq!(step.missing.len(), 1);
        assert_eq!(step.missing[0].technical_name, "mod_registered_missing");
        assert_eq!(step.missing[0].repository_name, "repoB");
        assert_eq!(step.missing[0].organization, "OCA");
        assert_eq!(step.unknown, vec!["mod_never_seen".to_string()]);
        // Remotes/merge refs follow the repo's host: GitHub by default...
        assert_eq!(step.repo_urls["repoA"], "https://github.com/OCA/repoA.git");
        assert_eq!(step.pending[0].merge_ref, "refs/pull/123/head");
        // ...GitLab once the collector stored a GitLab clone URL.
        let org = gh_organization::get_by_name(&mut conn, "OCA").unwrap();
        let repo_c = gh_repository::get_by_name(&mut conn, &org.id, "repoC").unwrap();
        let gl = "https://git.acme.io/oca/repoC.git";
        gh_repository::set_url(&mut conn, &repo_c.id, gl).unwrap();
        let step = &get_migration_plan(&mut conn, &modules, "17.0", Some("17.0"))[0];
        assert_eq!(step.repo_urls["repoC"], gl);
        assert_eq!(step.pending[0].merge_ref, "refs/merge-requests/123/head");
        assert_eq!(
            step.pending[0].url,
            "https://git.acme.io/oca/repoC/-/merge_requests/123"
        );
    }

    // Proves dependencies are re-resolved fresh per step, not accumulated
    // from a previous step: "dep" is a real dependency of "root" at 16.0
    // only. Both module rows also exist at 17.0, but the dependency link
    // isn't recreated there, so "dep" must be pulled in at the 16.0 step and
    // dropped again at the 17.0 step even though it's still in the DB.
    #[test]
    fn test_migration_plan_recomputes_dependency_closure_per_step() {
        let mut conn = setup_db();
        let root_v16 = make_module(&mut conn, "root", "OCA", "repo-root", 160);
        let dep_v16 = make_module(&mut conn, "dep", "OCA", "repo-dep", 160);
        link_module_dep(&mut conn, &root_v16, &dep_v16);
        make_module(&mut conn, "root", "OCA", "repo-root", 170);
        make_module(&mut conn, "dep", "OCA", "repo-dep", 170); // no link at 17.0

        let modules = vec![Text("root".to_string())];
        let steps = get_migration_plan(&mut conn, &modules, "16.0", Some("17.0"));
        assert_eq!(steps.len(), 2);

        fn names(step: &MigrationPlanStepResponse) -> Vec<&str> {
            let mut v: Vec<&str> = step
                .merged
                .iter()
                .map(|m| m.technical_name.as_str())
                .collect();
            v.sort_unstable();
            v
        }
        assert_eq!(names(&steps[0]), vec!["dep", "root"]);
        assert_eq!(names(&steps[1]), vec!["root"]);
    }

    // The real point of dependency recomputation: a module a user never
    // listed can still block the jump. "root" (explicitly requested)
    // declares two dependencies at 17.0 that have no module row there:
    // "dep_elsewhere" (which does exist at 16.0, so it's a known blocker
    // with a repo attached) and "dep_ghost" (never tracked anywhere, so it
    // must be unknown, not asserted as missing).
    #[test]
    fn test_migration_plan_flags_unported_dependency_as_blocker() {
        let mut conn = setup_db();
        make_module(&mut conn, "dep_elsewhere", "OCA", "repo-dep", 160);
        let root_v17 = make_module(&mut conn, "root", "OCA", "repo-root", 170);
        declare_dep_by_name(&mut conn, &root_v17, "dep_elsewhere");
        declare_dep_by_name(&mut conn, &root_v17, "dep_ghost");

        let modules = vec![Text("root".to_string())];
        let steps = get_migration_plan(&mut conn, &modules, "17.0", Some("17.0"));
        assert_eq!(steps.len(), 1);
        let step = &steps[0];

        assert_eq!(
            step.merged
                .iter()
                .map(|m| m.technical_name.as_str())
                .collect::<Vec<_>>(),
            vec!["root"]
        );
        assert_eq!(step.missing.len(), 1);
        assert_eq!(step.missing[0].technical_name, "dep_elsewhere");
        assert_eq!(step.missing[0].repository_name, "repo-dep");
        assert_eq!(step.missing[0].organization, "OCA");
        assert_eq!(step.unknown, vec!["dep_ghost".to_string()]);
    }

    // Odoo core (org "odoo", e.g. `base`/`web`/`mail`) is tracked like any
    // other repo, so a real dependency on it resolves in the closure walk -
    // but it ships in the base doodba image already and must never end up
    // in the generated addons.yaml (`merged`), unlike an OCA dependency.
    #[test]
    fn test_migration_plan_excludes_odoo_core_from_dependency_closure() {
        let mut conn = setup_db();
        let core_v17 = make_module(&mut conn, "base", "odoo", "odoo", 170);
        let root_v17 = make_module(&mut conn, "root", "OCA", "repo-root", 170);
        link_module_dep(&mut conn, &root_v17, &core_v17);

        let modules = vec![Text("root".to_string())];
        let steps = get_migration_plan(&mut conn, &modules, "17.0", Some("17.0"));
        let step = &steps[0];

        assert_eq!(
            step.merged
                .iter()
                .map(|m| m.technical_name.as_str())
                .collect::<Vec<_>>(),
            vec!["root"]
        );
        assert!(step.missing.is_empty());
        assert!(step.unknown.is_empty());
    }
}
