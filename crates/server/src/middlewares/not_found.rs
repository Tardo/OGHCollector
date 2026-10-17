// Copyright Alexandre D. Díaz
use actix_web::{
    dev::ServiceResponse, http::header, middleware::ErrorHandlerResponse, HttpResponse, Responder,
    Result,
};

use crate::minijinja_renderer::MiniJinjaRenderer;

/// Error handler for a 404 Page not found error. A route that renders its
/// own 404 page (the module page of a name with no data) keeps it.
pub fn handler_fn<B>(svc_res: ServiceResponse<B>) -> Result<ErrorHandlerResponse<B>> {
    let own_page = svc_res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if own_page {
        return Ok(ErrorHandlerResponse::Response(svc_res.map_into_left_body()));
    }
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

#[cfg(test)]
mod tests {
    use actix_web::{http::StatusCode, middleware::ErrorHandlers, test, web, App, HttpResponse};

    #[actix_web::test]
    async fn a_routes_own_404_page_is_kept() {
        let app = test::init_service(
            App::new()
                .wrap(ErrorHandlers::new().handler(StatusCode::NOT_FOUND, super::handler_fn))
                .route(
                    "/",
                    web::get().to(|| async {
                        HttpResponse::NotFound()
                            .content_type("text/html; charset=utf-8")
                            .body("own page")
                    }),
                ),
        )
        .await;
        let res = test::call_service(&app, test::TestRequest::get().uri("/").to_request()).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert_eq!(test::read_body(res).await, "own page");
    }
}
