// Copyright Alexandre D. Díaz
//! Doodba projects the collector detected (`collector/src/doodba.rs`): one
//! `doodba_project` row per repository and Odoo version, its JSON-text lists
//! read whole, plus the addons it enables (`doodba_project_addon`), joined by
//! technical name. Listings join `gh_repository`, so a workspace pool only
//! sees its own projects. `Summary`/`Report` are the parsed views `/v1` and
//! MCP answer with.
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Integer, Nullable, Text};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

use oghutils::version::odoo_version_u8_to_string;

use crate::schema::{doodba_project, doodba_project_addon};

#[derive(Queryable, QueryableByName, Selectable, Insertable, Debug, Clone, Serialize, Default)]
#[diesel(table_name = doodba_project)]
pub struct Model {
    pub gh_repository_id: i64,
    pub version_odoo: i32,
    pub last_commit_date: String,
    pub template_src: Option<String>,
    pub template_commit: Option<String>,
    pub template_commit_date: Option<String>,
    pub template_behind: Option<i32>,
    pub template_log: String,
    pub answers: String,
    pub base_image: Option<String>,
    pub repos: String,
    pub linters: String,
    pub dependencies: String,
    pub exposed: String,
    pub advisories: String,
    pub deploy: String,
}

/// An entry of the `repos` JSON (repos.yaml): `patches` are its merges after
/// the first one, what git-aggregator merges on top of the base branch.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct Repo {
    pub path: String,
    pub target: Option<String>,
    pub remotes: BTreeMap<String, String>,
    pub patches: Vec<Patch>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct Patch {
    pub remote: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// Its PR/MR's `open`/`closed`/`merged`; `None` = not a PR/MR, or not
    /// asked (another forge, no token).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// An entry of the `linters` JSON (.pre-commit-config.yaml hooks with a rev).
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct Linter {
    pub repo: String,
    pub rev: String,
}

/// The `deploy` JSON: the project's real instances, from its own
/// `.gitlab-ci.yml` `variables` (the included CI template isn't read: its
/// defaults, prod on and demo off, apply when they're unset) and the hosts of
/// `.copier-answers.yml` `domains_prod`/`domains_test`. A URL is only set for
/// an environment that is deployed.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
#[serde(default)]
pub struct Deploy {
    pub gitlab_ci: bool,
    pub prod: bool,
    /// `None` = the default branch.
    pub prod_branch: Option<String>,
    pub prod_url: Option<String>,
    pub demo: bool,
    pub demo_branch: Option<String>,
    /// The demo is refilled from prod's database and filestore.
    pub demo_clone_prod: bool,
    pub demo_reset_db: bool,
    pub demo_name: Option<String>,
    pub demo_url: Option<String>,
}

/// An entry of the `advisories` JSON: a PyPI advisory of an exact pip pin.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct Advisory {
    pub package: String,
    pub version: String,
    pub id: String,
    pub fixed_in: String,
    pub summary: String,
}

/// What a job stored for a repository at a version.
pub fn find(
    conn: &mut SqliteConnection,
    gh_repository_id: i64,
    version_odoo: i32,
) -> Option<Model> {
    doodba_project::table
        .find((gh_repository_id, version_odoo))
        .select(Model::as_select())
        .first(conn)
        .optional()
        .expect("DB error in doodba_project::find")
}

