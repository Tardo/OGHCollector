// Copyright Alexandre D. Díaz
//! UI language: the `lang` cookie (`en`/`es`/`gl`) if set, else the browser's `Accept-Language`
//! (no cookie = "auto"), else English. Templates translate with the `t` filter
//! (`{{ "Overview"|t }}`): the English text is the msgid. Catalogs are standard gettext files,
//! `web/templates/i18n/{es,gl}.po` (edit them with Poedit/Weblate, check with `msgfmt --check`),
//! parsed once at startup with `polib`; an untranslated or fuzzy entry shows the English text.
//! Entries whose reference comment is `#: web/js` are also what the browser gets: the layout
//! embeds them through `i18n_js()` and `web/js/utils/i18n.mjs` (i18next) reads them.
//! Responses vary on `Accept-Language` (see `main.rs`).
use actix_web::HttpRequest;
use minijinja::value::{Rest, Value};
use minijinja::State;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::LazyLock;

pub const COOKIE: &str = "lang";
const LANGS: [&str; 3] = ["en", "es", "gl"];

#[derive(Default)]
struct Catalog {
    all: HashMap<String, String>,
    /// The subset the browser needs, as ready-to-embed JSON.
    js_json: String,
}

fn load(path: &Path) -> Result<Catalog, String> {
    let po = polib::po_file::parse(path).map_err(|e| e.to_string())?;
    let mut all = HashMap::new();
    let mut js = BTreeMap::new();
    for m in po.messages() {
        if !m.is_singular() || !m.is_translated() || m.is_fuzzy() || m.msgctxt().is_some() {
            continue;
        }
        let id = m.msgid();
        let Ok(str) = m.msgstr() else {
            continue;
        };
        if m.source().contains("web/js") {
            js.insert(id.to_string(), str.to_string());
        }
        all.insert(id.to_string(), str.to_string());
    }
    // `<` escaped: the JSON sits inside a <script> element.
    let js_json = serde_json::to_string(&js)
        .map_err(|e| e.to_string())?
        .replace('<', "\\u003c");
    Ok(Catalog { all, js_json })
}

static CATALOGS: LazyLock<HashMap<&'static str, Catalog>> = LazyLock::new(|| {
    LANGS
        .iter()
        .filter(|l| **l != "en")
        .map(|l| {
            let path = format!("web/templates/i18n/{l}.po");
            let catalog = load(Path::new(&path)).unwrap_or_else(|e| {
                log::error!("i18n catalog {path}: {e}");
                Catalog::default()
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

/// `i18n_js()` template function: the browser's catalog (JSON object) for the current render.
pub fn i18n_js(_state: &State) -> Value {
    let lang = RENDER_LANG.with(|l| l.get());
    let json = CATALOGS.get(lang).map_or("{}", |c| c.js_json.as_str());
    Value::from_safe_string(if json.is_empty() {
        "{}".into()
    } else {
        json.into()
    })
}

/// `t` template filter: translation in the language of the current render. Extra arguments fill
/// `{0}`, `{1}`... of the (translated) text and make the result HTML: the text and every argument
/// that isn't already safe markup is escaped, so `{{ "Found {0} modules"|t(n) }}` and
/// `{{ "Edit {0}"|t(code_html) }}` are both safe.
pub fn t(_state: &State, text: String, args: Rest<Value>) -> Value {
    let lang = RENDER_LANG.with(|l| l.get());
    let text = CATALOGS
        .get(lang)
        .and_then(|c| c.all.get(&text).cloned())
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
            let c = super::load(std::path::Path::new(&format!(
                "../../web/templates/i18n/{l}.po"
            )))
            .unwrap();
            assert!(c.all.len() > 600 && c.js_json.contains("Loading..."));
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
