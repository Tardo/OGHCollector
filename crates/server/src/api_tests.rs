// Copyright Alexandre D. Díaz
//! The JSON API's contract over `sqlitedb::fixture`: status codes and the
//! fields clients read. `/v1/semantic-search` only up to its input checks
//! (a real query downloads the embedding model).
use actix_web::{http::StatusCode, test as awtest, web, App};
use serde_json::{json, Value};
use sqlitedb::fixture::demo;

use crate::workspace::Workspaces;

macro_rules! api {
    ($name:expr) => {{
        let (fixture, pool) = demo($name);
        let app = awtest::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(Workspaces::new(fixture.path.clone(), pool)))
                .service(crate::routes::atlas::route_atlas_data)
                .configure(|cfg| crate::configure_v1(cfg, true)),
        )
        .await;
        (fixture, app)
    }};
}

#[actix_web::test]
async fn a_bad_odoo_version_is_a_400() {
    let (_db, app) = api!("api-bad-version");
    for uri in [
        "/v1/module/sale_demo/v17",
        "/v1/module/sale_demo/v17?org=OCA",
        "/v1/module/sale_demo/v17/versions",
        "/v1/search?odoo_version=v17",
        "/v1/search/demo?odoo_version=v17",
        "/v1/search/demo?odoo_version=v17&installable=true",
        "/v1/semantic-search?q=invoices&odoo_version=v17",
        "/v1/compare/web_widget_demo?from=v17&to=18.0",
        "/v1/license-check?odoo_version=v17&modules=sale_demo",
        "/atlas/data/v17",
    ] {
        let response =
            awtest::call_service(&app, awtest::TestRequest::get().uri(uri).to_request()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
    }
}

/// An object's field names, sorted.
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

#[actix_web::test]
async fn the_api_answers_from_the_db() {
    let (_db, app) = api!("api-answers");
    let get = |uri: &'static str| {
        let app = &app;
        async move {
            let response =
                awtest::call_service(app, awtest::TestRequest::get().uri(uri).to_request()).await;
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            serde_json::from_slice::<Value>(&awtest::read_body(response).await).unwrap()
        }
    };

    assert_eq!(
        get("/v1/module/sale_demo").await,
        json!({
            "name": "sale demo",
            "technical_name": "sale_demo",
            "odoo_versions": ["18.0"],
            "repos": ["https://github.com/OCA/sale-workflow.git"],
        })
    );

    let module = get("/v1/module/sale_demo/18.0").await;
    assert_eq!(
        keys(&module[0]),
        [
            "application",
            "authors",
            "auto_install",
            "category",
            "committers",
            "controllers",
            "dependencies",
            "description",
            "folder_size",
            "git",
            "health",
            "icon",
            "installable",
            "license",
            "license_review",
            "maintainers",
            "migration_considerations",
            "models",
            "name",
            "odoo_version",
            "organization",
            "repository",
            "required_by",
            "security_warning_count",
            "security_warnings",
            "source_path",
            "technical_name",
            "update_date",
            "version",
            "views",
            "website",
        ]
    );
    assert_eq!(
        module[0]["dependencies"],
        json!({
            "odoo": {"odoo/odoo": ["sale"], "OCA/web": ["web_widget_demo"]},
            "pip": ["requests"],
            "bin": [],
        })
    );
    assert_eq!(module[0]["license_review"]["effective_license"], "AGPL-3");
    let factors: Vec<&str> = module[0]["health"]["factors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(factors, ["Security", "Activity", "Required by", "Maturity"]);
    let sale = get("/v1/module/sale/18.0").await;
    assert!(!sale[0]["required_by"].as_array().unwrap().is_empty());
    assert_eq!(get("/v1/module/nope/18.0").await, json!([]));

    let history = get("/v1/module/web_widget_demo/18.0/versions").await;
    assert_eq!(history[0]["versions"][0]["version_module"], "1.1.0");
    assert_eq!(history[0]["versions"][0]["is_latest"], true);

    assert_eq!(
        get("/v1/search?odoo_version=18.0&depends_on=sale").await,
        json!([{
            "technical_name": "sale_demo", "name": "sale demo", "category": "Sales",
            "installable": true, "application": false, "organization": "OCA",
            "repository": "sale-workflow",
        }])
    );
    assert_eq!(
        get("/v1/search/demo?odoo_version=17.0").await,
        json!([{"technical_name": "web_widget_demo", "versions": {"OCA/web": ["17.0"]}}])
    );

    let diff = get("/v1/compare/web_widget_demo?from=17.0&to=18.0").await;
    assert_eq!(
        (&diff["from_module_version"], &diff["to_module_version"]),
        (&json!("1.0.0"), &json!("1.1.0"))
    );
    assert_eq!(diff["analysis_available"], true);
    assert_eq!(
        get("/v1/compare/sale_demo?from=17.0&to=18.0").await,
        Value::Null
    );

    let review = get("/v1/license-check?odoo_version=18.0&modules=sale_demo,nope").await;
    assert_eq!(review["effective_license"], "AGPL-3");
    assert_eq!(review["not_found"], json!(["nope"]));

    assert_eq!(
        get("/v1/repo/web").await,
        json!({"name": "web", "organizations": {"OCA": {"17.0": 1, "18.0": 1}}})
    );

    let atlas = get("/atlas/data/18.0").await;
    let mut nodes: Vec<&str> = atlas["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["key"].as_str().unwrap())
        .collect();
    nodes.sort();
    assert_eq!(
        nodes,
        ["o_sale", "o_sale_demo", "o_web_widget_demo", "p_requests"]
    );
}
