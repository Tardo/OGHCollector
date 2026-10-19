// Copyright Alexandre D. Díaz
//! Answers `HEAD` with the `GET` route: `#[get]` only matches `GET`, so every `HEAD` (feed
//! readers, link checkers) was a 404. actix-http notes the method at decode time and drops
//! the body of a `HEAD` response itself, so rewriting the method here is enough. The handler
//! still runs in full, like the `GET` would.
use actix_web::body::MessageBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::http::Method;
use actix_web::middleware::Next;
use actix_web::Error;

pub async fn head_as_get(
    mut req: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, Error> {
    if req.method() == Method::HEAD {
        req.head_mut().method = Method::GET;
    }
    next.call(req).await
}

#[cfg(test)]
mod tests {
    use actix_web::{get, http::StatusCode, middleware, test, App, HttpResponse};

    #[get("/")]
    async fn index() -> HttpResponse {
        HttpResponse::Ok().finish()
    }

    #[actix_web::test]
    async fn head_reaches_a_get_route() {
        let app = test::init_service(
            App::new()
                .service(index)
                .wrap(middleware::from_fn(super::head_as_get)),
        )
        .await;
        let req = test::TestRequest::default()
            .method(actix_web::http::Method::HEAD)
            .uri("/")
            .to_request();
        assert_eq!(test::call_service(&app, req).await.status(), StatusCode::OK);
    }
}
