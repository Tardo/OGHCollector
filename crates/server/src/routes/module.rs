// Copyright Alexandre D. Díaz
use std::collections::{HashMap, HashSet};

use actix_web::{get, http::StatusCode, web, HttpRequest, HttpResponse, Responder, Result};
use diesel::sqlite::SqliteConnection;
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use oghutils::version::{odoo_version_u8_to_string, parse_odoo_version};
use sqlitedb::{models, Pool};

use super::api::v1::module::{process_modules_db, ModuleFullInfoResponse};

#[derive(Debug, Deserialize)]
pub struct RouteModulePageRequest {
    version: Option<String>,
}

// A version_module ever seen for a module, for the version-history dropdown
// on the module detail page. Fetched eagerly (not via a client-side request)
// since the page is server-rendered and the history is small per module.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleVersionHistoryInfo {
    pub version_module: String,
    pub create_date: String,
    pub is_latest: bool,
}

// An open PR/MR that changes this module, in any Odoo version: a port (the
// module isn't on that branch yet) or a change to it, with what the collector
// found in the module at the PR head.
#[derive(Debug, Serialize)]
pub struct ModulePullRequestInfo {
    pub title: String,
    pub prid: i64,
    pub odoo_version: String,
    pub repository: String,
    pub organization: String,
    pub url: String,
    pub author: Option<String>,
    pub age_days: Option<i64>,
    pub last_message_days: Option<i64>,
    pub ci_status: Option<String>,
    pub is_draft: bool,
    pub labels: Vec<String>,
    pub is_migration: bool,
    /// The one to pick among its version's PRs (`preference_key`), when it
    /// has several.
    pub preferred: bool,
    /// Module version (manifest) at the PR head.
    pub version: Option<String>,
    /// Grave security findings at the head / of those, not in the merged
    /// module (or, for a port, its previous version).
    pub grave: usize,
    pub grave_new: usize,
    pub minor: usize,
    pub notes: usize,
    /// `security`/`migration_notes` lists; `None` when the PR changes no code
    /// of the module or it isn't analyzed yet.
    pub analysis: Option<serde_json::Value>,
}

fn get_module_pull_requests(
    conn: &mut SqliteConnection,
    org: &str,
    module_technical_name: &str,
) -> Vec<ModulePullRequestInfo> {
    let mut rows = models::pull_request::get_by_technical_name_organization_name(
        conn,
        module_technical_name,
        org,
    );
    // Newest version first, then the PR most likely to merge.
    rows.sort_by_key(|(pr, _)| (std::cmp::Reverse(pr.version_odoo), pr.preference_key()));
    let mut repos: HashMap<i64, (String, String, String)> = HashMap::new();
    let mut per_version: HashMap<i32, usize> = HashMap::new();
    for (pr, _) in &rows {
        *per_version.entry(pr.version_odoo).or_default() += 1;
    }
    let mut seen_versions = HashSet::new();
    rows.into_iter()
        .map(|(pr, module)| {
            let (repository, organization, git) = repos
                .entry(pr.gh_repository_id)
                .or_insert_with(|| {
                    let repo =
                        models::gh_repository::get_by_id(conn, &pr.gh_repository_id).unwrap();
                    let org =
                        models::gh_organization::get_by_id(conn, &repo.gh_organization_id).unwrap();
                    let git = oghutils::git::clone_url(repo.url.as_deref(), &org.name, &repo.name);
                    (repo.name, org.name, git)
                })
                .clone();
            let analysis: Option<serde_json::Value> = module
                .analysis
                .as_deref()
                .and_then(|a| serde_json::from_str(a).ok());
            let security = analysis
                .as_ref()
                .and_then(|a| a["security"].as_array().cloned())
                .unwrap_or_default();
            let grave: Vec<&serde_json::Value> = security
                .iter()
                .filter(|w| w["severity"] == "error")
                .collect();
            ModulePullRequestInfo {
                url: oghutils::git::pull_request_url(&git, pr.prid),
                age_days: models::pull_request::days_since(pr.created_at.as_deref()),
                last_message_days: models::pull_request::days_since(pr.last_message_at.as_deref()),
                // Only worth pointing out among alternatives.
                preferred: seen_versions.insert(pr.version_odoo)
                    && per_version[&pr.version_odoo] > 1,
                labels: pr.label_names(),
                version: analysis
                    .as_ref()
                    .and_then(|a| a["version"].as_str().map(str::to_string)),
                grave_new: grave.iter().filter(|w| w["new"] == true).count(),
                grave: grave.len(),
                minor: security.len() - grave.len(),
                notes: analysis
                    .as_ref()
                    .and_then(|a| a["migration_notes"].as_array().map(Vec::len))
                    .unwrap_or_default(),
                analysis,
                is_migration: module.is_new,
                is_draft: pr.is_draft,
                author: pr.author,
                ci_status: pr.ci_status,
                title: pr.name,
                prid: pr.prid,
                odoo_version: odoo_version_u8_to_string(&(pr.version_odoo as u8)),
                repository,
                organization,
            }
        })
        .collect()
}

