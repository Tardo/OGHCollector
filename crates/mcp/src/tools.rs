// Copyright Alexandre D. Díaz
use std::collections::HashMap;

use cached::{proc_macro::cached, stores::TimedSizedCache};
use diesel::sqlite::SqliteConnection;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{
        CallToolResult, ContentBlock, Implementation, ProtocolVersion, ServerCapabilities,
        ServerInfo,
    },
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};
use serde::{Deserialize, Serialize};

use oghutils::version::{odoo_version_string_to_u8, odoo_version_u8_to_string};
use sqlitedb::{models, Pool};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchModulesParams {
    /// Substring to match against a module's technical name.
    pub name: String,
    /// Restrict to a specific Odoo version, e.g. "17.0".
    pub odoo_version: Option<String>,
    /// Restrict to installable (true) or non-installable (false) modules.
    pub installable: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ModuleSearchResult {
    pub technical_name: String,
    pub versions: HashMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetModuleParams {
    /// Module technical name, e.g. "sale_order_type".
    pub technical_name: String,
    /// Odoo version, e.g. "17.0".
    pub odoo_version: String,
    /// Restrict to a GitHub/GitLab organization name.
    pub org: Option<String>,
    /// Restrict to a repository name.
    pub repo: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetModuleCodeAnalysisParams {
    /// Module technical name, e.g. "sale_order_type".
    pub technical_name: String,
    /// Odoo version, e.g. "17.0".
    pub odoo_version: String,
    /// Restrict to a GitHub/GitLab organization name.
    pub org: Option<String>,
    /// Restrict to a repository name.
    pub repo: Option<String>,
    /// Specific module version to inspect, e.g. "1.0.2" (see
    /// `list_module_versions`). Defaults to the latest known version.
    pub version_module: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListModuleVersionsParams {
    /// Module technical name, e.g. "sale_order_type".
    pub technical_name: String,
    /// Odoo version, e.g. "17.0".
    pub odoo_version: String,
    /// Restrict to a GitHub/GitLab organization name.
    pub org: Option<String>,
    /// Restrict to a repository name.
    pub repo: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleVersionInfo {
    pub version_module: String,
    pub create_date: String,
    pub update_date: String,
    pub is_latest: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleVersionHistory {
    pub organization: String,
    pub repository: String,
    pub versions: Vec<ModuleVersionInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleView {
    pub xml_id: String,
    pub name: String,
    pub model: String,
    pub inherit_xml_id: Option<String>,
    pub is_new: bool,
    pub view_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleModelField {
    pub name: String,
    pub field_type: String,
    pub relation: Option<String>,
    pub attrs: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleModelMethod {
    pub name: String,
    pub decorators: Vec<String>,
    pub signature: String,
    pub docstring: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleModel {
    pub model_name: String,
    pub class_name: String,
    pub inherit_from: Vec<String>,
    pub is_new_model: bool,
    pub docstring: Option<String>,
    pub attrs: Option<serde_json::Value>,
    pub fields: Vec<ModuleModelField>,
    pub methods: Vec<ModuleModelMethod>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleDependencies {
    pub odoo: HashMap<String, Vec<String>>,
    pub pip: Vec<String>,
    pub bin: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleInfo {
    pub technical_name: String,
    pub name: String,
    pub odoo_version: String,
    pub module_version: String,
    pub description: String,
    pub authors: Vec<String>,
    pub maintainers: Vec<String>,
    pub license: String,
    pub category: String,
    pub application: bool,
    pub installable: bool,
    pub auto_install: bool,
    pub organization: String,
    pub repository: String,
    pub git: String,
    /// Date of the last git commit that touched this module, e.g.
    /// "2026-01-15 10:32:00". Combined with `last_commit_author`, use this to
    /// gauge whether a module is still actively maintained before
    /// recommending it for a pack.
    pub last_commit_date: String,
    pub last_commit_author: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleDocs {
    pub technical_name: String,
    pub odoo_version: String,
    pub organization: String,
    pub repository: String,
    /// Rendered `readme/INSTALL.md`, if the module has one - install-specific
    /// steps beyond `pip install`/adding to `depends` (system packages,
    /// config, etc.).
    pub installation: String,
    /// Rendered `readme/USAGE.md`, if the module has one - how to use the
    /// module once installed (menus, workflow, etc.).
    pub usage: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleDependencyInfo {
    pub technical_name: String,
    pub odoo_version: String,
    pub organization: String,
    pub repository: String,
    pub dependencies: ModuleDependencies,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleCodeAnalysis {
    pub technical_name: String,
    pub odoo_version: String,
    pub module_version: String,
    pub organization: String,
    pub repository: String,
    pub views: Vec<ModuleView>,
    pub models: Vec<ModuleModel>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchRepositoriesParams {
    /// Substring to match against a repository's name (SQL LIKE, \
    /// case-sensitive), e.g. "spain" matches "l10n-spain".
    pub name: String,
    /// Restrict to a specific GitHub/GitLab organization, e.g. "OCA".
    pub org: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepositorySearchResult {
    pub organization: String,
    pub repository: String,
    /// Odoo version -> number of modules recorded in this repository at that
    /// version.
    pub modules_by_odoo_version: HashMap<String, u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListRepositoryModulesParams {
    /// Organization name exactly as returned by search_repositories, e.g. "OCA".
    pub org: String,
    /// Repository name exactly as returned by search_repositories, e.g. "l10n-spain".
    pub repo: String,
    /// Restrict to a specific Odoo version, e.g. "17.0". Recommended: a \
    /// repository can carry many versions (one per branch), so omitting this \
    /// returns every version's modules at once.
    pub odoo_version: Option<String>,
    /// Restrict to installable (true) or non-installable (false) modules.
    pub installable: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleSummary {
    pub technical_name: String,
    pub name: String,
    pub odoo_version: String,
    pub module_version: String,
    pub description: String,
    pub category: String,
    pub application: bool,
    pub installable: bool,
    pub auto_install: bool,
    pub authors: Vec<String>,
    pub maintainers: Vec<String>,
    pub committers: Vec<String>,
    pub last_commit_date: String,
    pub last_commit_author: String,
    /// Direct (one level) dependencies only, unlike get_module's transitive
    /// closure - cheap to compute in bulk for every module in a repository.
    /// The `odoo` map is keyed by "organization/repository" since a
    /// dependency can live in a different repository than the module itself.
    pub depends: ModuleDependencies,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetCommitterActivityParams {
    /// Exact committer (git author) name, e.g. "Jane Doe". This project only
    /// tracks git commit author names, not emails or GitHub logins - get real
    /// names from the `committers` field of list_repository_modules.
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitterActivityEntry {
    pub technical_name: String,
    pub name: String,
    pub odoo_version: String,
    pub organization: String,
    pub repository: String,
    pub commits: i32,
    pub insertions: i32,
    pub deletions: i32,
}

fn build_search_results(rows: Vec<models::module::ModuleGenericInfo>) -> Vec<ModuleSearchResult> {
    rows.into_iter()
        .map(|row| {
            let versions = row
                .versions
                .split(',')
                .filter_map(|v| v.trim().parse::<u8>().ok())
                .map(|v| odoo_version_u8_to_string(&v))
                .collect::<Vec<String>>();
            let mut by_src = HashMap::new();
            by_src.insert(row.src, versions);
            ModuleSearchResult {
                technical_name: row.technical_name,
                versions: by_src,
            }
        })
        .collect()
}

fn get_module_views(conn: &mut SqliteConnection, module_version_id: &i64) -> Vec<ModuleView> {
    models::module_view::get_by_module_version_id(conn, module_version_id)
        .into_iter()
        .map(|v| ModuleView {
            is_new: v.inherit_xml_id.is_none(),
            xml_id: v.xml_id,
            name: v.name.unwrap_or_default(),
            model: v.model.unwrap_or_default(),
            inherit_xml_id: v.inherit_xml_id,
            view_type: v.view_type,
        })
        .collect()
}

fn get_module_models(conn: &mut SqliteConnection, module_version_id: &i64) -> Vec<ModuleModel> {
    models::module_model::get_by_module_version_id(conn, module_version_id)
        .into_iter()
        .map(|m| {
            let fields = models::module_model_field::get_by_module_model_id(conn, &m.id)
                .into_iter()
                .map(|f| ModuleModelField {
                    attrs: f.attrs_value(),
                    name: f.name,
                    field_type: f.field_type,
                    relation: f.relation,
                })
                .collect();
            let methods = models::module_model_method::get_by_module_model_id(conn, &m.id)
                .into_iter()
                .map(|meth| ModuleModelMethod {
                    decorators: meth.decorators_vec(),
                    name: meth.name,
                    signature: meth.signature,
                    docstring: meth.docstring,
                })
                .collect();
            let attrs = m.attrs_value();
            ModuleModel {
                model_name: m.model_name,
                class_name: m.class_name,
                inherit_from: m
                    .inherit_from
                    .map(|s| s.split(',').map(|x| x.to_string()).collect())
                    .unwrap_or_default(),
                is_new_model: m.is_new_model,
                docstring: m.docstring,
                attrs,
                fields,
                methods,
            }
        })
        .collect()
}

fn get_org_repo(
    conn: &mut SqliteConnection,
    module: &models::module::Model,
) -> (models::gh_organization::Model, models::gh_repository::Model) {
    let repo = models::gh_repository::get_by_id(conn, &module.gh_repository_id)
        .expect("module references a gh_repository row that does not exist");
    let org = models::gh_organization::get_by_id(conn, &repo.gh_organization_id)
        .expect("gh_repository references a gh_organization row that does not exist");
    (org, repo)
}

fn build_module_info(conn: &mut SqliteConnection, module: &models::module::Model) -> ModuleInfo {
    let (org, repo) = get_org_repo(conn, module);
    ModuleInfo {
        technical_name: module.technical_name.clone(),
        name: module.name.clone(),
        odoo_version: odoo_version_u8_to_string(&(module.version_odoo as u8)),
        module_version: module.version_module.clone(),
        description: module.description.clone().unwrap_or_default(),
        authors: models::module_author::get_names_by_module_id(conn, &module.id),
        maintainers: models::module_maintainer::get_names_by_module_id(conn, &module.id),
        license: module.license.clone().unwrap_or_default(),
        category: module.category.clone().unwrap_or_default(),
        application: module.application,
        installable: module.installable,
        auto_install: module.auto_install,
        git: format!("https://github.com/{}/{}.git", &org.name, &repo.name),
        organization: org.name,
        repository: repo.name,
        last_commit_date: module.last_commit_date.clone(),
        last_commit_author: module.last_commit_author.clone(),
    }
}

fn build_module_docs(conn: &mut SqliteConnection, module: &models::module::Model) -> ModuleDocs {
    let (org, repo) = get_org_repo(conn, module);
    ModuleDocs {
        technical_name: module.technical_name.clone(),
        odoo_version: odoo_version_u8_to_string(&(module.version_odoo as u8)),
        organization: org.name,
        repository: repo.name,
        installation: module.installation.clone().unwrap_or_default(),
        usage: module.usage.clone().unwrap_or_default(),
    }
}

fn build_module_dependency_info(
    conn: &mut SqliteConnection,
    module: &models::module::Model,
) -> ModuleDependencyInfo {
    let (org, repo) = get_org_repo(conn, module);
    let full_deps = models::dependency::get_full_dependency_info(conn, module);
    ModuleDependencyInfo {
        technical_name: module.technical_name.clone(),
        odoo_version: odoo_version_u8_to_string(&(module.version_odoo as u8)),
        organization: org.name,
        repository: repo.name,
        dependencies: ModuleDependencies {
            odoo: full_deps.odoo,
            pip: full_deps.pip,
            bin: full_deps.bin,
        },
    }
}

fn build_module_code_analysis(
    conn: &mut SqliteConnection,
    module: &models::module::Model,
    version_module: Option<&str>,
) -> ModuleCodeAnalysis {
    let (org, repo) = get_org_repo(conn, module);
    // None (default) resolves to the latest version; an explicit request that
    // doesn't match any known version comes back with empty views/models
    // rather than silently falling back to "latest".
    let resolved_version = match version_module {
        Some(v) => models::module_version::get_by_module_id_version_module(conn, &module.id, v),
        None => models::module_version::resolve_current(conn, module),
    };
    let (views, module_models, module_version) = match &resolved_version {
        Some(mv) => (
            get_module_views(conn, &mv.id),
            get_module_models(conn, &mv.id),
            mv.version_module.clone(),
        ),
        None => (
            Vec::new(),
            Vec::new(),
            version_module
                .map(|v| v.to_string())
                .unwrap_or_else(|| module.version_module.clone()),
        ),
    };
    ModuleCodeAnalysis {
        technical_name: module.technical_name.clone(),
        odoo_version: odoo_version_u8_to_string(&(module.version_odoo as u8)),
        module_version,
        organization: org.name,
        repository: repo.name,
        views,
        models: module_models,
    }
}

fn find_modules(
    conn: &mut SqliteConnection,
    technical_name: &str,
    version_odoo: &u8,
    org: &Option<String>,
    repo: &Option<String>,
) -> Vec<models::module::Model> {
    match (org, repo) {
        (Some(org), Some(repo)) => {
            models::module::get_by_technical_name_odoo_version_organization_name_repository_name(
                conn,
                technical_name,
                version_odoo,
                org,
                repo,
            )
        }
        (Some(org), None) => models::module::get_by_technical_name_odoo_version_organization_name(
            conn,
            technical_name,
            version_odoo,
            org,
        ),
        (None, Some(repo)) => models::module::get_by_technical_name_odoo_version_repository_name(
            conn,
            technical_name,
            version_odoo,
            repo,
        ),
        (None, None) => {
            let names = [technical_name.to_string()];
            models::module::get_by_technical_name_odoo_version(conn, &names, version_odoo)
        }
    }
}

fn build_repository_search_results(
    rows: Vec<models::gh_repository::RepositoryInfo>,
    org_filter: Option<&str>,
) -> Vec<RepositorySearchResult> {
    let mut by_repo: HashMap<(String, String), HashMap<String, u32>> = HashMap::new();
    for row in rows {
        if org_filter.is_some_and(|org| org != row.organization) {
            continue;
        }
        by_repo
            .entry((row.organization, row.name))
            .or_default()
            .insert(
                odoo_version_u8_to_string(&(row.version_odoo as u8)),
                row.num_modules as u32,
            );
    }
    let mut results: Vec<RepositorySearchResult> = by_repo
        .into_iter()
        .map(
            |((organization, repository), modules_by_odoo_version)| RepositorySearchResult {
                organization,
                repository,
                modules_by_odoo_version,
            },
        )
        .collect();
    results.sort_by(|a, b| (&a.organization, &a.repository).cmp(&(&b.organization, &b.repository)));
    results
}

fn build_module_direct_dependencies(
    conn: &mut SqliteConnection,
    module_id: &i64,
) -> ModuleDependencies {
    let mut odoo: HashMap<String, Vec<String>> = HashMap::new();
    for dep in models::dependency::get_module_dependency_info(conn, module_id) {
        odoo.entry(format!("{}/{}", dep.org, dep.repo))
            .or_default()
            .push(dep.module_name);
    }
    ModuleDependencies {
        odoo,
        pip: models::dependency::get_module_external_dependency_names(conn, module_id, "python"),
        bin: models::dependency::get_module_external_dependency_names(conn, module_id, "bin"),
    }
}

fn build_module_summary(
    conn: &mut SqliteConnection,
    module: &models::module::Model,
) -> ModuleSummary {
    ModuleSummary {
        technical_name: module.technical_name.clone(),
        name: module.name.clone(),
        odoo_version: odoo_version_u8_to_string(&(module.version_odoo as u8)),
        module_version: module.version_module.clone(),
        description: module.description.clone().unwrap_or_default(),
        category: module.category.clone().unwrap_or_default(),
        application: module.application,
        installable: module.installable,
        auto_install: module.auto_install,
        authors: models::module_author::get_names_by_module_id(conn, &module.id),
        maintainers: models::module_maintainer::get_names_by_module_id(conn, &module.id),
        committers: models::module_committer::get_names_by_module_id(conn, &module.id),
        last_commit_date: module.last_commit_date.clone(),
        last_commit_author: module.last_commit_author.clone(),
        depends: build_module_direct_dependencies(conn, &module.id),
    }
}

#[cached(
    type = "TimedSizedCache<String, Vec<ModuleSummary>>",
    key = "String",
    create = r#"
        {
            let ttl_secs = *crate::config::MCP_CONFIG.get_cache_ttl();
            TimedSizedCache::with_size_and_lifespan_and_refresh(500, ttl_secs, true)
        }
    "#,
    convert = r#"{ format!("{org}|{repo}|{odoo_version:?}|{installable:?}") }"#
)]
fn list_repository_modules_cached(
    pool: Pool,
    org: String,
    repo: String,
    odoo_version: Option<String>,
    installable: Option<bool>,
) -> Vec<ModuleSummary> {
    let mut conn = pool
        .get()
        .expect("failed to get a DB connection from the pool");
    let modules = models::module::get_by_organization_repository_name(&mut conn, &org, &repo);
    let version_filter = odoo_version.as_deref().map(odoo_version_string_to_u8);
    modules
        .iter()
        .filter(|m| version_filter.is_none_or(|v| m.version_odoo as u8 == v))
        .filter(|m| installable.is_none_or(|i| m.installable == i))
        .map(|m| build_module_summary(&mut conn, m))
        .collect::<Vec<_>>()
}

#[cached(
    type = "TimedSizedCache<String, Vec<ModuleInfo>>",
    key = "String",
    create = r#"
        {
            let ttl_secs = *crate::config::MCP_CONFIG.get_cache_ttl();
            TimedSizedCache::with_size_and_lifespan_and_refresh(500, ttl_secs, true)
        }
    "#,
    convert = r#"{ format!("{technical_name}|{odoo_version}|{org:?}|{repo:?}") }"#
)]
fn get_module_cached(
    pool: Pool,
    technical_name: String,
    odoo_version: String,
    org: Option<String>,
    repo: Option<String>,
) -> Vec<ModuleInfo> {
    let mut conn = pool
        .get()
        .expect("failed to get a DB connection from the pool");
    let version_odoo = odoo_version_string_to_u8(&odoo_version);
    let modules = find_modules(&mut conn, &technical_name, &version_odoo, &org, &repo);
    modules
        .iter()
        .map(|m| build_module_info(&mut conn, m))
        .collect::<Vec<_>>()
}

#[cached(
    type = "TimedSizedCache<String, Vec<ModuleDocs>>",
    key = "String",
    create = r#"
        {
            let ttl_secs = *crate::config::MCP_CONFIG.get_cache_ttl();
            TimedSizedCache::with_size_and_lifespan_and_refresh(500, ttl_secs, true)
        }
    "#,
    convert = r#"{ format!("{technical_name}|{odoo_version}|{org:?}|{repo:?}") }"#
)]
fn get_module_docs_cached(
    pool: Pool,
    technical_name: String,
    odoo_version: String,
    org: Option<String>,
    repo: Option<String>,
) -> Vec<ModuleDocs> {
    let mut conn = pool
        .get()
        .expect("failed to get a DB connection from the pool");
    let version_odoo = odoo_version_string_to_u8(&odoo_version);
    let modules = find_modules(&mut conn, &technical_name, &version_odoo, &org, &repo);
    modules
        .iter()
        .map(|m| build_module_docs(&mut conn, m))
        .collect::<Vec<_>>()
}

#[cached(
    type = "TimedSizedCache<String, Vec<ModuleDependencyInfo>>",
    key = "String",
    create = r#"
        {
            let ttl_secs = *crate::config::MCP_CONFIG.get_cache_ttl();
            TimedSizedCache::with_size_and_lifespan_and_refresh(500, ttl_secs, true)
        }
    "#,
    convert = r#"{ format!("{technical_name}|{odoo_version}|{org:?}|{repo:?}") }"#
)]
fn get_module_dependencies_cached(
    pool: Pool,
    technical_name: String,
    odoo_version: String,
    org: Option<String>,
    repo: Option<String>,
) -> Vec<ModuleDependencyInfo> {
    let mut conn = pool
        .get()
        .expect("failed to get a DB connection from the pool");
    let version_odoo = odoo_version_string_to_u8(&odoo_version);
    let modules = find_modules(&mut conn, &technical_name, &version_odoo, &org, &repo);
    modules
        .iter()
        .map(|m| build_module_dependency_info(&mut conn, m))
        .collect::<Vec<_>>()
}

#[cached(
    type = "TimedSizedCache<String, Vec<ModuleCodeAnalysis>>",
    key = "String",
    create = r#"
        {
            let ttl_secs = *crate::config::MCP_CONFIG.get_cache_ttl();
            TimedSizedCache::with_size_and_lifespan_and_refresh(500, ttl_secs, true)
        }
    "#,
    convert = r#"{ format!("{technical_name}|{odoo_version}|{org:?}|{repo:?}|{version_module:?}") }"#
)]
fn get_module_code_analysis_cached(
    pool: Pool,
    technical_name: String,
    odoo_version: String,
    org: Option<String>,
    repo: Option<String>,
    version_module: Option<String>,
) -> Vec<ModuleCodeAnalysis> {
    let mut conn = pool
        .get()
        .expect("failed to get a DB connection from the pool");
    let version_odoo = odoo_version_string_to_u8(&odoo_version);
    let modules = find_modules(&mut conn, &technical_name, &version_odoo, &org, &repo);
    modules
        .iter()
        .map(|m| build_module_code_analysis(&mut conn, m, version_module.as_deref()))
        .collect::<Vec<_>>()
}

fn json_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("failed to serialize result: {e}"), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

#[derive(Clone)]
pub struct OghMcp {
    pool: Pool,
}

#[tool_router]
impl OghMcp {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    #[tool(
        description = "Search Odoo modules by technical name substring, optionally filtered by \
                        Odoo version and/or installable flag. Returns, per matching repository, \
                        the Odoo versions in which the module was found. Good when you already \
                        know (part of) a module's technical name; if instead you're building a \
                        module pack for a country/topic and don't know exact names, use \
                        search_repositories + list_repository_modules instead, since not every \
                        module in a repository follows a naming convention (e.g. \
                        delivery_dhl_parcel lives in OCA/l10n-spain). Follow up with get_module \
                        for manifest metadata on a specific match."
    )]
    async fn search_modules(
        &self,
        Parameters(params): Parameters<SearchModulesParams>,
    ) -> Result<CallToolResult, McpError> {
        let pool = self.pool.clone();
        let rows = tokio::task::spawn_blocking(move || {
            let mut conn = pool
                .get()
                .expect("failed to get a DB connection from the pool");
            match (params.odoo_version, params.installable) {
                (Some(v), Some(installable)) => {
                    models::module::get_generic_info_by_odoo_version_installable(
                        &mut conn,
                        &params.name,
                        &odoo_version_string_to_u8(&v),
                        &installable,
                    )
                }
                (Some(v), None) => models::module::get_generic_info_by_odoo_version(
                    &mut conn,
                    &params.name,
                    &odoo_version_string_to_u8(&v),
                ),
                (None, Some(installable)) => models::module::get_generic_info_by_installable(
                    &mut conn,
                    &params.name,
                    &installable,
                ),
                (None, None) => models::module::get_generic_info(&mut conn, &params.name),
            }
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        json_result(&build_search_results(rows))
    }

    #[tool(
        description = "Get manifest metadata for one module at one Odoo version: description, \
                        authors/maintainers, license/category/application/installable flags, and \
                        which organization/repository carries it. This is the \
                        lightweight entry point for one module - call get_module_docs for \
                        install/usage instructions, get_module_dependencies for the dependency \
                        closure, or get_module_code_analysis for its views/models/fields/methods, \
                        only when you actually need that detail."
    )]
    async fn get_module(
        &self,
        Parameters(params): Parameters<GetModuleParams>,
    ) -> Result<CallToolResult, McpError> {
        let pool = self.pool.clone();
        let infos = tokio::task::spawn_blocking(move || {
            get_module_cached(
                pool,
                params.technical_name,
                params.odoo_version,
                params.org,
                params.repo,
            )
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        json_result(&infos)
    }

    #[tool(
        description = "Get rendered readme/INSTALL.md and readme/USAGE.md for one module at one \
                        Odoo version - install-specific steps beyond adding it to `depends` \
                        (system packages, config, etc.), and how to use it once installed (menus, \
                        workflow, etc.). Empty strings if the module doesn't have these files. \
                        Call get_module first to confirm the module exists."
    )]
    async fn get_module_docs(
        &self,
        Parameters(params): Parameters<GetModuleParams>,
    ) -> Result<CallToolResult, McpError> {
        let pool = self.pool.clone();
        let docs = tokio::task::spawn_blocking(move || {
            get_module_docs_cached(
                pool,
                params.technical_name,
                params.odoo_version,
                params.org,
                params.repo,
            )
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        json_result(&docs)
    }

    #[tool(
        description = "Get the transitive Odoo/pip/bin dependency closure for one module at one \
                        Odoo version. The `odoo` map is keyed by \"organization/repository\" \
                        since a dependency can live in a different repository than the module \
                        itself. For direct (one-level) dependencies of every module in a \
                        repository at once, use list_repository_modules instead."
    )]
    async fn get_module_dependencies(
        &self,
        Parameters(params): Parameters<GetModuleParams>,
    ) -> Result<CallToolResult, McpError> {
        let pool = self.pool.clone();
        let deps = tokio::task::spawn_blocking(move || {
            get_module_dependencies_cached(
                pool,
                params.technical_name,
                params.odoo_version,
                params.org,
                params.repo,
            )
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        json_result(&deps)
    }

    #[tool(
        description = "Get the code analysis for one module at one Odoo version: XML views it \
                        defines or inherits, and the Odoo models it defines or extends with their \
                        fields and methods, including docstrings and signatures. This is the \
                        heaviest tool in this server - only call it when you actually need \
                        view/model/field/method detail, not just to check what a module does or \
                        what it depends on. Defaults to the latest known module version; pass \
                        version_module (see list_module_versions) to inspect an older one."
    )]
    async fn get_module_code_analysis(
        &self,
        Parameters(params): Parameters<GetModuleCodeAnalysisParams>,
    ) -> Result<CallToolResult, McpError> {
        let pool = self.pool.clone();
        let analyses = tokio::task::spawn_blocking(move || {
            get_module_code_analysis_cached(
                pool,
                params.technical_name,
                params.odoo_version,
                params.org,
                params.repo,
                params.version_module,
            )
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        json_result(&analyses)
    }

    #[tool(
        description = "List every module version ever recorded for one module at one Odoo \
                        version (per matching repository), each with its first/last-seen dates \
                        and whether it's the latest. Use this to discover which version_module \
                        values can be passed to get_module."
    )]
    async fn list_module_versions(
        &self,
        Parameters(params): Parameters<ListModuleVersionsParams>,
    ) -> Result<CallToolResult, McpError> {
        let pool = self.pool.clone();
        let history = tokio::task::spawn_blocking(move || {
            let mut conn = pool
                .get()
                .expect("failed to get a DB connection from the pool");
            let version_odoo = odoo_version_string_to_u8(&params.odoo_version);
            let modules = find_modules(
                &mut conn,
                &params.technical_name,
                &version_odoo,
                &params.org,
                &params.repo,
            );
            modules
                .iter()
                .map(|m| {
                    let repo = models::gh_repository::get_by_id(&mut conn, &m.gh_repository_id)
                        .expect("module references a gh_repository row that does not exist");
                    let org =
                        models::gh_organization::get_by_id(&mut conn, &repo.gh_organization_id)
                            .expect(
                            "gh_repository references a gh_organization row that does not exist",
                        );
                    let versions = models::module_version::get_by_module_id(&mut conn, &m.id)
                        .into_iter()
                        .map(|v| ModuleVersionInfo {
                            is_latest: v.version_module == m.version_module,
                            version_module: v.version_module,
                            create_date: v.create_date,
                            update_date: v.update_date,
                        })
                        .collect();
                    ModuleVersionHistory {
                        organization: org.name,
                        repository: repo.name,
                        versions,
                    }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        json_result(&history)
    }

    #[tool(
        description = "Search GitHub/GitLab repositories by name substring (optionally scoped to \
                        one organization), e.g. name=\"spain\" finds organization \"OCA\" \
                        repository \"l10n-spain\". Returns, per matching repository, how many \
                        modules it carries at each Odoo version. Repositories on OCA are \
                        organized per country/topic (l10n-france, l10n-mexico, pos, \
                        account-invoicing, ...), so this is the entry point for building a \
                        module pack for a country or topic: find the repository here, then call \
                        list_repository_modules on it."
    )]
    async fn search_repositories(
        &self,
        Parameters(params): Parameters<SearchRepositoriesParams>,
    ) -> Result<CallToolResult, McpError> {
        let pool = self.pool.clone();
        let rows = tokio::task::spawn_blocking(move || {
            let mut conn = pool
                .get()
                .expect("failed to get a DB connection from the pool");
            models::gh_repository::search_by_name(&mut conn, &params.name)
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        json_result(&build_repository_search_results(
            rows,
            params.org.as_deref(),
        ))
    }

    #[tool(
        description = "List every module recorded in one repository (as found via \
                        search_repositories), optionally filtered by Odoo version and/or \
                        installable flag. This is the core tool to assemble a module pack: it \
                        returns every module a repository carries - not just ones matching a \
                        naming pattern - with manifest metadata, authors/maintainers/committers, \
                        last-commit date/author (to judge whether it's still maintained), and \
                        each module's direct (one level) Odoo/pip/bin dependencies (the Odoo ones \
                        grouped by which organization/repository carries them, since a dependency \
                        can live outside this repository). Dependencies here are direct, not the \
                        full transitive closure, and there's no views/models code analysis - call \
                        get_module_dependencies or get_module_code_analysis on individual modules \
                        of interest for that level of detail."
    )]
    async fn list_repository_modules(
        &self,
        Parameters(params): Parameters<ListRepositoryModulesParams>,
    ) -> Result<CallToolResult, McpError> {
        let pool = self.pool.clone();
        let summaries = tokio::task::spawn_blocking(move || {
            list_repository_modules_cached(
                pool,
                params.org,
                params.repo,
                params.odoo_version,
                params.installable,
            )
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        json_result(&summaries)
    }

    #[tool(
        description = "Given an exact committer (git author) name, list every module they've \
                        committed to across all Odoo versions and repositories, with commit \
                        counts and lines inserted/deleted, ordered by Odoo version then commit \
                        count. Use this to check who \
                        is actually maintaining a module in practice (as opposed to the nominal \
                        manifest authors/maintainers) - e.g. before recommending a module for a \
                        pack, confirm its top committer is still active elsewhere. Get real names \
                        from the committers field of list_repository_modules; this is an exact \
                        match, not a substring search."
    )]
    async fn get_committer_activity(
        &self,
        Parameters(params): Parameters<GetCommitterActivityParams>,
    ) -> Result<CallToolResult, McpError> {
        let pool = self.pool.clone();
        let entries = tokio::task::spawn_blocking(move || {
            let mut conn = pool
                .get()
                .expect("failed to get a DB connection from the pool");
            models::module_committer::get_activity_by_committer_name(&mut conn, &params.name)
                .into_iter()
                .map(|a| CommitterActivityEntry {
                    technical_name: a.technical_name,
                    name: a.name,
                    odoo_version: odoo_version_u8_to_string(&(a.version_odoo as u8)),
                    organization: a.organization,
                    repository: a.repository,
                    commits: a.commits,
                    insertions: a.insertions,
                    deletions: a.deletions,
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        json_result(&entries)
    }
}

#[tool_handler]
impl ServerHandler for OghMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "oghcollector-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "Read-only access to the OGHCollector Odoo module metadata database. Two \
                 discovery paths: (1) know (part of) a module's technical name? Use \
                 search_modules, then get_module for its manifest metadata. (2) building a \
                 module pack for a country or topic (e.g. \"what does a Spain localization \
                 need\")? Use search_repositories to find the relevant repository (OCA organizes \
                 these per country/topic), then list_repository_modules to bulk-list every \
                 module it carries with direct dependencies and maintenance signals (last \
                 commit, committers). Either way, get_module's response is intentionally light - \
                 call get_module_docs (install/usage instructions), get_module_dependencies \
                 (full transitive closure) or get_module_code_analysis (views/models/fields/\
                 methods) on individual modules only when you actually need that detail, since \
                 code analysis in particular can be large. Use list_module_versions to see a \
                 module's recorded version history, and get_committer_activity to check what \
                 else a specific person has committed to, e.g. to gauge whether they're still \
                 active."
                    .to_string(),
            )
    }
}
