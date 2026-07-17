// Copyright Alexandre D. Díaz
use actix_web::{get, HttpRequest, HttpResponse, Responder};

use crate::config::SERVER_CONFIG;
use crate::utils::get_base_url;

#[get("/robots.txt")]
pub async fn route(req: HttpRequest) -> impl Responder {
    let body = if SERVER_CONFIG.get_seo_enabled() {
        format!(
            "User-agent: *\nAllow: /\nSitemap: {}/sitemap.xml\n",
            get_base_url(&req)
        )
    } else {
        "User-agent: *\nDisallow: /\n".to_string()
    };
    HttpResponse::Ok()
        .content_type("text/plain; charset=utf-8")
        .body(body)
}
