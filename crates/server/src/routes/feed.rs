// Copyright Alexandre D. Díaz
use actix_web::{get, web, HttpRequest, HttpResponse, Result};
use cached::{proc_macro::cached, stores::TimedSizedCache};
use chrono::{NaiveDateTime, SecondsFormat};
use diesel::sqlite::SqliteConnection;
use serde::Deserialize;

use crate::config::SERVER_CONFIG;
use crate::utils::{get_base_url, xml_escape};
use oghutils::version::{odoo_version_u8_to_string, parse_odoo_version};
use sqlitedb::{models, Pool};

const FEED_LIMIT: i64 = 50;
const TITLE_MAX_CHARS: usize = 140;
const MAX_TYPES: usize = 20;
const MAX_QUERY_LEN: usize = 100;

/// Event types of the site-wide feed when `type` isn't given. committer/
/// author/maintainer/dependency events are ~80% of the log and would bury
/// everything else; security notices (minor findings) are opt-in too.
const DEFAULT_TYPES: &[&str] = &[
    "module",
    "migration_pr",
    "vulnerability",
    "issue",
    "repository",
    "organization",
];

#[derive(Debug, Deserialize)]
pub struct FeedRequest {
    /// Comma-separated event types, or "all".
    #[serde(rename = "type")]
    event_type: Option<String>,
    odoo_version: Option<String>,
    /// Free substring, e.g. a repository name.
    q: Option<String>,
}

struct FeedMeta {
    title: String,
    /// Absolute URL of this feed (Atom `id` + rel="self").
    self_url: String,
    /// Absolute URL of the HTML page the entries belong to.
    alternate_url: String,
}

/// Stored event dates are UTC "YYYY-MM-DD HH:MM:SS"; Atom wants RFC 3339.
fn atom_date(date: &str) -> String {
    NaiveDateTime::parse_from_str(date, "%Y-%m-%d %H:%M:%S")
        .map(|d| d.and_utc().to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn entry_title(event: &models::system_event::Model) -> String {
    let mut title: String = event.message.chars().take(TITLE_MAX_CHARS).collect();
    if event.message.chars().count() > TITLE_MAX_CHARS {
        title.push('…');
    }
    format!("[{}] {title}", event.event_type_name)
}

fn render_feed(meta: &FeedMeta, base_url: &str, events: &[models::system_event::Model]) -> String {
    let updated = events
        .first()
        .map(|e| atom_date(&e.date))
        .unwrap_or_else(|| atom_date(&sqlitedb::utils::date::get_sqlite_utc_now()));
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <feed xmlns=\"http://www.w3.org/2005/Atom\">\n\
         <title>{title}</title>\n\
         <id>{self_url}</id>\n\
         <link rel=\"self\" type=\"application/atom+xml\" href=\"{self_url}\"/>\n\
         <link rel=\"alternate\" type=\"text/html\" href=\"{alternate}\"/>\n\
         <updated>{updated}</updated>\n\
         <author><name>Odoo Module Metadata</name></author>\n\
         <generator>OGHCollector</generator>\n",
        title = xml_escape(&meta.title),
        self_url = xml_escape(&meta.self_url),
        alternate = xml_escape(&meta.alternate_url),
    );
    for event in events {
        // Messages are always plain text here: legacy `is_html` rows (if
        // any) are escaped like the rest rather than trusted as markup.
        xml.push_str(&format!(
            "<entry>\n\
             <title>{title}</title>\n\
             <id>{id}</id>\n\
             <link rel=\"alternate\" type=\"text/html\" href=\"{alternate}\"/>\n\
             <updated>{updated}</updated>\n\
             <category term=\"{category}\"/>\n\
             <content type=\"text\">{content}</content>\n\
             </entry>\n",
            title = xml_escape(&entry_title(event)),
            id = xml_escape(&format!("{base_url}/logs#event-{}", event.id)),
            alternate = xml_escape(&meta.alternate_url),
            updated = atom_date(&event.date),
            category = xml_escape(&event.event_type_name),
            content = xml_escape(&event.message),
        ));
    }
    xml.push_str("</feed>\n");
    xml
}

// Events only change when the collector runs, and feed readers poll: cache
// the rendered feed per (host, filters) for cache_ttl, like the sitemap.
#[cached(
    ty = "TimedSizedCache<String, String>",
    key = "String",
    create = r#"
        {
            let ttl_secs = *SERVER_CONFIG.get_cache_ttl();
            TimedSizedCache::with_size_and_lifespan_and_refresh(256, std::time::Duration::from_secs(ttl_secs), false)
        }
    "#,
    convert = r#"{ format!("{}|{}|{event_types:?}|{contains:?}", meta.self_url, base_url) }"#
)]
fn build_feed(
    conn: &mut SqliteConnection,
    meta: &FeedMeta,
    base_url: &str,
    event_types: &[String],
    contains: &[String],
) -> String {
    let events = models::system_event::get_feed(conn, event_types, contains, FEED_LIMIT);
    render_feed(meta, base_url, &events)
}

