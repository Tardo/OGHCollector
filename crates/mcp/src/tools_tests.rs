// Copyright Alexandre D. Díaz
//! The MCP tools' contract: names, required params and the shape of what
//! they answer, over `sqlitedb::fixture`. Skips `semantic_search_modules`'
//! happy path (downloads a model) and `scan_instance` (network).
use rmcp::model::ErrorCode;
use serde_json::{json, Value};
use sqlitedb::fixture::{demo, Fixture};

use super::*;

fn mcp(name: &str) -> (Fixture, OghMcp) {
    let (fixture, pool) = demo(name);
    (fixture, OghMcp::new(pool))
}

/// A tool call's JSON text.
fn json_of(result: Result<CallToolResult, McpError>) -> Value {
    let result = result.unwrap_or_else(|e| panic!("tool call failed: {e:?}"));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn params<T: serde::de::DeserializeOwned>(value: Value) -> Parameters<T> {
    Parameters(serde_json::from_value(value).unwrap())
}

#[test]
fn tool_names_and_required_params_are_stable() {
    let tools: Vec<(String, Vec<String>)> = OghMcp::tool_router()
        .list_all()
        .into_iter()
        .map(|tool| {
            let mut required: Vec<String> = tool
                .input_schema
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().map(|v| v.as_str().unwrap().to_string()).collect())
                .unwrap_or_default();
            required.sort();
            (tool.name.to_string(), required)
        })
        .collect();
    let expected: Vec<(String, Vec<String>)> = [
        (
            "compare_module_versions",
            &["from_version", "technical_name", "to_version"][..],
        ),
        ("get_committer_activity", &["name"]),
        ("get_module", &["odoo_version", "technical_name"]),
        (
            "get_module_code_analysis",
            &["odoo_version", "technical_name"],
        ),
        (
            "get_module_dependencies",
            &["odoo_version", "technical_name"],
        ),
        ("get_module_docs", &["odoo_version", "technical_name"]),
        ("list_module_versions", &["odoo_version", "technical_name"]),
        ("list_modules_by_criteria", &["odoo_version"]),
        ("list_repository_modules", &["org", "repo"]),
        (
            "review_module_licenses",
            &["odoo_version", "technical_names"],
        ),
        ("scan_instance", &["url"]),
        ("search_modules", &["name"]),
        ("search_repositories", &["name"]),
        ("semantic_search_modules", &["query"]),
    ]
    .iter()
    .map(|(name, required)| {
        (
            name.to_string(),
            required.iter().map(|r| r.to_string()).collect(),
        )
    })
    .collect();
    assert_eq!(tools, expected);
}

#[tokio::test]
async fn a_bad_odoo_version_is_invalid_params() {
    let (_db, mcp) = mcp("mcp-bad-version");
    let bad = "v17";
    let module = json!({"technical_name": "sale_demo", "odoo_version": bad});
    let calls = [
        (
            "search_modules",
            mcp.search_modules(params(json!({"name": "demo", "odoo_version": bad})))
                .await,
        ),
        (
            "list_modules_by_criteria",
            mcp.list_modules_by_criteria(params(json!({"odoo_version": bad})))
                .await,
        ),
        (
            "semantic_search_modules",
            mcp.semantic_search_modules(params(json!({"query": "x", "odoo_version": bad})))
                .await,
        ),
        ("get_module", mcp.get_module(params(module.clone())).await),
        (
            "get_module_docs",
            mcp.get_module_docs(params(module.clone())).await,
        ),
        (
            "get_module_dependencies",
            mcp.get_module_dependencies(params(module.clone())).await,
        ),
        (
            "get_module_code_analysis",
            mcp.get_module_code_analysis(params(module.clone())).await,
        ),
        (
            "list_module_versions",
            mcp.list_module_versions(params(module.clone())).await,
        ),
        (
            "list_repository_modules",
            mcp.list_repository_modules(params(
                json!({"org": "OCA", "repo": "web", "odoo_version": bad}),
            ))
            .await,
        ),
        (
            "review_module_licenses",
            mcp.review_module_licenses(params(
                json!({"technical_names": ["sale_demo"], "odoo_version": bad}),
            ))
            .await,
        ),
        (
            "compare_module_versions",
            mcp.compare_module_versions(params(
                json!({"technical_name": "web_widget_demo", "from_version": bad, "to_version": "18.0"}),
            ))
            .await,
        ),
    ];
    for (tool, result) in calls {
        match result {
            Err(e) => assert_eq!(e.code, ErrorCode::INVALID_PARAMS, "{tool}: {e:?}"),
            Ok(ok) => panic!("{tool} answered {:?}", ok.content),
        }
    }
}