/// Replaces what's stored for a repository at a version: `project` and its
/// `addons` (repos.yaml repo, technical name), or nothing when it's `None`
/// (the branch isn't a doodba project, or no longer is).
pub fn replace(
    conn: &mut SqliteConnection,
    gh_repository_id: i64,
    version_odoo: i32,
    project: Option<(&Model, &[(String, String)])>,
) -> QueryResult<()> {
    conn.transaction(|conn| {
        diesel::delete(
            doodba_project::table
                .filter(doodba_project::gh_repository_id.eq(gh_repository_id))
                .filter(doodba_project::version_odoo.eq(version_odoo)),
        )
        .execute(conn)?;
        diesel::delete(
            doodba_project_addon::table
                .filter(doodba_project_addon::gh_repository_id.eq(gh_repository_id))
                .filter(doodba_project_addon::version_odoo.eq(version_odoo)),
        )
        .execute(conn)?;
        let Some((project, addons)) = project else {
            return Ok(());
        };
        diesel::insert_into(doodba_project::table)
            .values(project)
            .execute(conn)?;
        for (repo, technical_name) in addons {
            diesel::insert_or_ignore_into(doodba_project_addon::table)
                .values((
                    doodba_project_addon::gh_repository_id.eq(gh_repository_id),
                    doodba_project_addon::version_odoo.eq(version_odoo),
                    doodba_project_addon::repo.eq(repo),
                    doodba_project_addon::technical_name.eq(technical_name),
                ))
                .execute(conn)?;
        }
        Ok(())
    })
}

/// A project with its repository: what the pages list and link.
#[derive(QueryableByName, Debug, Clone, Serialize)]
pub struct Row {
    #[diesel(embed)]
    pub project: Model,
    #[diesel(sql_type = Text)]
    pub org_name: String,
    #[diesel(sql_type = Text)]
    pub repository: String,
    #[diesel(sql_type = Nullable<Text>)]
    pub url: Option<String>,
    #[diesel(sql_type = BigInt)]
    pub addon_count: i64,
}

const ROW_SQL: &str = "SELECT p.*, o.name AS org_name, r.name AS repository, r.url,
    (SELECT COUNT(*) FROM doodba_project_addon a
     WHERE a.gh_repository_id = p.gh_repository_id AND a.version_odoo = p.version_odoo) AS addon_count
    FROM doodba_project p
    JOIN gh_repository r ON r.id = p.gh_repository_id
    JOIN gh_organization o ON o.id = r.gh_organization_id";

/// Sort key of a rev: its numbers (`v8.0.20` → [8, 0, 20]); a commit hash
/// sorts below any tag.
pub fn rev_key(rev: &str) -> Vec<u64> {
    if rev.len() >= 7 && rev.chars().all(|c| c.is_ascii_hexdigit()) {
        return Vec::new();
    }
    rev.split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse().ok())
        .collect()
}

/// The newest rev of each hook the fleet runs, per Odoo version; among
/// revs that don't order (commit hashes), the most used one.
pub fn newest_revs(rows: &[Row]) -> HashMap<(String, i32), String> {
    let mut counts: HashMap<(String, i32, String), usize> = HashMap::new();
    for row in rows {
        for lint in row.linters() {
            *counts
                .entry((lint.repo, row.project.version_odoo, lint.rev))
                .or_default() += 1;
        }
    }
    let mut out: HashMap<(String, i32), (Vec<u64>, usize, String)> = HashMap::new();
    for ((repo, version_odoo, rev), count) in counts {
        let candidate = (rev_key(&rev), count, rev);
        let best = out.entry((repo, version_odoo)).or_insert(candidate.clone());
        if candidate > *best {
            *best = candidate;
        }
    }
    out.into_iter().map(|(k, (_, _, rev))| (k, rev)).collect()
}

/// Every project the connection sees, by organization, repository, version.
pub fn list(conn: &mut SqliteConnection) -> Vec<Row> {
    diesel::sql_query(format!(
        "{ROW_SQL} ORDER BY o.name, r.name, p.version_odoo DESC"
    ))
    .load(conn)
    .expect("DB error in doodba_project::list")
}

pub fn get(
    conn: &mut SqliteConnection,
    org: &str,
    repository: &str,
    version_odoo: i32,
) -> Option<Row> {
    diesel::sql_query(format!(
        "{ROW_SQL} WHERE o.name = ? AND r.name = ? AND p.version_odoo = ?"
    ))
    .bind::<Text, _>(org)
    .bind::<Text, _>(repository)
    .bind::<Integer, _>(version_odoo)
    .get_result(conn)
    .optional()
    .expect("DB error in doodba_project::get")
}

