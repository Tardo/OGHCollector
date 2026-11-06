// Copyright Alexandre D. Díaz
//! UI language: the `lang` cookie (`en`/`es`/`gl`) if set, else the browser's `Accept-Language`
//! (no cookie = "auto"), else English. Templates translate with the `t` filter
//! (`{{ "Overview"|t }}`): the English text is the key, catalogs are
//! `web/templates/i18n/{es,gl}.json` (English → translation, read once at startup; a missing key
//! shows the English text). Responses vary on `Accept-Language` (see `main.rs`).
use actix_web::HttpRequest;
use minijinja::value::{Rest, Value};
use minijinja::State;
use std::collections::HashMap;
use std::sync::LazyLock;

pub const COOKIE: &str = "lang";
const LANGS: [&str; 3] = ["en", "es", "gl"];

type Catalog = HashMap<String, String>;

static CATALOGS: LazyLock<HashMap<&'static str, Catalog>> = LazyLock::new(|| {
    LANGS
        .iter()
        .filter(|l| **l != "en")
        .map(|l| {
            let path = format!("web/templates/i18n/{l}.json");
            let catalog = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
                .unwrap_or_else(|e| {
                    log::error!("i18n catalog {path}: {e}");
                    Catalog::new()
                });
            (*l, catalog)
        })
        .collect()
});

fn supported(tag: &str) -> Option<&'static str> {
    let primary = tag.split(['-', '_']).next()?.to_ascii_lowercase();
    LANGS.iter().copied().find(|l| *l == primary)
}

/// Picks from an `Accept-Language` value the supported language with the highest q.
fn from_accept(header: &str) -> Option<&'static str> {
    let mut best: Option<(f32, &'static str)> = None;
    for part in header.split(',') {
        let mut it = part.split(';');
        let Some(lang) = it.next().and_then(|t| supported(t.trim())) else {
            continue;
        };
        let q = it
            .find_map(|p| p.trim().strip_prefix("q=").map(str::to_string))
            .and_then(|q| q.parse::<f32>().ok())
            .unwrap_or(1.0);
        if q > 0.0 && best.is_none_or(|(bq, _)| q > bq) {
            best = Some((q, lang));
        }
    }
    best.map(|(_, l)| l)
}

pub fn current(req: &HttpRequest) -> &'static str {
    req.cookie(COOKIE)
        .and_then(|c| supported(c.value()))
        .or_else(|| {
            let h = req.headers().get("accept-language")?.to_str().ok()?;
            from_accept(h)
        })
        .unwrap_or("en")
}

thread_local! {
    static RENDER_LANG: std::cell::Cell<&'static str> = const { std::cell::Cell::new("en") };
}

/// Language of the render about to run on this thread (`MiniJinjaRenderer::render`, which is
/// synchronous): macros don't see the render context, so `t` can't read it from there.
pub fn set_render_lang(lang: &'static str) {
    RENDER_LANG.with(|l| l.set(lang));
}

/// `t` template filter: translation in the language of the current render. Extra arguments fill
/// `{0}`, `{1}`... of the (translated) text and make the result HTML: the text and every argument
/// that isn't already safe markup is escaped, so `{{ "Found {0} modules"|t(n) }}` and
/// `{{ "Edit {0}"|t(code_html) }}` are both safe.
pub fn t(_state: &State, text: String, args: Rest<Value>) -> Value {
    let lang = RENDER_LANG.with(|l| l.get());
    let text = CATALOGS
        .get(lang)
        .and_then(|c| c.get(&text).cloned())
        .unwrap_or(text);
    if args.is_empty() {
        return Value::from(text);
    }
    let mut out = String::new();
    let mut rest = escape(&text);
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let arg = tail
            .find('}')
            .and_then(|j| tail[1..j].parse::<usize>().ok().map(|n| (j, n)))
            .and_then(|(j, n)| args.get(n).map(|v| (j, v)));
        match arg {
            Some((j, v)) => {
                out.push_str(&if v.is_safe() {
                    v.to_string()
                } else {
                    escape(&v.to_string())
                });
                rest = tail[j + 1..].to_string();
            }
            None => {
                out.push('{');
                rest = tail[1..].to_string();
            }
        }
    }
    out.push_str(&rest);
    Value::from_safe_string(out)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

#[cfg(test)]
mod tests {
    use super::{escape, from_accept};

    #[test]
    fn t_fills_placeholders_and_escapes() {
        let mut env = minijinja::Environment::new();
        env.add_filter("t", super::t);
        env.add_template_owned(
            "x.html",
            r#"{{ "A <b> {0} & {1}"|t(v, c|safe) }}|{{ "a & b"|t }}"#,
        )
        .unwrap();
        let out = env
            .get_template("x.html")
            .unwrap()
            .render(minijinja::context!(v => "<i>", c => "<code>z</code>"))
            .unwrap();
        assert_eq!(out, "A &lt;b&gt; &lt;i&gt; &amp; <code>z</code>|a &amp; b");
        assert_eq!(escape("'"), "&#x27;");
    }

    #[test]
    fn every_template_compiles_and_catalogs_parse() {
        let mut env = minijinja::Environment::new();
        env.set_loader(minijinja::path_loader("../../web/templates"));
        env.add_filter("t", super::t);
        for entry in walkdir("../../web/templates") {
            let name = entry.trim_start_matches("../../web/templates/");
            if name.ends_with(".html") {
                env.get_template(name)
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
            }
        }
        for l in ["es", "gl"] {
            let s = std::fs::read_to_string(format!("../../web/templates/i18n/{l}.json")).unwrap();
            serde_json::from_str::<super::Catalog>(&s).unwrap();
        }
    }

    fn walkdir(dir: &str) -> Vec<String> {
        let mut out = vec![];
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path().to_string_lossy().into_owned();
            if e.path().is_dir() {
                out.extend(walkdir(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    #[test]
    fn accept_language_picks_best_supported() {
        assert_eq!(from_accept("gl-ES,gl;q=0.9,en;q=0.8"), Some("gl"));
        assert_eq!(from_accept("fr, es;q=0.5, en;q=0.4"), Some("es"));
        assert_eq!(from_accept("fr, de"), None);
        assert_eq!(from_accept("es;q=0, en;q=0.1"), Some("en"));
    }
}
