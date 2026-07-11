// Copyright Alexandre D. Díaz
use actix_web::{get, HttpResponse, Responder};

use crate::config::SERVER_CONFIG;

#[get("/robots.txt")]
pub async fn route() -> impl Responder {
    let body = if SERVER_CONFIG.get_seo_enabled() {
        "User-agent: *\nAllow: /\n"
    } else {
        "User-agent: *\nDisallow: /\n"
    };
    HttpResponse::Ok()
        .content_type("text/plain; charset=utf-8")
        .body(body)
}