fn atom_response(body: String) -> HttpResponse {
    HttpResponse::Ok()
        .content_type("application/atom+xml; charset=utf-8")
        .body(body)
}

fn bad_request(msg: String) -> HttpResponse {
    HttpResponse::BadRequest()
        .content_type("text/plain; charset=utf-8")
        .body(msg)
}

/// Site-wide activity feed: new/updated modules, migration PRs,
/// vulnerabilities... filterable by event type, Odoo version and substring.
#[get("/feed.atom")]
pub async fn route(
    pool: web::Data<Pool>,
    req: HttpRequest,
    info: web::Query<FeedRequest>,
) -> Result<HttpResponse> {
    let info = info.into_inner();
    let event_types: Vec<String> = match info.event_type.as_deref().map(str::trim) {
        None | Some("") => DEFAULT_TYPES.iter().map(|t| t.to_string()).collect(),
        Some("all") => Vec::new(),
        Some(raw) => raw
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from)
            .collect(),
    };
    if event_types.len() > MAX_TYPES {
        return Ok(bad_request(format!("too many types (max {MAX_TYPES})")));
    }
    let mut contains = Vec::new();
    let mut title = "Odoo Module Metadata: activity".to_string();
    if let Some(raw) = info.odoo_version.as_deref().filter(|v| !v.is_empty()) {
        let Some(version) = parse_odoo_version(raw) else {
            return Ok(bad_request(format!("invalid odoo_version: {raw:?}")));
        };
        let version = odoo_version_u8_to_string(&version);
        title.push_str(&format!(" [{version}]"));
        contains.push(format!("[{version}]"));
    }
    if let Some(q) = info.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        if q.chars().count() > MAX_QUERY_LEN {
            return Ok(bad_request(format!(
                "q too long (max {MAX_QUERY_LEN} characters)"
            )));
        }
        title.push_str(&format!(" matching \"{q}\""));
        contains.push(q.to_string());
    }

    let base_url = get_base_url(&req);
    let meta = FeedMeta {
        title,
        self_url: format!("{base_url}{}", req.uri()),
        alternate_url: format!("{base_url}/logs"),
    };
    let body = web::block(move || {
        let mut conn = pool.get().unwrap();
        build_feed(&mut conn, &meta, &base_url, &event_types, &contains)
    })
    .await?;
    Ok(atom_response(body))
}

/// Everything logged about one module (every type, every Odoo version).
/// Events name modules as `'technical_name'` but not always with their
/// organization, so a technical name shared by two organizations mixes both.
#[get("/module/{org}/{technical_name}/feed.atom")]
pub async fn route_module(
    pool: web::Data<Pool>,
    req: HttpRequest,
    path: web::Path<(String, String)>,
) -> Result<HttpResponse> {
    let (org, technical_name) = path.into_inner();
    let base_url = get_base_url(&req);
    let module_url = url::Url::parse(&base_url).ok().map(|mut url| {
        url.path_segments_mut()
            .unwrap()
            .extend(["module", &org, &technical_name]);
        url.to_string()
    });
    let meta = FeedMeta {
        title: format!("{technical_name} ({org}): Odoo Module Metadata activity"),
        self_url: format!("{base_url}{}", req.uri()),
        alternate_url: module_url.unwrap_or_else(|| format!("{base_url}/logs")),
    };
    let contains = vec![format!("'{technical_name}'")];
    let body = web::block(move || {
        let mut conn = pool.get().unwrap();
        build_feed(&mut conn, &meta, &base_url, &[], &contains)
    })
    .await?;
    Ok(atom_response(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_is_well_formed_and_escaped() {
        let meta = FeedMeta {
            title: "a & b".to_string(),
            self_url: "http://h/feed.atom?type=module&q=x".to_string(),
            alternate_url: "http://h/logs".to_string(),
        };
        let event = models::system_event::Model {
            id: 7,
            message: "Module '<web>' [18.0] updated \u{1}".to_string(),
            date: "2026-10-10 12:30:00".to_string(),
            event_type_id: 1,
            event_type_name: "module".to_string(),
            severity: "info".to_string(),
            is_html: true,
        };
        let xml = render_feed(&meta, "http://h", &[event]);
        assert!(xml.contains("<updated>2026-10-10T12:30:00Z</updated>"));
        assert!(xml.contains("href=\"http://h/feed.atom?type=module&amp;q=x\""));
        assert!(
            xml.contains("<title>[module] Module &apos;&lt;web&gt;&apos; [18.0] updated </title>")
        );
        assert!(xml.contains("<id>http://h/logs#event-7</id>"));
        assert!(!xml.contains('\u{1}'));
    }
}