/// An object's field names, sorted: the part of a tool's answer clients
/// (and the prompts written against it) depend on.
fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn tools_answer_from_the_db() {
    let (_db, mcp) = mcp("mcp-answers");
    let sale_demo = || json!({"technical_name": "sale_demo", "odoo_version": "18.0"});

    let found = json_of(mcp.search_modules(params(json!({"name": "demo"}))).await);
    assert_eq!(
        found,
        json!([
            {"technical_name": "sale_demo", "versions": {"OCA/sale-workflow": ["18.0"]}},
            {"technical_name": "web_widget_demo", "versions": {"OCA/web": ["17.0", "18.0"]}},
        ])
    );

    // "18" is read as 18.0, and answered as such.
    let rows = json_of(
        mcp.list_modules_by_criteria(params(json!({"odoo_version": "18", "depends_on": "sale"})))
            .await,
    );
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["technical_name"], "sale_demo");
    assert_eq!(rows[0]["odoo_version"], "18.0");

    let module = json_of(mcp.get_module(params(sale_demo())).await);
    assert_eq!(
        keys(&module[0]),
        [
            "application",
            "authors",
            "auto_install",
            "category",
            "description",
            "git",
            "installable",
            "last_commit_author",
            "last_commit_date",
            "license",
            "maintainers",
            "module_version",
            "name",
            "odoo_version",
            "organization",
            "repository",
            "technical_name",
        ]
    );
    assert_eq!(module[0]["license"], "AGPL-3");
    assert_eq!(module[0]["git"], "https://github.com/OCA/sale-workflow.git");

    let docs = json_of(mcp.get_module_docs(params(sale_demo())).await);
    assert_eq!(docs[0]["installation"], "");

    let deps = json_of(mcp.get_module_dependencies(params(sale_demo())).await);
    assert_eq!(
        deps[0]["dependencies"],
        json!({
            "bin": [],
            "odoo": {"OCA/web": ["web_widget_demo"], "odoo/odoo": ["sale"]},
            "pip": ["requests"],
        })
    );
    assert_eq!(deps[0]["license_review"]["effective_license"], "AGPL-3");

    let analysis = json_of(mcp.get_module_code_analysis(params(sale_demo())).await);
    assert_eq!(
        keys(&analysis[0]),
        [
            "controllers",
            "migration_considerations",
            "models",
            "module_version",
            "odoo_version",
            "organization",
            "records",
            "repository",
            "security_warnings",
            "technical_name",
            "views",
        ]
    );

    let history = json_of(
        mcp.list_module_versions(params(
            json!({"technical_name": "web_widget_demo", "odoo_version": "18.0"}),
        ))
        .await,
    );
    assert_eq!(history[0]["versions"][0]["version_module"], "1.1.0");
    assert_eq!(history[0]["versions"][0]["is_latest"], true);

    let review = json_of(
        mcp.review_module_licenses(params(
            json!({"technical_names": ["sale_demo", "nope"], "odoo_version": "18.0"}),
        ))
        .await,
    );
    assert_eq!(review["effective_license"], "AGPL-3");
    assert_eq!(review["licenses"], json!({"AGPL-3": 1, "LGPL-3": 2}));
    assert_eq!(review["not_found"], json!(["nope"]));

    let diff = json_of(
        mcp.compare_module_versions(params(json!({
            "technical_name": "web_widget_demo", "from_version": "17.0", "to_version": "18.0",
        })))
        .await,
    );
    assert_eq!(diff["from_module_version"], "1.0.0");
    assert_eq!(diff["to_module_version"], "1.1.0");
    assert_eq!(diff["analysis_available"], true);
    let missing_side = json_of(
        mcp.compare_module_versions(params(json!({
            "technical_name": "sale_demo", "from_version": "17.0", "to_version": "18.0",
        })))
        .await,
    );
    assert_eq!(missing_side, Value::Null);

    let repos = json_of(
        mcp.search_repositories(params(json!({"name": "sale"})))
            .await,
    );
    assert_eq!(
        repos,
        json!([{"organization": "OCA", "repository": "sale-workflow", "modules_by_odoo_version": {"18.0": 1}}])
    );

    let summaries = json_of(
        mcp.list_repository_modules(params(
            json!({"org": "OCA", "repo": "web", "odoo_version": "17.0"}),
        ))
        .await,
    );
    assert_eq!(summaries.as_array().unwrap().len(), 1);
    assert_eq!(summaries[0]["module_version"], "1.0.0");
    assert_eq!(summaries[0]["committers"], json!(["Ana Dev"]));

    let activity = json_of(
        mcp.get_committer_activity(params(json!({"name": "Ana Dev"})))
            .await,
    );
    assert_eq!(activity.as_array().unwrap().len(), 4);

    // Unknown module: an empty answer, not an error.
    let none = json_of(
        mcp.get_module(params(
            json!({"technical_name": "nope", "odoo_version": "18.0"}),
        ))
        .await,
    );
    assert_eq!(none, json!([]));
}