// A rename/merge OpenUpgrade records for this module's name (joined by
// name, like `/migration`).
#[derive(Debug, Serialize)]
pub struct ModuleUpstreamInfo {
    /// `renamed` / `merged` (into an existing module).
    pub kind: &'static str,
    pub name: String,
    pub odoo_version: String,
    /// `name`'s organization, when it's collected (its page link).
    pub org: Option<String>,
    /// In `replaces`: what `name` became at that step, when it isn't this
    /// module yet (it became this one at a later step).
    pub via: Option<String>,
}

#[derive(Debug, Serialize, Default)]
pub struct ModuleUpstream {
    /// What this module became, each followed through the later versions
    /// (renamed to B in 17.0, then merged into C in 18.0).
    pub became: Vec<Vec<ModuleUpstreamInfo>>,
    /// What became this module, directly or through later steps, per
    /// version: dozens for a core module (`account`), so the page folds them
    /// into one line.
    pub replaces: Vec<(String, Vec<ModuleUpstreamInfo>)>,
    /// Modules in `replaces`.
    pub replaced: usize,
}

fn get_module_upstream(conn: &mut SqliteConnection, technical_name: &str) -> ModuleUpstream {
    let rows = models::openupgrade_rename::all(conn);
    // Every step of a chain is at a later version (forward) or an earlier
    // one (backward), so both walks end, renames back and forth included.
    let next = |r: &models::openupgrade_rename::Model| {
        rows.iter()
            .filter(|n| n.old_name == r.new_name && n.version_odoo > r.version_odoo)
            .min_by_key(|n| n.version_odoo)
    };
    let chains: Vec<Vec<&models::openupgrade_rename::Model>> = rows
        .iter()
        .filter(|r| r.old_name == technical_name)
        .map(|first| {
            let mut chain = vec![first];
            while let Some(n) = next(chain[chain.len() - 1]) {
                chain.push(n);
            }
            chain
        })
        .collect();
    let mut back = Vec::new();
    let mut queue = vec![(technical_name, i32::MAX)];
    let mut seen = HashSet::new();
    while let Some((name, before)) = queue.pop() {
        for r in &rows {
            if r.new_name == name
                && r.version_odoo < before
                && r.old_name != technical_name
                && seen.insert((r.version_odoo, r.old_name.as_str()))
            {
                back.push(r);
                queue.push((&r.old_name, r.version_odoo));
            }
        }
    }
    back.sort_by_key(|r| (r.version_odoo, r.old_name.as_str()));

    let names: Vec<String> = chains
        .iter()
        .flatten()
        .map(|r| r.new_name.clone())
        .chain(back.iter().map(|r| r.old_name.clone()))
        .collect();
    let orgs: HashMap<String, String> =
        models::module::get_repository_org_by_technical_names(conn, &names)
            .into_iter()
            .map(|m| (m.technical_name, m.organization))
            .collect();
    let info = |r: &models::openupgrade_rename::Model, name: &str, via| ModuleUpstreamInfo {
        kind: if r.merged { "merged" } else { "renamed" },
        name: name.to_string(),
        odoo_version: odoo_version_u8_to_string(&(r.version_odoo as u8)),
        org: orgs.get(name).cloned(),
        via,
    };
    let mut upstream = ModuleUpstream {
        became: chains
            .iter()
            .map(|chain| chain.iter().map(|r| info(r, &r.new_name, None)).collect())
            .collect(),
        replaced: back.len(),
        ..Default::default()
    };
    // Sorted by version, so each version's are consecutive.
    for r in back {
        let via = (r.new_name != technical_name).then(|| r.new_name.clone());
        let info = info(r, &r.old_name, via);
        match upstream.replaces.last_mut() {
            Some((v, infos)) if *v == info.odoo_version => infos.push(info),
            _ => upstream
                .replaces
                .push((info.odoo_version.clone(), vec![info])),
        }
    }
    upstream
}

