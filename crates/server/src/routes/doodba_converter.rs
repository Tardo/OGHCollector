use minijinja::context;
use actix_web::{web, get, HttpRequest, Responder, Result};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

use crate::utils::get_minijinja_context;
use crate::minijinja_renderer::MiniJinjaRenderer;

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::{Pool, models};


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