/// Projects enabling the module `technical_name` (of `module_org`, else of
/// any organization), at whatever version: from a repos.yaml repo named like
/// the module's repository (`*` = all of its addons), or as a `private` addon
/// when the module is in the project's own repository. The module side reads
/// `main.*`, which a workspace pool doesn't shadow: its projects are listed
/// whatever workspace the module is in.
/// ponytail: by name, so a fork named like the upstream repo (`web`) counts
/// for both; resolving each repos.yaml remote would tell them apart.
pub fn using_module(
    conn: &mut SqliteConnection,
    technical_name: &str,
    module_org: Option<&str>,
) -> Vec<Row> {
    diesel::sql_query(format!(
        "{ROW_SQL} WHERE EXISTS (
            SELECT 1 FROM doodba_project_addon a
            JOIN main.module m ON m.technical_name = ?1 AND m.version_odoo = a.version_odoo
            JOIN main.gh_repository mr ON mr.id = m.gh_repository_id
            JOIN main.gh_organization mo ON mo.id = mr.gh_organization_id
            WHERE a.gh_repository_id = p.gh_repository_id AND a.version_odoo = p.version_odoo
            AND a.technical_name IN (?1, '*') AND (?2 IS NULL OR mo.name = ?2)
            AND CASE WHEN a.repo = 'private' THEN m.gh_repository_id = a.gh_repository_id
                ELSE mr.name = a.repo END)
         ORDER BY p.version_odoo DESC, o.name, r.name"
    ))
    .bind::<Text, _>(technical_name)
    .bind::<Nullable<Text>, _>(module_org)
    .load(conn)
    .expect("DB error in doodba_project::using_module")
}

/// An enabled addon of one project, for fleet-wide "who shares what".
#[derive(QueryableByName, Debug, Clone, Serialize)]
pub struct SharedAddon {
    #[diesel(sql_type = Text)]
    pub repo: String,
    #[diesel(sql_type = Text)]
    pub technical_name: String,
    #[diesel(sql_type = Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = Text)]
    pub org_name: String,
    #[diesel(sql_type = Text)]
    pub repository: String,
}

/// Every named, non-private addon of every project the pool sees.
pub fn all_addons(conn: &mut SqliteConnection) -> Vec<SharedAddon> {
    diesel::sql_query(
        "SELECT a.repo, a.technical_name, a.version_odoo, o.name AS org_name, r.name AS repository
         FROM doodba_project_addon a
         JOIN gh_repository r ON r.id = a.gh_repository_id
         JOIN gh_organization o ON o.id = r.gh_organization_id
         WHERE a.repo != 'private' AND a.technical_name != '*'",
    )
    .load(conn)
    .expect("DB error in doodba_project::all_addons")
}

impl Row {
    pub fn repos(&self) -> Vec<Repo> {
        serde_json::from_str(&self.project.repos).unwrap_or_default()
    }

    pub fn linters(&self) -> Vec<Linter> {
        serde_json::from_str(&self.project.linters).unwrap_or_default()
    }

    pub fn advisories(&self) -> Vec<Advisory> {
        serde_json::from_str(&self.project.advisories).unwrap_or_default()
    }

    /// Credentials outside repos.yaml, where a shared read token is the norm.
    pub fn exposed_elsewhere(&self) -> Vec<String> {
        let mut e = self.exposed();
        e.retain(|e| !e.starts_with("odoo/custom/src/repos.yaml "));
        e
    }

    pub fn deploy(&self) -> Deploy {
        serde_json::from_str(&self.project.deploy).unwrap_or_default()
    }

    pub fn exposed(&self) -> Vec<String> {
        serde_json::from_str(&self.project.exposed).unwrap_or_default()
    }

    pub fn odoo_version(&self) -> String {
        odoo_version_u8_to_string(&(self.project.version_odoo as u8))
    }