// A repository with a module of this technical name, in any organization:
// the same name, not necessarily the same code.
#[derive(Debug, Serialize, PartialEq)]
pub struct ModuleLocation {
    pub org: String,
    pub repo: String,
    pub versions: Vec<String>,
    /// It starts after every earlier one ended (a move, not a collision).
    pub moved: bool,
}

// Odoo versions where several repositories have the name: Odoo loads one
// module per technical name, so only one of them fits an addons path.
#[derive(Debug, Serialize, PartialEq)]
pub struct ModuleCollision {
    /// `org/repo`, the reference first: Odoo core, else who had it first.
    pub locations: Vec<String>,
    pub versions: Vec<String>,
    /// The reference is Odoo core.
    pub core: bool,
}

#[derive(Debug, Serialize, Default)]
pub struct ModuleLocations {
    /// Oldest first.
    pub repos: Vec<ModuleLocation>,
    pub collisions: Vec<ModuleCollision>,
}

fn module_locations(rows: &[models::module::ModuleInfo]) -> ModuleLocations {
    let mut versions: HashMap<(&str, &str), Vec<i32>> = HashMap::new();
    let mut shared: std::collections::BTreeMap<i32, Vec<(&str, &str)>> = Default::default();
    for r in rows {
        let key = (r.organization.as_str(), r.repository.as_str());
        versions.entry(key).or_default().push(r.version_odoo);
        shared.entry(r.version_odoo).or_default().push(key);
    }
    versions.values_mut().for_each(|v| v.sort_unstable());
    let label = |v: &i32| odoo_version_u8_to_string(&(*v as u8));

    let mut keys: Vec<(&str, &str)> = versions.keys().copied().collect();
    keys.sort_by_key(|k| (versions[k][0], *k));
    let mut ended: Option<i32> = None;
    let repos = keys
        .iter()
        .map(|k| {
            let vs = &versions[k];
            let moved = ended.is_some_and(|e| vs[0] > e);
            ended = ended.max(vs.last().copied());
            ModuleLocation {
                org: k.0.to_string(),
                repo: k.1.to_string(),
                versions: vs.iter().map(label).collect(),
                moved,
            }
        })
        .collect();

    let mut collisions: Vec<ModuleCollision> = Vec::new();
    for (v, mut keys) in shared.into_iter().filter(|(_, k)| k.len() > 1) {
        // ponytail: core = org `odoo` (odoo/odoo, design-themes).
        keys.sort_by_key(|k| (k.0 != "odoo", versions[k][0], *k));
        let locations: Vec<String> = keys.iter().map(|(o, r)| format!("{o}/{r}")).collect();
        match collisions.iter_mut().find(|c| c.locations == locations) {
            Some(c) => c.versions.push(label(&v)),
            None => collisions.push(ModuleCollision {
                core: keys[0].0 == "odoo",
                locations,
                versions: vec![label(&v)],
            }),
        }
    }
    ModuleLocations { repos, collisions }
}

// Keyed by Odoo version (e.g. "17.0") for the template's per-tab
// version-history dropdown; shared by `route` (active tab, eager) and
// `route_tab` (one lazily-loaded tab).
fn build_module_context(
    conn: &mut SqliteConnection,
    modules: &[models::module::Model],
    version_module: Option<&str>,
) -> (
    Vec<ModuleFullInfoResponse>,
    HashMap<String, Vec<ModuleVersionHistoryInfo>>,
) {
    let module_versions: HashMap<String, Vec<ModuleVersionHistoryInfo>> = modules
        .iter()
        .map(|m| {
            let odoo_ver = odoo_version_u8_to_string(&(m.version_odoo as u8));
            let mut history: Vec<ModuleVersionHistoryInfo> =
                models::module_version::get_by_module_id(conn, &m.id)
                    .into_iter()
                    .map(|v| ModuleVersionHistoryInfo {
                        is_latest: v.version_module == m.version_module,
                        version_module: v.version_module,
                        create_date: v.create_date,
                    })
                    .collect();
            history.reverse(); // newest first
            (odoo_ver, history)
        })
        .collect();
    let module_infos = process_modules_db(conn, modules, version_module);
    (module_infos, module_versions)
}

