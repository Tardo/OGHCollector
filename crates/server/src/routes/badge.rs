// Copyright Alexandre D. Díaz
use actix_web::{get, web, HttpResponse, Result};
use diesel::sqlite::SqliteConnection;
use serde::Deserialize;

use crate::utils::xml_escape;
use oghutils::version::{odoo_version_u8_to_string, parse_odoo_version};
use sqlitedb::{models, Pool};

// shields.io's palette, so these sit well next to other README badges.
const GREEN: &str = "#4c1";
const YELLOW: &str = "#dfb317";
const ORANGE: &str = "#fe7d37";
const RED: &str = "#e05d44";
const BLUE: &str = "#007ec6";
const GREY: &str = "#9f9f9f";

#[derive(Debug, Deserialize)]
pub struct BadgeRequest {
    odoo_version: Option<String>,
}

// ponytail: rough Verdana 11px advance widths; `textLength` in the SVG
// stretches/squeezes the text to whatever this estimates, so being a few
// pixels off only changes the padding, never overflows.
fn text_width(text: &str) -> f32 {
    text.chars()
        .map(|c| match c {
            'i' | 'l' | 'j' | '.' | ',' | ':' | ';' | '\'' | '|' | '!' => 3.6,
            ' ' | 'f' | 't' | 'r' | '(' | ')' | '-' | '/' | '·' => 4.6,
            'm' | 'w' | 'M' | 'W' => 9.6,
            c if c.is_ascii_uppercase() => 7.6,
            c if c.is_ascii_digit() => 7.0,
            _ => 6.6,
        })
        .sum()
}

/// Flat two-part badge (label on grey, message on `color`). Only text built
/// from DB data and fixed strings ends up here - never raw request input -
/// and it's escaped anyway: an SVG served from this origin can run script.
fn render_badge(label: &str, message: &str, color: &str) -> String {
    let label_text = text_width(label).round();
    let message_text = text_width(message).round();
    let label_w = label_text + 10.0;
    let message_w = message_text + 10.0;
    let width = label_w + message_w;
    let label_x = label_w / 2.0;
    let message_x = label_w + message_w / 2.0;
    let label = xml_escape(label);
    let message = xml_escape(message);
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="20" role="img" aria-label="{label}: {message}"><title>{label}: {message}</title><linearGradient id="s" x2="0" y2="100%"><stop offset="0" stop-color="#bbb" stop-opacity=".1"/><stop offset="1" stop-opacity=".1"/></linearGradient><clipPath id="r"><rect width="{width}" height="20" rx="3" fill="#fff"/></clipPath><g clip-path="url(#r)"><rect width="{label_w}" height="20" fill="#555"/><rect x="{label_w}" width="{message_w}" height="20" fill="{color}"/><rect width="{width}" height="20" fill="url(#s)"/></g><g fill="#fff" text-anchor="middle" font-family="Verdana,Geneva,DejaVu Sans,sans-serif" text-rendering="geometricPrecision" font-size="11"><text x="{label_x}" y="15" fill="#010101" fill-opacity=".3" textLength="{label_text}">{label}</text><text x="{label_x}" y="14" textLength="{label_text}">{label}</text><text x="{message_x}" y="15" fill="#010101" fill-opacity=".3" textLength="{message_text}">{message}</text><text x="{message_x}" y="14" textLength="{message_text}">{message}</text></g></svg>"##
    )
}