    pub fn summary(&self) -> Summary {
        let patches: Vec<Patch> = self.repos().into_iter().flat_map(|r| r.patches).collect();
        let deploy = self.deploy();
        Summary {
            organization: self.org_name.clone(),
            repository: self.repository.clone(),
            odoo_version: self.odoo_version(),
            last_commit_date: self.project.last_commit_date.clone(),
            template_commit: self.project.template_commit.clone(),
            template_behind: self.project.template_behind,
            addons: self.addon_count,
            patches_done: patches.iter().filter(|p| p.is_done()).count(),
            patches: patches.len(),
            advisories: self.advisories().len(),
            exposed_credentials: self.exposed().len(),
            prod_url: deploy.prod_url,
            demo_url: deploy.demo_url,
        }
    }
}

impl Patch {
    /// The PR/MR number of a `refs/pull/N/...` or `refs/merge-requests/N/...` ref.
    pub fn number(&self) -> Option<i64> {
        self.git_ref
            .strip_prefix("refs/pull/")
            .or_else(|| self.git_ref.strip_prefix("refs/merge-requests/"))?
            .split('/')
            .next()?
            .parse()
            .ok()
    }

    /// Its PR/MR is merged or closed: the patch can go (or needs another).
    pub fn is_done(&self) -> bool {
        matches!(self.state.as_deref(), Some("merged" | "closed"))
    }
}