#[get("/module/{org}/{technical_name}")]
pub async fn route(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: web::Data<Pool>,
    path: web::Path<(String, String)>,
    info: web::Query<RouteModulePageRequest>,
) -> Result<impl Responder> {
    let (org, technical_name) = path.into_inner();
    let org_ctx = org.clone();
    let technical_name_ctx = technical_name.clone();
    let version_module = info.version.clone();
    let (
        module_infos,
        module_versions,
        (pull_request_count, pull_requests),
        odoo_versions,
        upstream,
        locations,
    ) = web::block(move || {
        let mut conn = pool.get().unwrap();
        let modules = models::module::get_by_technical_name_organization_name(
            &mut conn,
            &technical_name,
            &org,
        );
        let merged_versions: HashSet<i32> = modules.iter().map(|m| m.version_odoo).collect();

        let mut distinct_versions: Vec<i32> = merged_versions.iter().copied().collect();
        distinct_versions.sort_unstable();
        let odoo_versions: Vec<String> = distinct_versions
            .iter()
            .rev() // newest first, matches the tab order
            .map(|v| odoo_version_u8_to_string(&(*v as u8)))
            .collect();

        // Only the newest Odoo version is rendered eagerly here - the rest
        // are fetched lazily by the client on first tab activation (see
        // route_tab below), to avoid computing dependency trees, code
        // analysis and required_by (up to 500 rows) for versions nobody
        // may ever look at.
        let active_modules: Vec<models::module::Model> = match distinct_versions.last() {
            Some(&newest) => modules
                .iter()
                .filter(|m| m.version_odoo == newest)
                .cloned()
                .collect(),
            None => Vec::new(),
        };
        let (module_infos, module_versions) =
            build_module_context(&mut conn, &active_modules, version_module.as_deref());
        let pull_request_count = models::pull_request::count_by_technical_name_organization_name(
            &mut conn,
            &technical_name,
            &org,
        );
        // Lazy (route_pull_requests) unless it's the only tab.
        let pull_requests = match modules.is_empty() {
            true => get_module_pull_requests(&mut conn, &org, &technical_name),
            false => Vec::new(),
        };
        let upstream = get_module_upstream(&mut conn, &technical_name);
        let locations = module_locations(&models::module::get_info(&mut conn, &technical_name));
        (
            module_infos,
            module_versions,
            (pull_request_count, pull_requests),
            odoo_versions,
            upstream,
            locations,
        )
    })
    .await?;

    // Nothing collected under this name here: still a page (it may say what
    // OpenUpgrade renamed it into), but a 404 for crawlers.
    let status = match odoo_versions.is_empty() && pull_request_count == 0 {
        true => StatusCode::NOT_FOUND,
        false => StatusCode::OK,
    };
    Ok(tmpl_env
        .render(
            "pages/module.html",
            context!(
                ..get_minijinja_context(&req),
                ..context!(
                    page_name => "module",
                    org => org_ctx,
                    technical_name => technical_name_ctx,
                    module_infos => module_infos,
                    module_versions => module_versions,
                    pull_request_count => pull_request_count,
                    pull_requests => pull_requests,
                    odoo_versions => odoo_versions,
                    upstream => upstream,
                    locations => locations,
                )
            ),
        )?
        .customize()
        .with_status(status))
}

// The "Pull requests" tab (`partials/module_pull_requests.html`), fetched by
// module.mjs on first activation: see `route`.
#[get("/module/{org}/{technical_name}/pull-requests")]
pub async fn route_pull_requests(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: web::Data<Pool>,
    path: web::Path<(String, String)>,
) -> Result<HttpResponse> {
    let (org, technical_name) = path.into_inner();
    let pull_requests = web::block(move || {
        get_module_pull_requests(&mut pool.get().unwrap(), &org, &technical_name)
    })
    .await?;
    let html = tmpl_env.render(
        "partials/module_pull_requests.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(pull_requests => pull_requests)
        ),
    )?;
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html.0))
}

