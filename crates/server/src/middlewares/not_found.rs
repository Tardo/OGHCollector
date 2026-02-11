// Copyright Alexandre D. Díaz
use actix_web::{
    dev::ServiceResponse, http::header, middleware::ErrorHandlerResponse, HttpResponse, Responder,
    Result,
};

use crate::minijinja_renderer::MiniJinjaRenderer;

/// Error handler for a 404 Page not found error.
pub fn handler_fn<B>(svc_res: ServiceResponse<B>) -> Result<ErrorHandlerResponse<B>> {
    let res = get_error_response(&svc_res, "Page not found");

    Ok(ErrorHandlerResponse::Response(ServiceResponse::new(
        svc_res.into_parts().0,
        res.map_into_right_body(),
    )))
}

/// Generic error handler.
fn get_error_response<B>(res: &ServiceResponse<B>, error: &str) -> HttpResponse {
    let req = res.request();

    let tmpl_env = MiniJinjaRenderer::from_req(req);

    // Provide a fallback to a simple plain text response in case an error occurs during the
    // rendering of the error page.
    let fallback = |err: &str| {
        HttpResponse::build(res.status())
            .content_type(header::ContentType::plaintext())
            .body(err.to_string())
    };

    let ctx = minijinja::context! {
        error => error,
        status_code => res.status().as_str(),
    };

    match tmpl_env.render("pages/error.html", ctx) {
        Ok(body) => body
            .customize()
            .with_status(res.status())
            .respond_to(req)
            .map_into_boxed_body(),

        Err(_) => fallback(error),
    }
}