/// "14.0–16.0, 18.0": runs of consecutive versions, consecutive meaning
/// adjacent among every version the DB knows (`all_versions`, ascending) -
/// 9.0 is the next version after 8.0, not 8.1.
fn version_ranges(module_versions: &[i32], all_versions: &[i32]) -> String {
    let mut runs: Vec<(i32, i32)> = Vec::new();
    let mut previous_index: Option<usize> = None;
    for (index, version) in all_versions.iter().enumerate() {
        if !module_versions.contains(version) {
            continue;
        }
        match (previous_index, runs.last_mut()) {
            (Some(previous), Some(run)) if previous + 1 == index => run.1 = *version,
            _ => runs.push((*version, *version)),
        }
        previous_index = Some(index);
    }
    let fmt = |v: i32| odoo_version_u8_to_string(&(v as u8));
    runs.into_iter()
        .map(|(from, to)| {
            if from == to {
                fmt(from)
            } else {
                format!("{}–{}", fmt(from), fmt(to))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn badge_content(
    conn: &mut SqliteConnection,
    org: &str,
    technical_name: &str,
    kind: &str,
    odoo_version: Option<u8>,
) -> (String, String, &'static str) {
    let not_found = || ("odoo module".to_string(), "not found".to_string(), GREY);
    let modules =
        models::module::get_by_technical_name_organization_name(conn, technical_name, org);
    match kind {
        "versions" => {
            if modules.is_empty() {
                return not_found();
            }
            let mut all_versions = models::module::get_odoo_versions(conn);
            all_versions.reverse();
            let mine: Vec<i32> = modules.iter().map(|m| m.version_odoo).collect();
            (
                "odoo".to_string(),
                version_ranges(&mine, &all_versions),
                BLUE,
            )
        }
        "status" => {
            // Default: the newest Odoo version the DB knows about - "is it
            // ported to the latest release yet?".
            let target = match odoo_version {
                Some(v) => v as i32,
                None => match models::module::get_odoo_versions(conn).first() {
                    Some(v) => *v,
                    None => return not_found(),
                },
            };
            let label = format!("odoo {}", odoo_version_u8_to_string(&(target as u8)));
            if modules.iter().any(|m| m.version_odoo == target) {
                return (label, "available".to_string(), GREEN);
            }
            let pr_open = models::pull_request::get_by_technical_name_organization_name(
                conn,
                technical_name,
                org,
            )
            .iter()
            .any(|pr| pr.version_odoo == target);
            if pr_open {
                (label, "migration PR open".to_string(), YELLOW)
            } else if modules.is_empty() {
                not_found()
            } else {
                (label, "not migrated".to_string(), GREY)
            }
        }
        "security" | "license" => {
            let module = match odoo_version {
                Some(v) => modules.into_iter().find(|m| m.version_odoo == v as i32),
                None => modules.into_iter().max_by_key(|m| m.version_odoo),
            };
            let Some(module) = module else {
                return not_found();
            };
            if kind == "security" {
                let warnings = models::module_version::resolve_current(conn, &module)
                    .map(|mv| {
                        models::module_security_warning::get_by_module_version_id(conn, &mv.id)
                    })
                    .unwrap_or_default();
                let count =
                    |severity: &str| warnings.iter().filter(|w| w.severity == severity).count();
                let grave = count(models::module_security_warning::SEVERITY_ERROR);
                let minor = count(models::module_security_warning::SEVERITY_WARNING);
                // Labelled as what it is - automated static analysis - so a
                // clean result doesn't read like a security audit.
                let (message, color) = match (grave, minor) {
                    (0, 0) => ("no findings".to_string(), GREEN),
                    (0, minor) => (format!("{minor} minor"), YELLOW),
                    (grave, 0) => (format!("{grave} grave"), RED),
                    (grave, minor) => (format!("{grave} grave · {minor} minor"), RED),
                };
                ("static analysis".to_string(), message, color)
            } else {
                let review = models::dependency::get_full_dependency_info(conn, &module)
                    .license_review(&[module.id]);
                let license = module.license_str();
                let color = match review.findings.first().map(|f| f.severity.as_str()) {
                    Some(oghutils::license::SEVERITY_ERROR) => RED,
                    Some(oghutils::license::SEVERITY_WARNING) => ORANGE,
                    _ => BLUE,
                };
                // Only when its dependencies force a stronger copyleft on
                // this (open-source) module - see LICENSE_EFFECTIVE_UPGRADE.
                let upgraded = review
                    .findings
                    .iter()
                    .any(|f| f.code == "LICENSE_EFFECTIVE_UPGRADE");
                let message = match review.effective_license {
                    Some(effective) if upgraded => format!("{license} → {effective}"),
                    _ => license.to_string(),
                };
                ("license".to_string(), message, color)
            }
        }
        _ => ("badge".to_string(), "unknown type".to_string(), GREY),
    }
}

/// README badges: `versions` (Odoo versions carrying the module), `status`
/// (merged / migration PR / missing at `odoo_version`, default the newest),
/// `security` (static-analysis findings) and `license` (license plus the
/// effective one of its dependency closure) - the last two at `odoo_version`,
/// default the module's newest. Unknown modules/kinds still get a 200 grey
/// badge: a 404 would be swapped for the HTML not-found page and show up as a
/// broken image.
#[get("/badge/{org}/{technical_name}/{kind}.svg")]
pub async fn route(
    pool: web::Data<Pool>,
    path: web::Path<(String, String, String)>,
    info: web::Query<BadgeRequest>,
) -> Result<HttpResponse> {
    let (org, technical_name, kind) = path.into_inner();
    let odoo_version = info.odoo_version.as_deref().and_then(parse_odoo_version);
    let svg = web::block(move || {
        let mut conn = pool.get().unwrap();
        let (label, message, color) =
            badge_content(&mut conn, &org, &technical_name, &kind, odoo_version);
        render_badge(&label, &message, color)
    })
    .await?;
    Ok(HttpResponse::Ok()
        .content_type("image/svg+xml; charset=utf-8")
        .body(svg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test as awtest, App};

    #[test]
    fn version_ranges_follow_known_versions() {
        let all = [61, 70, 80, 90, 100, 110, 120];
        assert_eq!(version_ranges(&[80, 90, 100, 120], &all), "8.0–10.0, 12.0");
        assert_eq!(version_ranges(&[61], &all), "6.1");
        assert_eq!(version_ranges(&[], &all), "");
    }

    #[test]
    fn badge_svg_is_escaped_and_sized() {
        let svg = render_badge("license", "<AGPL-3> & co", RED);
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.contains("&lt;AGPL-3&gt; &amp; co"));
        assert!(!svg.contains("<AGPL"));
        assert!(svg.contains(&format!("fill=\"{RED}\"")));
    }

    #[actix_web::test]
    async fn badge_route_pattern_splits_the_svg_extension() {
        #[get("/badge/{org}/{technical_name}/{kind}.svg")]
        async fn probe(path: web::Path<(String, String, String)>) -> HttpResponse {
            let (org, name, kind) = path.into_inner();
            HttpResponse::Ok().body(format!("{org}|{name}|{kind}"))
        }
        let app = awtest::init_service(App::new().service(probe)).await;
        let req = awtest::TestRequest::get()
            .uri("/badge/OCA/web_responsive/versions.svg")
            .to_request();
        let body = awtest::call_and_read_body(&app, req).await;
        assert_eq!(body, "OCA|web_responsive|versions");
    }
}