// Renders the content of a single Odoo-version tab (`partials/module_version_content.html`)
// for `module.mjs` to inject on demand when that tab is first shown - see
// the comment on `route` above for why this isn't computed eagerly for
// every version.
#[get("/module/{org}/{technical_name}/tab/{odoo_version}")]
pub async fn route_tab(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: web::Data<Pool>,
    path: web::Path<(String, String, String)>,
    info: web::Query<RouteModulePageRequest>,
) -> Result<HttpResponse> {
    let (org, technical_name, odoo_version) = path.into_inner();
    let version_odoo = crate::utils::odoo_version(&odoo_version)?;
    let version_module = info.version.clone();
    let (module_infos, module_versions) = web::block(move || {
        let mut conn = pool.get().unwrap();
        let modules = models::module::get_by_technical_name_odoo_version_organization_name(
            &mut conn,
            &technical_name,
            &version_odoo,
            &org,
        );
        build_module_context(&mut conn, &modules, version_module.as_deref())
    })
    .await?;

    let Some(module) = module_infos.into_iter().next() else {
        return Ok(HttpResponse::NotFound().finish());
    };
    let history = module_versions
        .get(&module.odoo_version)
        .cloned()
        .unwrap_or_default();
    let html = tmpl_env.render(
        "partials/module_version_content.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(module => module, history => history)
        ),
    )?;
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html.0))
}

#[derive(Debug, Deserialize)]
pub struct RouteModuleCompareRequest {
    from: Option<String>,
    to: Option<String>,
}

// The two versions to compare: what was asked for when it parses and the
// module has it, otherwise the module's two newest versions (oldest first).
pub(crate) fn pick_compare_versions(
    available_desc: &[i32],
    from: Option<&str>,
    to: Option<&str>,
) -> Option<(i32, i32)> {
    let pick = |raw: Option<&str>, fallback: Option<&i32>| {
        raw.and_then(parse_odoo_version)
            .map(|v| v as i32)
            .filter(|v| available_desc.contains(v))
            .or_else(|| fallback.copied())
    };
    let to = pick(to, available_desc.first())?;
    let from = pick(
        from,
        available_desc
            .iter()
            .find(|v| **v < to)
            .or(available_desc.get(1)),
    )?;
    Some((from, to))
}

