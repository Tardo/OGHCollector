use minijinja::{context, Value};
use actix_web::HttpRequest;

pub fn get_minijinja_context(req: &HttpRequest) -> Value {
    let scheme = req.connection_info().scheme().to_string();
    let host = req.connection_info().host().to_string();
    context!(
        REQ_SCHEME => scheme.clone(),
        REQ_HOST => host.clone(),
        REQ_BASE_URL => format!("{}://{}", &scheme, &host),
    )
}