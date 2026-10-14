// Copyright Alexandre D. Díaz
//! The workflow editor (`workflow_editor_enabled`): shows the built-in
//! workflows and the collector config's `workflows:` (never its sources or
//! actions), and builds YAML to copy by hand: the server stays read-only.
//! Parsing and validation are the collector's own (`oghworkflow`), so what
//! the editor accepts is what `oghcollector --check` accepts.
use std::collections::BTreeMap;

use actix_web::{get, post, web, HttpRequest, HttpResponse, Responder, Result};
use minijinja::context;
use serde_json::json;

use crate::config::SERVER_CONFIG;
use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

#[get("/workflows")]
pub async fn route(tmpl_env: MiniJinjaRenderer, req: HttpRequest) -> Result<impl Responder> {
    tmpl_env.render(
        "pages/workflows.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(page_name => "workflows")
        ),
    )
}

/// What the editor starts from: the node types and fields (the rules), and
/// the existing workflows with where each comes from. The config file is
/// read on every request; a broken one is reported, the built-ins still load.
fn data(collector_config: &str) -> serde_json::Value {
    let (file, file_error) = match oghworkflow::file_workflows(collector_config) {
        Ok(file) => (file, None),
        Err(err) => (Default::default(), Some(err)),
    };
    let all: BTreeMap<_, _> = oghworkflow::with_built_in(&file).into_iter().collect();
    let workflows: Vec<serde_json::Value> = all
        .into_iter()
        .map(|(name, wf)| {
            let origin = if file.contains_key(&name) {
                "collector config"
            } else {
                "built-in"
            };
            let error = oghworkflow::validate(&name, &wf).err();
            json!({"name": name, "origin": origin, "workflow": wf, "error": error})
        })
        .collect();
    json!({
        "meta": oghworkflow::metadata(),
        "workflows": workflows,
        "file_error": file_error,
    })
}

#[get("/workflows/data")]
pub async fn route_data() -> Result<HttpResponse> {
    let path = SERVER_CONFIG.get_collector_config().to_string();
    let body = web::block(move || data(&path)).await?;
    // Re-read on every request, so never served from a cache.
    Ok(HttpResponse::Ok()
        .insert_header((actix_web::http::header::CACHE_CONTROL, "no-cache"))
        .json(body))
}

/// YAML → the workflows in it (normalized, as JSON) and every error
/// `oghcollector --check` would report for them. Stateless: nothing is kept.
fn parse(yaml: &str) -> serde_json::Value {
    // Anyone can post here: no alias bombs or deep nesting reach the parser.
    if let Err(err) = oghworkflow::check_untrusted_yaml(yaml) {
        return json!({"workflows": {}, "errors": [err]});
    }
    match oghworkflow::parse_workflows(yaml) {
        Ok(found) => {
            let found: BTreeMap<_, _> = found.into_iter().collect();
            let errors: Vec<String> = found
                .iter()
                .filter_map(|(name, wf)| oghworkflow::validate(name, wf).err())
                .collect();
            json!({"workflows": found, "errors": errors})
        }
        Err(err) => json!({"workflows": {}, "errors": [err]}),
    }
}

/// The body is capped by actix's default payload limit (256 KiB).
#[post("/workflows/parse")]
pub async fn route_parse(body: String) -> Result<HttpResponse> {
    let parsed = web::block(move || parse(&body)).await?;
    Ok(HttpResponse::Ok().json(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test as awtest, App};

    #[actix_web::test]
    async fn yaml_is_parsed_and_validated_like_the_collector_does() {
        let app = awtest::init_service(App::new().service(route_parse)).await;
        let post = |body: &str| {
            awtest::TestRequest::post()
                .uri("/workflows/parse")
                .set_payload(body.to_string())
                .to_request()
        };
        let ok = "workflows:\n  w: {on: module, nodes: [{id: l, type: log, message: hi}]}\n";
        let res: serde_json::Value = awtest::call_and_read_body_json(&app, post(ok)).await;
        assert_eq!(res["errors"], json!([]));
        assert_eq!(
            res["workflows"]["w"]["nodes"][0],
            json!({"id": "l", "type": "log", "message": "hi"})
        );
        let bad = "workflows:\n  w: {on: module, nodes: [{id: l, type: pr_comment, body: x}]}\n";
        let res: serde_json::Value = awtest::call_and_read_body_json(&app, post(bad)).await;
        assert!(res["errors"][0]
            .as_str()
            .unwrap()
            .contains("can't run `on: module`"));
        let res: serde_json::Value =
            awtest::call_and_read_body_json(&app, post("workflows: [")).await;
        assert_eq!(res["errors"].as_array().unwrap().len(), 1);
        let mut bomb = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
        for i in 1..9 {
            let refs = vec![format!("*a{}", i - 1); 10].join(", ");
            bomb += &format!("a{i}: &a{i} [{refs}]\n");
        }
        for hostile in [bomb, "[".repeat(200_000)] {
            let res: serde_json::Value =
                awtest::call_and_read_body_json(&app, post(&hostile)).await;
            assert_eq!(res["errors"].as_array().unwrap().len(), 1, "{res}");
        }
        let huge = "#".repeat(300 * 1024);
        let res = awtest::call_service(&app, post(&huge)).await;
        assert!(res.status().is_client_error());
    }

    #[test]
    fn the_editor_gets_the_built_ins_and_the_file_s_workflows_only() {
        let dir = std::env::temp_dir().join(format!("ogh-editor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("collector.yaml");
        std::fs::write(
            &file,
            "sources: [{source: Secret, versions: [18.0], actions: [ai]}]\n\
             workflows:\n  mine: {on: module, nodes: [{id: l, type: log, message: hi}]}\n",
        )
        .unwrap();
        let got = data(dir.join("collector").to_str().unwrap());
        let names: Vec<(&str, &str)> = got["workflows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| (w["name"].as_str().unwrap(), w["origin"].as_str().unwrap()))
            .collect();
        assert_eq!(
            names,
            [("default", "built-in"), ("mine", "collector config")]
        );
        assert!(!got.to_string().contains("Secret"), "sources stay out");
        assert!(got["meta"]["node_types"].as_array().unwrap().len() > 10);
        std::fs::write(&file, "workflows: [").unwrap();
        let data = data(dir.join("collector").to_str().unwrap());
        assert!(data["file_error"].is_string());
        assert_eq!(
            data["workflows"].as_array().unwrap().len(),
            1,
            "the built-ins still load"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