/// Side-by-side "what changed" between two Odoo versions of one module.
#[get("/module/{org}/{technical_name}/compare")]
pub async fn route_compare(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: web::Data<Pool>,
    path: web::Path<(String, String)>,
    info: web::Query<RouteModuleCompareRequest>,
) -> Result<impl Responder> {
    let (org, technical_name) = path.into_inner();
    let org_ctx = org.clone();
    let technical_name_ctx = technical_name.clone();
    let info = info.into_inner();
    let (name, versions, picked, diff) = web::block(move || {
        let mut conn = pool.get().unwrap();
        let mut modules = models::module::get_by_technical_name_organization_name(
            &mut conn,
            &technical_name,
            &org,
        );
        modules.sort_by_key(|m| std::cmp::Reverse(m.version_odoo));
        modules.dedup_by_key(|m| m.version_odoo);
        let available: Vec<i32> = modules.iter().map(|m| m.version_odoo).collect();
        let name = modules.first().map(|m| m.name.clone());
        let picked = pick_compare_versions(&available, info.from.as_deref(), info.to.as_deref());
        let diff = picked.and_then(|(from, to)| {
            let old = modules.iter().find(|m| m.version_odoo == from)?;
            let new = modules.iter().find(|m| m.version_odoo == to)?;
            Some(models::module_compare::compare(&mut conn, old, new))
        });
        let versions: Vec<String> = available
            .iter()
            .map(|v| odoo_version_u8_to_string(&(*v as u8)))
            .collect();
        let picked = picked.map(|(from, to)| {
            (
                odoo_version_u8_to_string(&(from as u8)),
                odoo_version_u8_to_string(&(to as u8)),
            )
        });
        (name, versions, picked, diff)
    })
    .await?;

    tmpl_env.render(
        "pages/module_compare.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "module_compare",
                org => org_ctx,
                technical_name => technical_name_ctx,
                module_name => name,
                versions => versions,
                from_version => picked.as_ref().map(|p| p.0.clone()),
                to_version => picked.map(|p| p.1),
                diff => diff,
            )
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::pick_compare_versions;

    #[test]
    fn compare_versions_default_to_the_two_newest_and_ignore_bad_input() {
        let available = [180, 170, 160];
        assert_eq!(
            pick_compare_versions(&available, None, None),
            Some((170, 180))
        );
        assert_eq!(
            pick_compare_versions(&available, Some("16.0"), Some("18.0")),
            Some((160, 180))
        );
        assert_eq!(
            pick_compare_versions(&available, Some("garbage"), Some("17.0")),
            Some((160, 170))
        );
        assert_eq!(
            pick_compare_versions(&available, None, Some("15.0")),
            Some((170, 180))
        );
        assert_eq!(pick_compare_versions(&[180], None, None), None);
    }

    // Real rows: moves between repos (same org or not) are a timeline,
    // shared versions a collision with Odoo core as the reference.
    #[test]
    fn locations_tell_moves_and_collisions() {
        let get = |rows: &[(&str, &str, &[i32])]| {
            let rows: Vec<_> = rows
                .iter()
                .flat_map(|(org, repo, vs)| {
                    vs.iter().map(|v| sqlitedb::models::module::ModuleInfo {
                        technical_name: "m".into(),
                        name: "m".into(),
                        version_odoo: *v,
                        organization: org.to_string(),
                        repository: repo.to_string(),
                        repository_url: None,
                    })
                })
                .collect();
            let l = super::module_locations(&rows);
            let repos: Vec<_> = l
                .repos
                .iter()
                .map(|r| {
                    (
                        format!("{}/{}", r.org, r.repo),
                        r.versions.join(","),
                        r.moved,
                    )
                })
                .collect();
            let collisions: Vec<_> = l
                .collisions
                .iter()
                .map(|c| (c.locations.join(" "), c.versions.join(","), c.core))
                .collect();
            (repos, collisions)
        };
        let r = |name: &str, vs: &str, moved| (name.to_string(), vs.to_string(), moved);
        // auth_totp: OCA → OCA → odoo, no overlap.
        let (repos, collisions) = get(&[
            ("odoo", "odoo", &[140, 150]),
            ("OCA", "server-tools", &[80, 90, 100]),
            ("OCA", "server-auth", &[110, 120]),
        ]);
        assert_eq!(
            repos,
            [
                r("OCA/server-tools", "8.0,9.0,10.0", false),
                r("OCA/server-auth", "11.0,12.0", true),
                r("odoo/odoo", "14.0,15.0", true),
            ]
        );
        assert!(collisions.is_empty());
        // l10n_es: both from 6.1, Odoo core first.
        let (repos, collisions) = get(&[
            ("OCA", "l10n-spain", &[61, 70]),
            ("odoo", "odoo", &[61, 70, 80]),
        ]);
        assert!(!repos[1].2);
        assert_eq!(
            collisions,
            [("odoo/odoo OCA/l10n-spain".into(), "6.1,7.0".into(), true)]
        );
        // No core: who had it first is the reference.
        let (_, collisions) = get(&[("OCA", "mail", &[180]), ("OCA", "social", &[170, 180])]);
        assert_eq!(
            collisions,
            [("OCA/social OCA/mail".into(), "18.0".into(), false)]
        );
        let (repos, _) = get(&[("OCA", "web", &[170])]);
        assert_eq!(repos, [r("OCA/web", "17.0", false)]);
    }

    // Both directions, chains followed; the other module links only when
    // it's collected.
    #[test]
    fn upstream_tells_renames_and_merges_both_ways() {
        let (db, pool) = sqlitedb::fixture::demo("module-upstream");
        let mut write = sqlitedb::new_write_pool(&db.path).get().unwrap();
        let row = |old: &str, new: &str, merged| (old.to_string(), new.to_string(), merged);
        for (v, rows) in [
            (170, vec![row("web_widget_old", "web_widget_demo", false)]),
            (
                180,
                vec![
                    row("web_widget_demo", "sale", true),
                    row("sale_tip", "sale", true),
                ],
            ),
        ] {
            sqlitedb::models::openupgrade_rename::replace_for_version(&mut write, v, &rows)
                .unwrap();
        }
        let mut conn = pool.get().unwrap();
        let tuple =
            |u: &super::ModuleUpstreamInfo| (u.kind, u.name.clone(), u.org.clone(), u.via.clone());
        let get = |conn: &mut _, name| {
            let u = super::get_module_upstream(conn, name);
            let became: Vec<Vec<_>> = u
                .became
                .iter()
                .map(|chain| chain.iter().map(tuple).collect())
                .collect();
            let replaces: Vec<(String, Vec<_>)> = u
                .replaces
                .iter()
                .map(|(v, infos)| (v.clone(), infos.iter().map(tuple).collect()))
                .collect();
            (became, replaces, u.replaced)
        };
        let some = |s: &str| Some(s.to_string());
        let step = |kind, name: &str, org: Option<String>, via: Option<String>| {
            (kind, name.to_string(), org, via)
        };
        // A → B (17.0) → C (18.0), from each end and the middle.
        let (became, replaces, _) = get(&mut conn, "web_widget_old");
        assert_eq!(
            became,
            [vec![
                step("renamed", "web_widget_demo", some("OCA"), None),
                step("merged", "sale", some("odoo"), None),
            ]]
        );
        assert!(replaces.is_empty());
        let (became, replaces, _) = get(&mut conn, "web_widget_demo");
        assert_eq!(became, [vec![step("merged", "sale", some("odoo"), None)]]);
        assert_eq!(
            replaces,
            [(
                "17.0".into(),
                vec![step("renamed", "web_widget_old", None, None)]
            )]
        );
        let (became, replaces, replaced) = get(&mut conn, "sale");
        assert!(became.is_empty());
        assert_eq!(
            replaces,
            [
                (
                    "17.0".into(),
                    vec![step(
                        "renamed",
                        "web_widget_old",
                        None,
                        some("web_widget_demo")
                    )]
                ),
                (
                    "18.0".into(),
                    vec![
                        step("merged", "sale_tip", None, None),
                        step("merged", "web_widget_demo", some("OCA"), None),
                    ]
                ),
            ]
        );
        assert_eq!(replaced, 3);
    }
}

