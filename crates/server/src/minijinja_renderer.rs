// Copyright 2025 Alexandre D. Díaz
use actix_utils::future::{ready, Ready};
use actix_web_lab::respond::Html;
use actix_web::{
    dev, 
    error,
    web,
    FromRequest,
    HttpRequest
};

pub struct MiniJinjaRenderer {
    tmpl_env: web::Data<minijinja_autoreload::AutoReloader>,
}

impl MiniJinjaRenderer {
    pub fn render(
        &self,
        tmpl: &str,
        ctx: impl Into<minijinja::value::Value>,
    ) -> actix_web::Result<Html> {
        self.tmpl_env
            .acquire_env()
            .map_err(|_| error::ErrorInternalServerError("could not acquire template env"))?
            .get_template(tmpl)
            .map_err(|_| error::ErrorInternalServerError("could not find template"))?
            .render(ctx.into())
            .map(Html)
            .map_err(|err| {
                log::error!("{err}");
                error::ErrorInternalServerError("template error")
            })
    }

    pub fn from_req(req: &HttpRequest) -> MiniJinjaRenderer {
        MiniJinjaRenderer::extract(req).into_inner().unwrap()
    }
}

impl FromRequest for MiniJinjaRenderer {
    type Error = actix_web::Error;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _pl: &mut dev::Payload) -> Self::Future {
        let tmpl_env = <web::Data<minijinja_autoreload::AutoReloader>>::extract(req)
            .into_inner()
            .unwrap();

        ready(Ok(Self { tmpl_env }))
    }
}