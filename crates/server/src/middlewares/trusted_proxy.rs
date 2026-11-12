// Copyright Alexandre D. Díaz
use actix_web::body::MessageBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::http::header;
use actix_web::middleware::Next;
use actix_web::Error;

use crate::config::SERVER_CONFIG;

/// Strips `Forwarded`/`X-Forwarded-*` headers unless the direct TCP peer is one of the
/// configured `trusted_proxies` - otherwise a client connecting straight to this server
/// (bypassing the reverse proxy, e.g. through a leftover port mapping) could spoof the IP
/// that ends up in access logs and in `ConnectionInfo::host()`/`scheme()`. With
/// `trusted_proxies` empty (the default) every request is stripped, so this is a no-op and
/// behavior matches not having a reverse proxy at all.
pub async fn strip_untrusted_forwarded_headers(
    mut req: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, Error> {
    let trusted = req
        .peer_addr()
        .is_some_and(|addr| SERVER_CONFIG.is_trusted_proxy(addr.ip()));
    if !trusted {
        let headers = req.headers_mut();
        headers.remove(header::FORWARDED);
        headers.remove("x-forwarded-for");
        headers.remove("x-forwarded-host");
        headers.remove("x-forwarded-proto");
        // Identity headers set by the auth proxy: spoofable from a direct connection.
        headers.remove(SERVER_CONFIG.get_auth_user_header());
        headers.remove(SERVER_CONFIG.get_auth_groups_header());
    }
    next.call(req).await
}

/// With `auth_enabled`, no identity header = the request did not come through the auth
/// proxy (or came from an untrusted peer, which stripped it): 401.
pub async fn require_auth(
    req: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, Error> {
    if SERVER_CONFIG.get_auth_enabled()
        && !req
            .headers()
            .contains_key(SERVER_CONFIG.get_auth_user_header())
    {
        return Err(actix_web::error::ErrorUnauthorized(
            "authentication required",
        ));
    }
    next.call(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{middleware, test, web, App, HttpRequest};

    #[actix_web::test]
    async fn untrusted_peer_cannot_send_identity() {
        let app = test::init_service(
            App::new()
                .wrap(middleware::from_fn(strip_untrusted_forwarded_headers))
                .route(
                    "/",
                    web::get().to(|r: HttpRequest| async move {
                        format!(
                            "{}{}",
                            r.headers().contains_key("x-auth-request-user"),
                            r.headers().contains_key("x-auth-request-groups")
                        )
                    }),
                ),
        )
        .await;
        let req = test::TestRequest::get()
            .uri("/")
            .peer_addr("203.0.113.9:1234".parse().unwrap())
            .insert_header(("X-Auth-Request-User", "mallory"))
            .insert_header(("X-Auth-Request-Groups", "admin"))
            .to_request();
        let body = test::call_and_read_body(&app, req).await;
        assert_eq!(&body[..], b"falsefalse");
    }
}
