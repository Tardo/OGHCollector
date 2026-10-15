// Copyright Alexandre D. Díaz
//! The workflow editor (`workflow_editor_enabled`): shows the built-in
//! workflows and the collector config's `workflows:` (never its sources or
//! actions), and builds YAML to copy by hand: the server stays read-only.
//! Parsing and validation are the collector's own (`oghworkflow`), so what
//! the editor accepts is what `oghcollector --check` accepts.
//!
//! `workflows` (only with `workflow_editor_enabled`, default off): `GET /workflows` (page), `GET
//! /workflows/data` (JSON: `oghworkflow::metadata()` + built-ins + the `collector_config` file's
//! `workflows:` — never sources/actions —, re-read per request, a broken file → `file_error`,
//! built-ins still load) and `POST /workflows/parse` (YAML text → normalized workflows + `validate`
//! errors; stateless; actix's 256 KiB payload default caps it, and
//! `oghworkflow::check_untrusted_yaml` (yaml-rust2 event scan: no aliases, depth ≤ 64) runs first —
//! an alias bomb of a few KB made the YAML loader abort the process on allocation failure; nesting
//! alone is caught by yaml-rust2's recursion limit). `/workflows/data` is `Cache-Control: no-cache`
//! (re-read per request; DefaultHeaders would make it public/1h). Copy gives only the `<name>:`
//! block indented for an existing `workflows:` (a second top-level `workflows:` would clobber the
//! file's); Download gives the whole document. Served as JSON, not template `tojson` (minijinja
//! `json` feature isn't on for a server-only build, and prompts could carry `</script>`). Compose
//! mounts `collector.yaml` into server too.
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
/// the existing workflows with where each comes from: the workflows file
/// next to the collector config (none there: the copy built in), then the
/// config's own, which replace any of the same name. Both files are read on
/// every request; a broken one is reported and left out, the rest still load.
fn data(collector_config: &str) -> serde_json::Value {
    let mut errors = Vec::new();
    let (shipped, from_file) = oghworkflow::shipped(collector_config).unwrap_or_else(|err| {
        errors.push(err);
        Default::default()
    });
    let own = oghworkflow::file_workflows(collector_config).unwrap_or_else(|err| {
        errors.push(err);
        Default::default()
    });
    let mut all: BTreeMap<_, _> = shipped.into_iter().collect();
    all.extend(own.clone());
    let file_error = (!errors.is_empty()).then(|| errors.join("\n"));
    let base = if from_file {
        "workflows.yaml"
    } else {
        "built-in"
    };
    let workflows: Vec<serde_json::Value> = all
        .into_iter()
        .map(|(name, wf)| {
            let origin = if own.contains_key(&name) {
                "collector config"
            } else {
                base
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
                .flat_map(|(name, wf)| oghworkflow::problems(name, wf))
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
        std::fs::write(
            dir.join("workflows.yaml"),
            "workflows:\n  shared: {on: module, enabled: false, nodes: [{id: o, type: osv}]}\n",
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
            [("mine", "collector config"), ("shared", "workflows.yaml")]
        );
        assert!(!got.to_string().contains("Secret"), "sources stay out");
        // No workflows file there: the copy built in.
        std::fs::remove_file(dir.join("workflows.yaml")).unwrap();
        let built_in = data(dir.join("collector").to_str().unwrap());
        let default = built_in["workflows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["name"] == "default")
            .unwrap();
        assert_eq!(default["origin"], "built-in");
        std::fs::write(
            dir.join("workflows.yaml"),
            "workflows:\n  shared: {on: module, enabled: false, nodes: [{id: o, type: osv}]}\n",
        )
        .unwrap();
        assert!(got["meta"]["node_types"].as_array().unwrap().len() > 10);
        std::fs::write(&file, "workflows: [").unwrap();
        let data = data(dir.join("collector").to_str().unwrap());
        assert!(data["file_error"].is_string());
        assert_eq!(
            data["workflows"].as_array().unwrap().len(),
            1,
            "the workflows file still loads"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
