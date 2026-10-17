// Copyright Alexandre D. Díaz
//! `utils::markdown` (`markdown` filter): module description → HTML via `pulldown-cmark`; raw HTML
//! shown as text, URL scheme allowlist (http/https/mailto/relative), relative images resolved
//! against the `repo_files` raw URL. Custom filters must also be registered in the template-test
//! `Environment` (`routes/module.rs`) once a template it renders uses them. `num` (thousands
//! separators) and `avatar` (initials + a name-derived hue, `partials/report.html`) are
//! display-only filters.
use actix_web::HttpRequest;
use minijinja::{context, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::sync::LazyLock;

use crate::config::SERVER_CONFIG;

/// A request's Odoo version ("17.0") as stored (170); anything else is a
/// 400, not a panic.
pub fn odoo_version(raw: &str) -> Result<u8, actix_web::Error> {
    oghutils::version::parse_odoo_version(raw)
        .ok_or_else(|| actix_web::error::ErrorBadRequest(format!("invalid odoo version: {raw:?}")))
}

static PIP_NAMES_MAP: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    let path = "files/pip_names.txt";
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            log::error!("No se pudo abrir {path}: {e}");
            return HashMap::new();
        }
    };

    let reader = BufReader::new(file);
    let mut map = HashMap::new();

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l.trim().to_string(),
            Err(_) => continue,
        };

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut parts = line.splitn(2, ',');
        let Some(key_part) = parts.next() else {
            continue;
        };
        let Some(value_part) = parts.next() else {
            continue;
        };

        let key = key_part.trim().to_ascii_lowercase();
        let value = value_part.trim().to_string();

        map.insert(key, value);
    }

    map
});

pub fn normalize_python_dep(name: String) -> String {
    let lower = name.to_ascii_lowercase();
    PIP_NAMES_MAP.get(&lower).cloned().unwrap_or(name)
}

/// Escapes text for XML content and attribute values (sitemap, Atom feeds,
/// SVG badges), dropping the control characters XML 1.0 can't carry at all -
/// commit messages end up in event text and occasionally contain them.
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// `markdown` template filter: renders a module description (OCA's
/// `readme/DESCRIPTION.md`, else the manifest `description`) as HTML. The text
/// comes from third-party repos, so raw HTML is shown as text and link/image
/// URLs other than http(s)/mailto/relative ones become `#`. Relative images
/// are resolved against `img_base`, the module folder's raw-file URL.
// ponytail: manifest descriptions are often RST or hard-wrapped text and are
// rendered as Markdown anyway (RST titles happen to be setext headings).
pub fn markdown(text: Option<String>, img_base: Option<String>) -> Value {
    use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};
    let safe_url = |url: CowStr<'static>| -> CowStr<'static> {
        // Allowlist on the scheme: anything with a `:` before the first `/?#`
        // must be one of these, so `java&#9;script:` & co. never get through.
        let lower = url.to_ascii_lowercase();
        match lower.find([':', '/', '?', '#']) {
            Some(i) if lower[i..].starts_with(':') => {
                if ["http", "https", "mailto"].contains(&&lower[..i]) {
                    url
                } else {
                    CowStr::Borrowed("#")
                }
            }
            _ => url,
        }
    };
    // Images point into the module folder: `../static/..` from DESCRIPTION.md
    // (it lives in `readme/`) or `static/..` written from the module root.
    let img_base = img_base.and_then(|base| url::Url::parse(&base).ok());
    let resolve_img = |url: CowStr<'static>| -> CowStr<'static> {
        let url = safe_url(url);
        match &img_base {
            Some(base) if url::Url::parse(&url).is_err() && !url.starts_with(['/', '#']) => base
                .join(url.strip_prefix("../").unwrap_or(&url))
                .map_or(url, |resolved| resolved.to_string().into()),
            _ => url,
        }
    };
    let text = text.unwrap_or_default();
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let events = Parser::new_ext(&text, options).map(|event| match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: safe_url(dest_url.into_static()),
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Image {
            link_type,
            dest_url: resolve_img(dest_url.into_static()),
            title,
            id,
        }),
        event => event,
    });
    let mut out = String::new();
    html::push_html(&mut out, events);
    Value::from_safe_string(out)
}

/// `1234567` → `"1,234,567"`.
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `num` filter: integers get thousands separators, anything else prints as is.
pub fn num(value: Value) -> String {
    i64::try_from(value.clone())
        .map(thousands)
        .unwrap_or_else(|_| value.to_string())
}

