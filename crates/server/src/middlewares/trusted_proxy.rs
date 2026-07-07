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
    }
    next.call(req).await
}