#[cfg(test)]
mod template_tests {
    use minijinja::{context, Environment};
    use serde_json::json;

    #[test]
    fn module_source_links_follow_the_scanned_directory() {
        let mut env = Environment::new();
        env.add_filter("markdown", crate::utils::markdown);
        env.add_filter("repo_files", |git: String| oghutils::git::files_url(&git));
        env.add_template(
            "module",
            include_str!("../../../../web/templates/partials/module_version_content.html"),
        )
        .unwrap();
        env.add_template(
            "overview",
            include_str!("../../../../web/templates/partials/modules_version_content.html"),
        )
        .unwrap();
        for (org, repo, path) in [
            ("OCA", "web", "web_responsive"),
            ("odoo", "odoo", "addons/web"),
            ("odoo", "odoo", "odoo/addons/base"),
        ] {
            let module = json!({
                "git": format!("https://github.com/{org}/{repo}.git"),
                "organization": org,
                "repository": repo,
                "odoo_version": "18.0",
                "source_path": path,
                "description": "## Usage\n\n<b>x</b>\n\n![s](../static/description/s.png)",
                "folder_size": 0,
                "security_warnings": [{"file": "models/test.py", "line": 42, "code": "TEST"}],
                "security_warning_count": 0,
                "migration_considerations": [],
                "dependencies": {"odoo": {}, "pip": [], "bin": []},
                "required_by": [],
                "license": "AGPL-3",
                "license_review": {
                    "effective_license": "AGPL-3",
                    "effective_license_source": "web",
                    "proprietary": [],
                    "licenses": {"AGPL-3": 1},
                    "findings": [{"severity": "warning", "code": "LICENSE_X", "message": "msg", "module": "web", "related": []}],
                },
                "views": [],
                "controllers": [],
                "models": [],
                "health": {"score": 62, "grade": "fair", "factors": [
                    {"name": "Security", "points": null, "detail": "No code analysis"},
                    {"name": "Activity", "points": 20, "detail": "Last human commit 8 months ago"},
                ]},
            });
            let html = env
                .get_template("module")
                .unwrap()
                .render(context!(module => module, history => Vec::<String>::new(), HEALTH_SCORE_ENABLED => true))
                .unwrap();
            assert!(html.contains("<h2>Usage</h2>") && html.contains("&lt;b&gt;x"));
            assert!(html.contains(&format!(
                "https://github.com/{org}/{repo}/raw/18.0/{path}/static/description/s.png"
            )));
            assert!(html.contains(">62</span>"));
            assert!(html.contains("<span>Security</span><span>n/a</span>"));
            assert!(html.contains("<span>Activity</span><span>20<small>/25</small>"));
            assert!(html.contains("Last human commit 8 months ago"));
            assert!(html.contains(&format!("https://github.com/{org}/{repo}/tree/18.0/{path}")));
            assert!(html.contains(&format!(
                "https://github.com/{org}/{repo}/blob/18.0/{path}/models/test.py#L42"
            )));
            let group = json!({
                "odoo_version": "18.0",
                "pull_requests": [],
                "avg_days_open": null,
                "security_errors": [{
                    "organization": org,
                    "repository": repo,
                    "technical_name": "web",
                    "source_path": path,
                    "files_url": format!("https://github.com/{org}/{repo}"),
                    "file": "models/test.py",
                    "line": 42,
                    "code": "TEST",
                    "message": "test"
                }],
                "security_warnings": [],
                "migration_warnings": [],
                "migration_infos": [],
            });
            let overview = env
                .get_template("overview")
                .unwrap()
                .render(context!(g => group))
                .unwrap();
            assert!(overview.contains(&format!(
                "https://github.com/{org}/{repo}/blob/18.0/{path}/models/test.py#L42"
            )));
        }
    }