/// A project in a listing (`/v1/projects`, MCP `list_doodba_projects`).
#[derive(Serialize, Debug)]
pub struct Summary {
    pub organization: String,
    pub repository: String,
    pub odoo_version: String,
    pub last_commit_date: String,
    pub template_commit: Option<String>,
    /// Template commits since `template_commit`; `None` = unknown.
    pub template_behind: Option<i32>,
    pub addons: i64,
    pub patches: usize,
    /// Patches whose PR/MR is merged or closed.
    pub patches_done: usize,
    pub advisories: usize,
    pub exposed_credentials: usize,
    pub prod_url: Option<String>,
    pub demo_url: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct ReportAddon {
    pub repo: String,
    pub technical_name: String,
    /// Organization of the module it resolves to; `None` = not collected.
    pub organization: Option<String>,
    pub in_next_version: bool,
}

/// One project whole (`/v1/project/...`, MCP `get_doodba_project`).
#[derive(Serialize, Debug)]
pub struct Report {
    pub organization: String,
    pub repository: String,
    pub odoo_version: String,
    pub last_commit_date: String,
    pub template_src: Option<String>,
    pub template_commit: Option<String>,
    pub template_commit_date: Option<String>,
    pub template_behind: Option<i32>,
    /// "<hash> <subject>" of the template commits not applied, newest first (capped).
    pub template_pending: Vec<String>,
    pub base_image: Option<String>,
    pub answers: serde_json::Value,
    pub repos: Vec<Repo>,
    pub linters: Vec<Linter>,
    pub dependencies: BTreeMap<String, Vec<String>>,
    pub advisories: Vec<Advisory>,
    /// Its production and demo instances (CI switches, demo options, URLs).
    pub deploy: Deploy,
    /// Where the project files hold credentials ("<file> <host>").
    pub exposed_credentials: Vec<String>,
    /// The next indexed Odoo version, which `in_next_version` is about.
    pub next_odoo_version: Option<String>,
    pub addons: Vec<ReportAddon>,
}

pub fn report(conn: &mut SqliteConnection, row: &Row) -> Report {
    let p = &row.project;
    let next = super::module::get_odoo_versions(conn)
        .into_iter()
        .filter(|v| *v > p.version_odoo)
        .min();
    Report {
        organization: row.org_name.clone(),
        repository: row.repository.clone(),
        odoo_version: row.odoo_version(),
        last_commit_date: p.last_commit_date.clone(),
        template_src: p.template_src.clone(),
        template_commit: p.template_commit.clone(),
        template_commit_date: p.template_commit_date.clone(),
        template_behind: p.template_behind,
        template_pending: serde_json::from_str(&p.template_log).unwrap_or_default(),
        base_image: p.base_image.clone(),
        answers: serde_json::from_str(&p.answers).unwrap_or_default(),
        repos: row.repos(),
        linters: row.linters(),
        dependencies: serde_json::from_str(&p.dependencies).unwrap_or_default(),
        advisories: row.advisories(),
        deploy: row.deploy(),
        exposed_credentials: row.exposed(),
        next_odoo_version: next.map(|v| odoo_version_u8_to_string(&(v as u8))),
        addons: addons(conn, p.gh_repository_id, p.version_odoo, next)
            .into_iter()
            .map(|a| ReportAddon {
                repo: a.repo,
                technical_name: a.technical_name,
                organization: a.org_name,
                in_next_version: a.in_next != 0,
            })
            .collect(),
    }
}

/// An enabled addon and where it stands: the organization of the module
/// row it resolves to at the project's version (`None` = not collected), and
/// whether it exists at `next_version` under its name or one OpenUpgrade
/// renamed/merged it into. `private` addons (`odoo/custom/src/private`) only
/// ever resolve in the project's own repository: in a group of client
/// projects, another project's private module of the same name isn't it.
/// The others prefer the repository named like their addons.yaml key.
#[derive(QueryableByName, Debug, Clone, Serialize)]
pub struct AddonRow {
    #[diesel(sql_type = Text)]
    pub repo: String,
    #[diesel(sql_type = Text)]
    pub technical_name: String,
    #[diesel(sql_type = Nullable<Text>)]
    pub org_name: Option<String>,
    #[diesel(sql_type = Integer)]
    pub in_next: i32,
}

/// Whether any repository called `repo` has modules collected at `version_odoo`.
pub fn repo_has_modules(conn: &mut SqliteConnection, repo: &str, version_odoo: i32) -> bool {
    #[derive(QueryableByName)]
    struct Count {
        #[diesel(sql_type = BigInt)]
        n: i64,
    }
    let found: Vec<Count> = diesel::sql_query(
        "SELECT COUNT(*) AS n FROM module m JOIN gh_repository r ON r.id = m.gh_repository_id
         WHERE r.name = ?1 AND m.version_odoo = ?2",
    )
    .bind::<Text, _>(repo)
    .bind::<Integer, _>(version_odoo)
    .load(conn)
    .expect("DB error in doodba_project::repo_has_modules");
    found.first().is_some_and(|c| c.n > 0)
}

pub fn addons(
    conn: &mut SqliteConnection,
    gh_repository_id: i64,
    version_odoo: i32,
    next_version: Option<i32>,
) -> Vec<AddonRow> {
    diesel::sql_query(
        "SELECT a.repo, a.technical_name,
            CASE WHEN a.repo = 'private' THEN
                (SELECT o.name FROM gh_repository r
                 JOIN gh_organization o ON o.id = r.gh_organization_id
                 WHERE r.id = a.gh_repository_id AND EXISTS (
                     SELECT 1 FROM module m WHERE m.gh_repository_id = r.id
                     AND m.technical_name = a.technical_name AND m.version_odoo = a.version_odoo))
            -- SQLite rejects outer columns in a subquery's ORDER BY: two lookups.
            ELSE COALESCE(
                (SELECT o.name FROM module m
                 JOIN gh_repository r ON r.id = m.gh_repository_id
                 JOIN gh_organization o ON o.id = r.gh_organization_id
                 WHERE m.technical_name = a.technical_name AND m.version_odoo = a.version_odoo
                 AND r.name = a.repo LIMIT 1),
                (SELECT o.name FROM module m
                 JOIN gh_repository r ON r.id = m.gh_repository_id
                 JOIN gh_organization o ON o.id = r.gh_organization_id
                 WHERE m.technical_name = a.technical_name AND m.version_odoo = a.version_odoo
                 ORDER BY r.id LIMIT 1)) END AS org_name,
            (?3 IS NOT NULL AND CASE WHEN a.repo = 'private' THEN
                EXISTS (SELECT 1 FROM module m WHERE m.gh_repository_id = a.gh_repository_id
                        AND m.technical_name = a.technical_name AND m.version_odoo = ?3)
            ELSE (
                EXISTS (SELECT 1 FROM module m
                        WHERE m.technical_name = a.technical_name AND m.version_odoo = ?3)
                OR EXISTS (SELECT 1 FROM openupgrade_rename u
                           WHERE u.old_name = a.technical_name
                           AND u.version_odoo > a.version_odoo AND u.version_odoo <= ?3)) END) AS in_next
         FROM doodba_project_addon a
         WHERE a.gh_repository_id = ?1 AND a.version_odoo = ?2
         ORDER BY a.repo, a.technical_name",
    )
    .bind::<BigInt, _>(gh_repository_id)
    .bind::<Integer, _>(version_odoo)
    .bind::<Nullable<Integer>, _>(next_version)
    .load(conn)
    .expect("DB error in doodba_project::addons")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{gh_organization, gh_repository, module};
    use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};

