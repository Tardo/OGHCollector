// Copyright Alexandre D. Díaz
use actix_web::{get, post, web, Error as AWError, HttpRequest, HttpResponse, Responder, Result};
use minijinja::context;
use serde::Deserialize;

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

/// Upper bound on the submitted target URL. Purely a payload-size guard; the
/// real protection against SSRF is `oghscan::validate_target`, which refuses
/// any non-public address before a single request is sent.
const MAX_TARGET_LEN: usize = 2048;

#[get("/scan")]
pub async fn route(tmpl_env: MiniJinjaRenderer, req: HttpRequest) -> Result<impl Responder> {
    tmpl_env.render(
        "pages/scan.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "scan"
            )
        ),
    )
}

#[derive(Debug, Deserialize)]
pub struct ScanRequest {
    // Optional so a missing field yields our friendly "A URL is required."
    // message rather than a raw serde error.
    pub url: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ScanErrorResponse {
    pub error: String,
}

/// Runs a live scan against the submitted URL and returns the report as JSON.
///
/// Deliberately a plain async handler that awaits `scan_instance` directly -
/// never inside `web::block`. The scanner spawns its own tokio tasks, which
/// require a live runtime; `web::block` runs on the blocking thread pool where
/// that would panic.
#[post("/scan/run")]
pub async fn route_run(payload: web::Json<ScanRequest>) -> Result<HttpResponse, AWError> {
    let target = payload.url.as_deref().unwrap_or("").trim();
    if target.is_empty() {
        return Ok(HttpResponse::BadRequest().json(ScanErrorResponse {
            error: "A URL is required.".to_string(),
        }));
    }
    if target.len() > MAX_TARGET_LEN {
        return Ok(HttpResponse::BadRequest().json(ScanErrorResponse {
            error: format!("URL is too long (max {MAX_TARGET_LEN} characters)."),
        }));
    }
    let report = oghscan::scan_instance(target).await;
    Ok(HttpResponse::Ok().json(report))
}