    #[test]
    fn pull_requests_escape_what_prs_bring() {
        let mut env = Environment::new();
        env.add_template(
            "prs.html",
            include_str!("../../../../web/templates/partials/module_pull_requests.html"),
        )
        .unwrap();
        let xss = "<script>x</script>";
        let pr = json!({
            "title": xss, "prid": 7, "odoo_version": "18.0", "repository": "web",
            "organization": "OCA", "url": "https://github.com/OCA/web/pull/7",
            "author": xss, "age_days": 1, "last_message_days": 0, "ci_status": "success",
            "is_draft": false, "labels": [xss], "is_migration": true, "preferred": true,
            "version": "18.0.1.0.0", "grave": 1, "grave_new": 1, "minor": 0, "notes": 0,
            "analysis": {"security": [{"severity": "error", "new": true, "message": xss,
                                       "file": "a.py", "line": 3}],
                         "migration_notes": []},
        });
        let html = env
            .get_template("prs.html")
            .unwrap()
            .render(context!(pull_requests => vec![pr]))
            .unwrap();
        assert!(!html.contains("<script>"));
        assert!(html.contains("best for 18.0") && html.contains("1 of 1"));
        assert!(html.contains("1 grave finding (1 new)"));
        assert!(html.contains(r#"data-ci="success" data-kind="migration""#));
    }

    #[test]
    fn module_links_use_gitlab_routes_for_gitlab_repos() {
        let mut env = Environment::new();
        env.add_filter("markdown", crate::utils::markdown);
        env.add_filter("repo_files", |git: String| oghutils::git::files_url(&git));
        env.add_template(
            "module",
            include_str!("../../../../web/templates/partials/module_version_content.html"),
        )
        .unwrap();
        let module = json!({
            "git": "https://gitlab.com/acme/erp/sales.git",
            "organization": "acme",
            "repository": "sales",
            "odoo_version": "18.0",
            "source_path": "sale_x",
            "description": "![s](../static/description/s.png)",
            "security_warnings": [{"file": "models/a.py", "line": 7, "code": "TEST"}],
            "security_warning_count": 0,
            "migration_considerations": [],
            "dependencies": {"odoo": {}, "pip": [], "bin": []},
            "required_by": [],
            "views": [],
            "controllers": [],
            "models": [],
            "folder_size": 0,
            "license_review": {"licenses": {}, "proprietary": [], "findings": []},
        });
        let html = env
            .get_template("module")
            .unwrap()
            .render(context!(module => module, history => Vec::<String>::new()))
            .unwrap();
        let base = "https://gitlab.com/acme/erp/sales/-";
        assert!(html.contains(&format!("{base}/raw/18.0/sale_x/static/description/s.png")));
        assert!(html.contains(&format!("{base}/tree/18.0/sale_x")));
        assert!(html.contains(&format!("{base}/blob/18.0/sale_x/models/a.py#L7")));
        assert!(!html.contains("github.com"));
    }
}