    const MIGRATIONS: EmbeddedMigrations = embed_migrations!("../../migrations");

    fn add_module(conn: &mut SqliteConnection, name: &str, org: &str, repo: &str, version: u8) {
        module::add(
            conn,
            &module::ManifestInfo {
                technical_name: name.to_string(),
                source_path: name.to_string(),
                version_odoo: version,
                name: name.to_string(),
                version_module: "1.0.0".to_string(),
                installable: true,
                git_org: org.to_string(),
                git_repo: repo.to_string(),
                last_commit_hash: "abc".to_string(),
                last_commit_date: "2024-01-01".to_string(),
                ..Default::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn private_addons_only_resolve_in_their_own_project() {
        let mut conn = SqliteConnection::establish(":memory:").unwrap();
        conn.run_pending_migrations(MIGRATIONS).unwrap();
        let conn = &mut conn;
        // Another client's project of the same group has a private module
        // of the same name, at both versions.
        for version in [160, 170] {
            add_module(conn, "iso_custom", "isonor", "client_b", version);
            add_module(conn, "web_responsive", "OCA", "web", version);
        }
        let org = gh_organization::add(conn, "isonor").unwrap();
        let project = gh_repository::add(conn, &org.id, "client_a").unwrap();
        add_module(conn, "base_technical_user", "OCA", "server-tools", 160);
        let addon_rows = [
            ("private".to_string(), "iso_custom".to_string()),
            ("web".to_string(), "web_responsive".to_string()),
            ("server-tools".to_string(), "*".to_string()),
        ];
        let model = Model {
            gh_repository_id: project.id,
            version_odoo: 160,
            ..Default::default()
        };
        replace(conn, project.id, 160, Some((&model, &addon_rows))).unwrap();
        let found = |conn: &mut SqliteConnection| {
            addons(conn, project.id, 160, Some(170))
                .into_iter()
                .map(|a| (a.technical_name, a.org_name, a.in_next))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            found(conn),
            [
                ("iso_custom".into(), None, 0),
                ("*".into(), None, 0),
                ("web_responsive".into(), Some("OCA".into()), 1),
            ]
        );
        let users = |conn: &mut SqliteConnection, name: &str, org: Option<&str>| {
            using_module(conn, name, org)
                .into_iter()
                .map(|r| format!("{}/{} {}", r.org_name, r.repository, r.project.version_odoo))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            users(conn, "web_responsive", Some("OCA")),
            ["isonor/client_a 160"]
        );
        assert!(users(conn, "web_responsive", Some("isonor")).is_empty());
        // `server-tools: "*"`.
        assert_eq!(
            users(conn, "base_technical_user", None),
            ["isonor/client_a 160"]
        );
        assert!(
            users(conn, "iso_custom", None).is_empty(),
            "client_b's isn't its"
        );
        // Its own private module, not migrated to 17.0 in its repository.
        add_module(conn, "iso_custom", "isonor", "client_a", 160);
        assert_eq!(
            found(conn)[0],
            ("iso_custom".into(), Some("isonor".into()), 0)
        );
        assert_eq!(users(conn, "iso_custom", None), ["isonor/client_a 160"]);
    }
}
