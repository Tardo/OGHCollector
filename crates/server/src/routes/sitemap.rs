// Copyright Alexandre D. Díaz
use actix_web::{get, web, HttpRequest, HttpResponse, Result};
use cached::{proc_macro::cached, stores::TimedSizedCache};
use diesel::sqlite::SqliteConnection;
use url::Url;

use crate::config::SERVER_CONFIG;
use crate::utils::get_base_url;
use sqlitedb::{models, Pool};

// Only the pages a search engine should actually index: static, no
// query-string variants, no per-user/localStorage-only pages like
// /favorites or /pack, no dev-tool pages (doodba/osv/logs/api/mcp, hidden
// behind the navbar's `dev-only` user/dev mode switch).
const STATIC_PATHS: &[&[&str]] = &[&[], &["modules"], &["committers"], &["atlas"]];

const EMPTY_URLSET: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
     <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\"></urlset>\n";

// `Url::path_segments_mut` percent-encodes each segment, so an org/module/
// committer name with e.g. a space or `&` still produces a valid URL.
fn page_url(base: &Url, segments: &[&str]) -> String {
    let mut url = base.clone();
    url.path_segments_mut().unwrap().extend(segments);
    xml_escape(url.as_ref())
}

// `&` survives path percent-encoding (it's not in the URL path
// percent-encode set), but XML text content still needs it escaped.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// Same whole-page cache pattern as modules.rs's compute_modules_page_data:
// module::list/committer::list scan the full catalog (tens of thousands of
// rows), and the result is identical for every visitor at a given host, so
// there's no reason to redo it more often than cache_ttl - keyed on `base`
// (not a single dummy key like modules.rs) so a multi-host deployment
// (e.g. staging + prod behind the same binary) doesn't leak one host's URLs
// into another's sitemap.
#[cached(
    type = "TimedSizedCache<String, String>",
    key = "String",
    create = r#"
        {
            let ttl_secs = *SERVER_CONFIG.get_cache_ttl();
            TimedSizedCache::with_size_and_lifespan_and_refresh(8, ttl_secs, true)
        }
    "#,
    convert = r#"{ base.to_string() }"#
)]
fn build_sitemap(base: &Url, conn: &mut SqliteConnection) -> String {
    let mut body = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for segments in STATIC_PATHS {
        body.push_str(&format!(
            "  <url><loc>{}</loc></url>\n",
            page_url(base, segments)
        ));
    }
    for m in models::module::list(conn) {
        let loc = page_url(base, &["module", &m.org_name, &m.technical_name]);
        body.push_str(&format!("  <url><loc>{loc}</loc></url>\n"));
    }
    for c in models::committer::list(conn) {
        let loc = page_url(base, &["committer", &c.name]);
        body.push_str(&format!("  <url><loc>{loc}</loc></url>\n"));
    }
    body.push_str("</urlset>\n");
    body
}

#[get("/sitemap.xml")]
pub async fn route(pool: web::Data<Pool>, req: HttpRequest) -> Result<HttpResponse> {
    // Mirrors robots.txt's seo_enabled gate: a site that tells crawlers to
    // stay out has no use for an expensive full-catalog sitemap either.
    if !SERVER_CONFIG.get_seo_enabled() {
        return Ok(HttpResponse::Ok()
            .content_type("application/xml; charset=utf-8")
            .body(EMPTY_URLSET));
    }

    // Host is attacker-controllable input (see get_base_url) - fall back to
    // an empty sitemap rather than unwrap-panicking the request on a
    // malformed Host header.
    let Ok(base) = Url::parse(&get_base_url(&req)) else {
        return Ok(HttpResponse::Ok()
            .content_type("application/xml; charset=utf-8")
            .body(EMPTY_URLSET));
    };
    let body = web::block(move || {
        let mut conn = pool.get().unwrap();
        build_sitemap(&base, &mut conn)
    })
    .await?;

    Ok(HttpResponse::Ok()
        .content_type("application/xml; charset=utf-8")
        .body(body))
}

#[cfg(test)]
mod tests {
    use super::{page_url, xml_escape};
    use url::Url;

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("a&b<c>"), "a&amp;b&lt;c&gt;");
    }

    #[test]
    fn test_page_url_encodes_and_escapes_committer_name() {
        let base = Url::parse("https://example.com").unwrap();
        assert_eq!(
            page_url(&base, &["committer", "A & B Bot"]),
            "https://example.com/committer/A%20&amp;%20B%20Bot"
        );
    }
}
