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
pub struct ModuleFullInfo {
    pub technical_name: String,
    pub name: String,
    pub odoo_version: String,
    pub module_version: String,
    pub description: String,
    pub authors: Vec<String>,
    pub maintainers: Vec<String>,
    pub committers: Vec<String>,
    pub license: String,
    pub category: String,
    pub application: bool,
    pub installable: bool,
    pub auto_install: bool,
    pub organization: String,
    pub repository: String,
    pub git: String,
    pub dependencies: ModuleDependencies,
    pub views: Vec<ModuleView>,
    pub models: Vec<ModuleModel>,
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

fn get_module_views(conn: &mut SqliteConnection, module_id: &i64) -> Vec<ModuleView> {
    models::module_view::get_by_module_id(conn, module_id)
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

fn get_module_models(conn: &mut SqliteConnection, module_id: &i64) -> Vec<ModuleModel> {
    models::module_model::get_by_module_id(conn, module_id)
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

fn build_full_info(conn: &mut SqliteConnection, module: &models::module::Model) -> ModuleFullInfo {
    let repo = models::gh_repository::get_by_id(conn, &module.gh_repository_id)
        .expect("module references a gh_repository row that does not exist");
    let org = models::gh_organization::get_by_id(conn, &repo.gh_organization_id)
        .expect("gh_repository references a gh_organization row that does not exist");
    let full_deps = models::dependency::get_full_dependency_info(conn, module);
    ModuleFullInfo {
        technical_name: module.technical_name.clone(),
        name: module.name.clone(),
        odoo_version: odoo_version_u8_to_string(&(module.version_odoo as u8)),
        module_version: module.version_module.clone(),
        description: module.description.clone().unwrap_or_default(),
        authors: models::module_author::get_names_by_module_id(conn, &module.id),
        maintainers: models::module_maintainer::get_names_by_module_id(conn, &module.id),
        committers: models::module_committer::get_names_by_module_id(conn, &module.id),
        license: module.license.clone().unwrap_or_default(),
        category: module.category.clone().unwrap_or_default(),
        application: module.application,
        installable: module.installable,
        auto_install: module.auto_install,
        git: format!("https://github.com/{}/{}.git", &org.name, &repo.name),
        organization: org.name,
        repository: repo.name,
        dependencies: ModuleDependencies {
            odoo: full_deps.odoo,
            pip: full_deps.pip,
            bin: full_deps.bin,
        },
        views: get_module_views(conn, &module.id),
        models: get_module_models(conn, &module.id),
    }
}

fn cache_ttl_secs() -> u64 {
    std::env::var("OGHCOLLECTOR_MCP_CACHE_TTL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3600)
}

/// `get_module` resolves the full transitive dependency graph recursively (see
/// `get_full_dependency_info`), which can mean dozens of DB round-trips for a module deep in
/// the Odoo/OCA graph - the same cost shape as `atlas::get_graph_data`, which this mirrors.
/// The DB only changes when the collector runs, so a TTL cache is safe; `pool` and `conn` are
/// deliberately excluded from the cache key via `convert` (only the query parameters matter).
#[cached(
    type = "TimedSizedCache<String, Vec<ModuleFullInfo>>",
    key = "String",
    create = "{ TimedSizedCache::with_size_and_lifespan_and_refresh(500, cache_ttl_secs(), true) }",
    convert = r#"{ format!("{technical_name}|{odoo_version}|{org:?}|{repo:?}") }"#
)]
fn get_module_cached(
    pool: Pool,
    technical_name: String,
    odoo_version: String,
    org: Option<String>,
    repo: Option<String>,
) -> Vec<ModuleFullInfo> {
    let mut conn = pool
        .get()
        .expect("failed to get a DB connection from the pool");
    let version_odoo = odoo_version_string_to_u8(&odoo_version);
    let modules = match (&org, &repo) {
        (Some(org), Some(repo)) => {
            models::module::get_by_technical_name_odoo_version_organization_name_repository_name(
                &mut conn,
                &technical_name,
                &version_odoo,
                org,
                repo,
            )
        }
        (Some(org), None) => models::module::get_by_technical_name_odoo_version_organization_name(
            &mut conn,
            &technical_name,
            &version_odoo,
            org,
        ),
        (None, Some(repo)) => models::module::get_by_technical_name_odoo_version_repository_name(
            &mut conn,
            &technical_name,
            &version_odoo,
            repo,
        ),
        (None, None) => models::module::get_by_technical_name_odoo_version(
            &mut conn,
            std::slice::from_ref(&technical_name),
            &version_odoo,
        ),
    };
    modules
        .iter()
        .map(|m| build_full_info(&mut conn, m))
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
                        the Odoo versions in which the module was found."
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
        description = "Get full information for one module at one Odoo version: manifest \
                        metadata, authors/maintainers/committers, the transitive Odoo/pip/bin \
                        dependency closure, and a code analysis (XML views it defines or \
                        inherits, and the Odoo models it defines or extends with their fields \
                        and methods, including docstrings and signatures)."
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
                "Read-only access to the OGHCollector Odoo module metadata database. Use \
                 search_modules to find a module's technical name and which repositories/Odoo \
                 versions carry it, then get_module for full manifest, dependency and code \
                 analysis details."
                    .to_string(),
            )
    }
}
