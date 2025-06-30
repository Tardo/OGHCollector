// Copyright 2025 Alexandre D. Díaz
use minijinja::context;
use actix_web::{web, get, HttpRequest, Responder, Result};

use crate::utils::get_minijinja_context;
use crate::minijinja_renderer::MiniJinjaRenderer;

use sqlitedb::Pool;


#[get("/doodba/converter")]
pub async fn route(pool: web::Data<Pool>, tmpl_env: MiniJinjaRenderer, req: HttpRequest) -> Result<impl Responder> {
    let conn = web::block(move || pool.get())
        .await?.unwrap();
    return tmpl_env.render("pages/doodba_converter.html", context!(
        ..get_minijinja_context(&req),
        ..context!(
            page_name => "doodba_converter",
        )
    ))
}