/// `avatar` filter: initials and a stable hue (0-359) for a committer name.
/// A parenthesized suffix ("Name (ACME)") is ignored; one word gives two letters.
pub fn avatar(name: String) -> Value {
    let words: Vec<&str> = name
        .split('(')
        .next()
        .unwrap_or_default()
        .split(|c: char| c.is_whitespace() || "-_.".contains(c))
        .filter(|w| w.chars().next().is_some_and(char::is_alphabetic))
        .collect();
    let initials: String = match words.as_slice() {
        [] => name.chars().take(1).collect(),
        [one] => one
            .chars()
            .filter(|c| c.is_alphanumeric())
            .take(2)
            .collect(),
        [first, .., last] => first.chars().take(1).chain(last.chars().take(1)).collect(),
    };
    // FNV-1a: stable across runs and platforms, unlike std's hasher.
    let hash = name.bytes().fold(0x811c_9dc5_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    context!(initials => initials.to_uppercase(), hue => hash % 360)
}

pub fn get_base_url(req: &HttpRequest) -> String {
    let conn_info = req.connection_info();
    format!("{}://{}", conn_info.scheme(), conn_info.host())
}

pub fn get_minijinja_context(req: &HttpRequest) -> Value {
    let base_url = get_base_url(req);
    context!(
        REQ_SCHEME => req.connection_info().scheme().to_string(),
        REQ_HOST => req.connection_info().host().to_string(),
        REQ_BASE_URL => base_url.clone(),
        REQ_URL => format!("{}{}", &base_url, req.path()),
        MCP_INFO_ENABLED => SERVER_CONFIG.get_mcp_info_enabled(),
        WORKFLOW_EDITOR_ENABLED => SERVER_CONFIG.get_workflow_editor_enabled(),
        SCAN_ENABLED => SERVER_CONFIG.get_scan_enabled(),
        MCP_URL => SERVER_CONFIG.get_mcp_url().clone(),
        SEO_ENABLED => SERVER_CONFIG.get_seo_enabled(),
        SEMANTIC_SEARCH_ENABLED => SERVER_CONFIG.get_semantic_search_enabled(),
        HEALTH_SCORE_ENABLED => SERVER_CONFIG.get_health_score_enabled(),
        WORKSPACES => req
            .app_data::<actix_web::web::Data<crate::workspace::Workspaces>>()
            .map(|ws| ws.names())
            .unwrap_or_default(),
        WORKSPACE => crate::workspace::current(req),
    )
}

#[cfg(test)]
mod tests {
    use super::{avatar, markdown, num, xml_escape};

    #[test]
    fn test_markdown_is_sanitized() {
        let html = markdown(
            Some(
                "# T\n\n- [ok](https://x.org) <b>raw</b>\n\n<script>alert(1)</script>\n\n\
             [a](javascript:alert(1)) [b](JaVaScript:x) [c](java&#9;script:x) \
             <javascript:alert(1)> <me@x.org> ![i](data:text/html,x) [r](../readme)"
                    .into(),
            ),
            None,
        )
        .to_string();
        assert!(html.contains("<h1>T</h1>") && html.contains("<li>"));
        assert!(html.contains(r#"<a href="https://x.org">ok</a>"#));
        assert!(html.contains("&lt;b&gt;raw") && html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script") && !html.contains("<b>"));
        assert_eq!(html.matches(r##"href="#""##).count(), 4);
        assert!(html.contains(r##"<img src="#""##));
        assert!(html.contains(r#"href="mailto:me@x.org""#));
        assert!(html.contains(r#"href="../readme""#));
        assert_eq!(markdown(None, None).to_string(), "");
        let html = markdown(
            Some("![a](../static/description/a.png) ![b](./static/b.png) ![c](https://x.org/c.png) ![d](javascript:x)".into()),
            Some("https://github.com/OCA/web/raw/18.0/web_x/".into()),
        )
        .to_string();
        assert!(html.contains(
            r#"src="https://github.com/OCA/web/raw/18.0/web_x/static/description/a.png""#
        ));
        assert!(html.contains(r#"src="https://github.com/OCA/web/raw/18.0/web_x/static/b.png""#));
        assert!(html.contains(r#"src="https://x.org/c.png""#) && html.contains(r##"src="#""##));
    }

    #[test]
    fn test_xml_escape() {
        assert_eq!(
            xml_escape("a&b<c>\"d' \u{1b}[0m\tok"),
            "a&amp;b&lt;c&gt;&quot;d&apos; [0m\tok"
        );
    }

    #[test]
    fn test_num_and_avatar() {
        assert_eq!(num(19879.into()), "19,879");
        assert_eq!(num((-1234567).into()), "-1,234,567");
        assert_eq!(num(999.into()), "999");
        assert_eq!(num(18.5.into()), "18.5");
        let initials = |name: &str| {
            avatar(name.into())
                .get_attr("initials")
                .unwrap()
                .to_string()
        };
        assert_eq!(initials("Pedro M. Baeza"), "PB");
        assert_eq!(initials("Stéphane Bidoul (ACSONE)"), "SB");
        assert_eq!(initials("OCA-git-bot"), "OB");
        assert_eq!(initials("eLBati"), "EL");
        assert_eq!(initials("Yusuke1998"), "YU");
        let hue = |name: &str| avatar(name.into()).get_attr("hue").unwrap();
        assert_eq!(hue("Pedro M. Baeza"), hue("Pedro M. Baeza"));
    }
}
