// Copyright Alexandre D. Díaz
//! Vulnerabilities (`/osv`): the OSV advisories PyPI reports for the pinned
//! Python dependencies of tracked modules, per Odoo version (one lazily
//! loaded tab each), grouped by package. A package is its PEP 503 name, not
//! the requirement string: `cryptography<37` and `cryptography==36.0.0` are
//! one package with two pins, sharing most advisories.
use actix_web::{get, web, HttpRequest, HttpResponse, Responder, Result};
use diesel::sqlite::SqliteConnection;
use minijinja::context;
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::models;

use crate::workspace::AreaPool;

// Field order is the sort order (technical name first).
#[derive(Debug, Serialize, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct OSVModuleRef {
    pub technical_name: String,
    pub org_name: String,
    pub name: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct OSVAdvisory {
    pub osv_id: String,
    pub summary: String,
    pub details: String,
    pub fixed_in: String,
    // The package's pins (specifiers) this advisory was reported for.
    pub pins: BTreeSet<String>,
    // Lowercased text the page filter matches: package, ID, summary, modules.
    pub search: String,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct OSVPackage {
    pub name: String,
    // Specifier (`<39`, `==36.0.0`) → modules requiring the package that way.
    pub pins: BTreeMap<String, BTreeSet<OSVModuleRef>>,
    pub advisories: Vec<OSVAdvisory>,
    pub module_count: usize,
}

// One Odoo-version tab's worth of content, ordered newest-first like the
// rest of the version-tabbed pages (see modules.rs).
#[derive(Debug, Serialize, Clone, Default)]
pub struct OSVVersionGroup {
    pub odoo_version: String,
    pub packages: Vec<OSVPackage>,
    pub vuln_count: usize,
    pub package_count: usize,
    pub module_count: usize,
    pub no_fix_count: usize,
}

// `cryptography>=38,<39` → (`cryptography`, `>=38,<39`).
fn split_requirement(req: &str) -> (&str, &str) {
    let at = req
        .find(|c: char| "<>=!~;[( ".contains(c))
        .unwrap_or(req.len());
    (req[..at].trim(), req[at..].trim())
}

// PEP 503: `sentry_sdk`, `Sentry-SDK` and `sentry.sdk` are one package.
fn package_key(name: &str) -> String {
    name.to_lowercase()
        .split(['-', '_', '.'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

// First line of prose of the Markdown details: GHSA texts open with a
// `### Summary`-style heading, some with a code fence; NVD-derived (PYSEC)
// ones with a "<package> is a ..." sentence, skipped.
fn summary(details: &str, package: &str) -> String {
    let mut in_fence = false;
    let line = details
        .lines()
        .map(str::trim)
        .find(|l| {
            if l.starts_with("```") {
                in_fence = !in_fence;
                return false;
            }
            !in_fence && !l.is_empty() && !l.starts_with(['#', '|'])
        })
        .unwrap_or_default();
    let intro = format!("{} is a", package.to_lowercase());
    let line = match line.to_lowercase().starts_with(&intro) {
        true => line.split_once(". ").map_or(line, |(_, rest)| rest),
        false => line,
    };
    line.replace(['`', '*'], "")
}

// Highest of the listed fixed releases (`2.4.0, 1.10.13`), compared numerically.
fn version_key(fixed_in: &str) -> Vec<u64> {
    fixed_in
        .split(',')
        .map(|v| {
            v.split(|c: char| !c.is_ascii_digit())
                .filter_map(|n| n.parse().ok())
                .collect::<Vec<u64>>()
        })
        .max()
        .unwrap_or_default()
}

fn compute_osv_groups(conn: &mut SqliteConnection) -> Vec<OSVVersionGroup> {
    // version → package key → (package, its advisories by ID).
    type ByPackage = BTreeMap<String, (OSVPackage, BTreeMap<String, OSVAdvisory>)>;
    let mut res: BTreeMap<i32, ByPackage> = BTreeMap::new();
    for row in models::dependency_osv::get_osv_info(conn) {
        let (name, pin) = split_requirement(&row.name);
        let (package, advisories) = res
            .entry(row.version_odoo)
            .or_default()
            .entry(package_key(name))
            .or_insert_with(|| {
                let package = OSVPackage {
                    name: name.to_string(),
                    ..Default::default()
                };
                (package, BTreeMap::new())
            });
        package
            .pins
            .entry(pin.to_string())
            .or_default()
            .insert(OSVModuleRef {
                technical_name: row.module_technical_name,
                org_name: row.org_name,
                name: row.module_name,
            });
        advisories
            .entry(row.osv_id.clone())
            .or_insert_with(|| OSVAdvisory {
                osv_id: row.osv_id,
                summary: summary(&row.details, name),
                details: row.details,
                fixed_in: row.fixed_in,
                pins: BTreeSet::new(),
                search: String::new(),
            })
            .pins
            .insert(pin.to_string());
    }

    // Newest Odoo version first.
    res.into_iter()
        .rev()
        .map(|(version_odoo, by_package)| {
            let mut modules = BTreeSet::new();
            let mut packages = by_package
                .into_values()
                .map(|(mut package, advisories)| {
                    let mut advisories = advisories
                        .into_values()
                        .map(|mut adv| {
                            let affected = adv
                                .pins
                                .iter()
                                .flat_map(|pin| &package.pins[pin])
                                .map(|m| m.technical_name.as_str())
                                .collect::<Vec<_>>()
                                .join(" ");
                            adv.search = format!(
                                "{} {} {} {affected}",
                                package.name, adv.osv_id, adv.summary
                            )
                            .to_lowercase();
                            adv
                        })
                        .collect::<Vec<_>>();
                    // No fix first, then the highest release to upgrade to.
                    advisories.sort_by_key(|a| {
                        (!a.fixed_in.is_empty(), Reverse(version_key(&a.fixed_in)))
                    });
                    package.advisories = advisories;
                    let package_modules = package.pins.values().flatten().collect::<BTreeSet<_>>();
                    package.module_count = package_modules.len();
                    modules.extend(package_modules.into_iter().cloned());
                    package
                })
                .collect::<Vec<_>>();
            packages.sort_by_key(|p| Reverse(p.advisories.len()));
            OSVVersionGroup {
                odoo_version: odoo_version_u8_to_string(&(version_odoo as u8)),
                vuln_count: packages.iter().map(|p| p.advisories.len()).sum(),
                package_count: packages.len(),
                module_count: modules.len(),
                no_fix_count: packages
                    .iter()
                    .flat_map(|p| &p.advisories)
                    .filter(|a| a.fixed_in.is_empty())
                    .count(),
                packages,
            }
        })
        .collect::<Vec<_>>()
}

#[get("/osv")]
pub async fn route(
    pool: AreaPool,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    let res = web::block(move || {
        let mut conn = pool.get().unwrap();
        compute_osv_groups(&mut conn)
    })
    .await?;

    tmpl_env.render(
        "pages/osv.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "osv",
                osv_info => res,
            )
        ),
    )
}

// Renders the content of a single Odoo-version tab (`partials/osv_version_content.html`)
// for osv.mjs to inject on demand when that tab is first shown, instead of
// shipping every version's packages/vulnerabilities up front.
#[get("/osv/tab/{odoo_version}")]
pub async fn route_tab(
    tmpl_env: MiniJinjaRenderer,
    pool: AreaPool,
    path: web::Path<String>,
) -> Result<HttpResponse> {
    let odoo_version = path.into_inner();
    let group = web::block(move || {
        let mut conn = pool.get().unwrap();
        compute_osv_groups(&mut conn)
            .into_iter()
            .find(|g| g.odoo_version == odoo_version)
    })
    .await?;

    let Some(version) = group else {
        return Ok(HttpResponse::NotFound().finish());
    };
    let html = tmpl_env.render(
        "partials/osv_version_content.html",
        context!(version => version),
    )?;
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_osv_helpers() {
        assert_eq!(
            split_requirement("cryptography>=38,<39"),
            ("cryptography", ">=38,<39")
        );
        assert_eq!(split_requirement("pypdf ~= 3.1"), ("pypdf", "~= 3.1"));
        assert_eq!(split_requirement("bokeh"), ("bokeh", ""));
        assert_eq!(package_key("Sentry__SDK"), package_key("sentry.sdk"));
        assert_eq!(package_key("pdfminer.six"), "pdfminer-six");
        assert_eq!(
            summary(
                "### Summary\n\n```py\nx\n```\nCalling `load_pem` **crashes**.\nMore.",
                "x"
            ),
            "Calling load_pem crashes."
        );
        assert_eq!(
            summary(
                "PyJWT is a JWT library. Prior to 2.12.0, crit is ignored.",
                "pyjwt"
            ),
            "Prior to 2.12.0, crit is ignored."
        );
        assert_eq!(summary("", "x"), "");
        assert!(version_key("2.4.0, 1.10.13") > version_key("2.3.9"));
        assert!(version_key("6.12.0") > version_key("6.6.0"));
        assert!(version_key("").is_empty());
    }
}
