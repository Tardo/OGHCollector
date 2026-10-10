// Copyright Alexandre D. Díaz
//! License review of a set of Odoo modules plus their `depends` closure.
//!
//! Pure logic, no DB: callers pass in the closure graph (see
//! `sqlitedb::models::dependency::FullDependencyInfo::{nodes, edges}`).
//! Odoo modules extend each other through Python inheritance, so every module
//! is treated as a derivative work of everything in its dependency closure.
//! This is a heuristic for spotting problems early, not legal advice.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const SEVERITY_ERROR: &str = "error";
pub const SEVERITY_WARNING: &str = "warning";
pub const SEVERITY_INFO: &str = "info";

/// Max closure modules named in one finding's message (the full list is in
/// `LicenseFinding::related`).
const MESSAGE_LIST_MAX: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LicenseKind {
    Lgpl3,
    Gpl2Only,
    Gpl2Plus,
    Gpl3,
    Agpl3,
    Proprietary,
    /// "Other OSI approved licence" or anything not in Odoo's documented list:
    /// can't be judged automatically.
    Unknown,
}

impl LicenseKind {
    /// Classifies a manifest `license` value. Odoo documents a closed list
    /// (AGPL-3, GPL-2, GPL-2 or any later version, GPL-3, GPL-3 or any later
    /// version, LGPL-3, Other OSI approved licence, OEEL-1, OPL-1, Other
    /// proprietary); a few common spelling variants are accepted too.
    pub fn from_manifest(license: &str) -> Self {
        match license.trim().to_ascii_lowercase().as_str() {
            "agpl-3" | "agpl-3.0" | "agplv3" | "agpl-3 or any later version" => Self::Agpl3,
            "gpl-3" | "gpl-3.0" | "gplv3" | "gpl-3 or any later version" => Self::Gpl3,
            "gpl-2 or any later version" => Self::Gpl2Plus,
            "gpl-2" | "gpl-2.0" | "gplv2" => Self::Gpl2Only,
            "lgpl-3" | "lgpl-3.0" | "lgplv3" | "lgpl-3 or any later version" => Self::Lgpl3,
            "oeel-1" | "opl-1" | "other proprietary" => Self::Proprietary,
            _ => Self::Unknown,
        }
    }

    /// Copyleft strength: a combined work must be distributed under the
    /// strongest license among its open-source parts.
    fn copyleft_rank(self) -> Option<u8> {
        match self {
            Self::Lgpl3 => Some(1),
            Self::Gpl2Only | Self::Gpl2Plus => Some(2),
            Self::Gpl3 => Some(3),
            Self::Agpl3 => Some(4),
            Self::Proprietary | Self::Unknown => None,
        }
    }

    fn is_strong_copyleft(self) -> bool {
        matches!(
            self,
            Self::Gpl2Only | Self::Gpl2Plus | Self::Gpl3 | Self::Agpl3
        )
    }

    fn is_v3(self) -> bool {
        matches!(self, Self::Lgpl3 | Self::Gpl3 | Self::Agpl3)
    }
}

/// One module of the reviewed closure.
pub struct LicenseNode<'a> {
    pub technical_name: &'a str,
    pub license: &'a str,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LicenseFinding {
    /// "error" (can't be distributed as is), "warning" (extra obligations or
    /// licenses needed) or "info" (worth knowing / review manually).
    pub severity: String,
    pub code: String,
    pub message: String,
    /// Technical name of the module the finding is about.
    pub module: String,
    /// Closure modules involved, as "technical_name (license)".
    pub related: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LicenseReport {
    /// License the combined work (reviewed modules + every dependency) must be
    /// distributed under: the strongest copyleft license among its
    /// open-source modules. None when none was recognized.
    pub effective_license: Option<String>,
    /// Module carrying `effective_license` (a reviewed module itself when it
    /// is already the strongest one).
    pub effective_license_source: Option<String>,
    /// Proprietary modules of the closure, as "technical_name (license)":
    /// each needs its own commercial license.
    pub proprietary: Vec<String>,
    /// Number of closure modules (reviewed ones included) per license.
    pub licenses: BTreeMap<String, usize>,
    /// Most severe first.
    pub findings: Vec<LicenseFinding>,
}

fn label(node: &LicenseNode) -> String {
    format!("{} ({})", node.technical_name, node.license)
}

fn list_for_message(items: &[String]) -> String {
    let shown = items
        .iter()
        .take(MESSAGE_LIST_MAX)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match items.len().saturating_sub(MESSAGE_LIST_MAX) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

// Problem of `module` (kind `a`) being a derivative of closure module `d`.
fn pair_issue(a: LicenseKind, d: LicenseKind) -> Option<(&'static str, &'static str)> {
    use LicenseKind::*;
    match (a, d) {
        (Proprietary, d) if d.is_strong_copyleft() => {
            Some((SEVERITY_ERROR, "LICENSE_PROPRIETARY_ON_COPYLEFT"))
        }
        (a, Proprietary) if a.is_strong_copyleft() => {
            Some((SEVERITY_ERROR, "LICENSE_COPYLEFT_ON_PROPRIETARY"))
        }
        (Gpl2Only, d) if d.is_v3() => Some((SEVERITY_ERROR, "LICENSE_GPL2_ONLY")),
        (a, Gpl2Only) if a.is_v3() => Some((SEVERITY_ERROR, "LICENSE_GPL2_ONLY")),
        (Lgpl3, Proprietary) => Some((SEVERITY_WARNING, "LICENSE_NEEDS_PROPRIETARY")),
        _ => None,
    }
}

fn pair_message(code: &str, module: &str, related: &str) -> String {
    match code {
        "LICENSE_PROPRIETARY_ON_COPYLEFT" => format!(
            "{module} is proprietary but depends on GPL/AGPL code: {related}. A proprietary \
             module can't be distributed as a derivative of copyleft code."
        ),
        "LICENSE_COPYLEFT_ON_PROPRIETARY" => format!(
            "{module} depends on proprietary code: {related}. The GPL/AGPL terms of the combined \
             work can't be met while it includes proprietary modules."
        ),
        "LICENSE_GPL2_ONLY" => format!(
            "{module} is combined with {related}: GPL-2-only and (L/A)GPL-3 code are \
             incompatible."
        ),
        _ => format!(
            "{module} depends on proprietary modules: {related}. Using it requires a commercial \
             license for each of them."
        ),
    }
}

/// Every module reachable from `start` through `edges` (excluding `start`
/// unless a cycle leads back to it).
fn descendants(start: i64, adjacency: &HashMap<i64, Vec<i64>>) -> HashSet<i64> {
    let mut seen = HashSet::new();
    let mut stack = vec![start];
    while let Some(id) = stack.pop() {
        for child in adjacency.get(&id).into_iter().flatten() {
            if seen.insert(*child) {
                stack.push(*child);
            }
        }
    }
    seen.remove(&start);
    seen
}

// Strongest copyleft module among `ids`; ties go to the first id given, so
// callers list the module itself first to be reported as its own source.
fn strongest<'a>(
    ids: impl Iterator<Item = &'a i64>,
    nodes: &HashMap<i64, LicenseNode>,
) -> Option<(u8, i64)> {
    let mut best: Option<(u8, i64)> = None;
    for id in ids {
        let Some(node) = nodes.get(id) else { continue };
        let Some(rank) = LicenseKind::from_manifest(node.license).copyleft_rank() else {
            continue;
        };
        if best.is_none_or(|(best_rank, _)| rank > best_rank) {
            best = Some((rank, *id));
        }
    }
    best
}

/// Reviews `roots` (the modules asked about) together with their dependency
/// closure: `nodes` must contain every module of the closure, roots
/// included, and `edges` every `(dependent, dependency)` pair of it.
pub fn review(
    nodes: &HashMap<i64, LicenseNode>,
    edges: &[(i64, i64)],
    roots: &[i64],
) -> LicenseReport {
    let mut adjacency: HashMap<i64, Vec<i64>> = HashMap::new();
    for (from, to) in edges {
        adjacency.entry(*from).or_default().push(*to);
    }
    // Sorted by name so the output is stable across HashMap orderings.
    let mut ids: Vec<i64> = nodes.keys().copied().collect();
    ids.sort_by(|a, b| nodes[a].technical_name.cmp(nodes[b].technical_name));

    let mut report = LicenseReport::default();
    for id in &ids {
        let node = &nodes[id];
        *report.licenses.entry(node.license.to_string()).or_default() += 1;
        let kind = LicenseKind::from_manifest(node.license);
        match kind {
            LicenseKind::Proprietary => report.proprietary.push(label(node)),
            LicenseKind::Unknown => report.findings.push(LicenseFinding {
                severity: SEVERITY_INFO.to_string(),
                code: "LICENSE_UNKNOWN".to_string(),
                message: format!(
                    "{} declares \"{}\", which can't be checked automatically: review its terms \
                     manually.",
                    node.technical_name, node.license
                ),
                module: node.technical_name.to_string(),
                related: Vec::new(),
            }),
            _ => {}
        }

        let closure = descendants(*id, &adjacency);
        // One finding per (module, problem), listing every module involved.
        let mut by_code: BTreeMap<(&str, &str), Vec<String>> = BTreeMap::new();
        for dep_id in ids.iter().filter(|d| closure.contains(d)) {
            let dep = &nodes[dep_id];
            if let Some(issue) = pair_issue(kind, LicenseKind::from_manifest(dep.license)) {
                by_code.entry(issue).or_default().push(label(dep));
            }
        }
        for ((severity, code), related) in by_code {
            report.findings.push(LicenseFinding {
                severity: severity.to_string(),
                code: code.to_string(),
                message: pair_message(code, &label(node), &list_for_message(&related)),
                module: node.technical_name.to_string(),
                related,
            });
        }

        // A root whose closure forces a stronger copyleft than its own.
        if roots.contains(id) {
            let own_rank = kind.copyleft_rank();
            let in_closure = ids.iter().filter(|d| closure.contains(d));
            if let (Some(own_rank), Some((rank, source_id))) = (
                own_rank,
                strongest(std::iter::once(id).chain(in_closure), nodes),
            ) {
                if rank > own_rank {
                    let source = &nodes[&source_id];
                    let agpl = LicenseKind::from_manifest(source.license) == LicenseKind::Agpl3;
                    report.findings.push(LicenseFinding {
                        // AGPL's network clause adds real obligations (offer
                        // the source to every user of a hosted instance).
                        severity: if agpl {
                            SEVERITY_WARNING
                        } else {
                            SEVERITY_INFO
                        }
                        .to_string(),
                        code: "LICENSE_EFFECTIVE_UPGRADE".to_string(),
                        message: format!(
                            "{} depends on {}: combined with its dependencies it must be \
                             distributed under {}{}.",
                            label(node),
                            label(source),
                            source.license,
                            if agpl {
                                ", including offering the source to users of a hosted instance"
                            } else {
                                ""
                            }
                        ),
                        module: node.technical_name.to_string(),
                        related: vec![label(source)],
                    });
                }
            }
        }
    }

    let roots_first = roots
        .iter()
        .chain(ids.iter().filter(|id| !roots.contains(id)));
    if let Some((_, source_id)) = strongest(roots_first, nodes) {
        let source = &nodes[&source_id];
        report.effective_license = Some(source.license.to_string());
        report.effective_license_source = Some(source.technical_name.to_string());
    }

    let rank = |severity: &str| match severity {
        SEVERITY_ERROR => 0,
        SEVERITY_WARNING => 1,
        _ => 2,
    };
    report.findings.sort_by_key(|f| rank(&f.severity));
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(mods: &[(i64, &str, &str)], edges: &[(i64, i64)], roots: &[i64]) -> LicenseReport {
        let nodes = mods
            .iter()
            .map(|(id, name, license)| {
                (
                    *id,
                    LicenseNode {
                        technical_name: name,
                        license,
                    },
                )
            })
            .collect();
        review(&nodes, edges, roots)
    }

    fn codes(report: &LicenseReport) -> Vec<(&str, &str, &str)> {
        report
            .findings
            .iter()
            .map(|f| (f.severity.as_str(), f.code.as_str(), f.module.as_str()))
            .collect()
    }

    #[test]
    fn classifies_every_documented_odoo_license() {
        use LicenseKind::*;
        for (raw, kind) in [
            ("AGPL-3", Agpl3),
            ("GPL-2", Gpl2Only),
            ("GPL-2 or any later version", Gpl2Plus),
            ("GPL-3", Gpl3),
            ("GPL-3 or any later version", Gpl3),
            ("LGPL-3", Lgpl3),
            ("Other OSI approved licence", Unknown),
            ("OEEL-1", Proprietary),
            ("OPL-1", Proprietary),
            ("Other proprietary", Proprietary),
            ("", Unknown),
        ] {
            assert_eq!(LicenseKind::from_manifest(raw), kind, "{raw}");
        }
    }

    #[test]
    fn copyleft_closure_without_conflicts_reports_effective_license_only() {
        // AGPL module on LGPL core: the common OCA case, nothing to flag.
        let report = run(
            &[(1, "sale_x", "AGPL-3"), (2, "sale", "LGPL-3")],
            &[(1, 2)],
            &[1],
        );
        assert!(report.findings.is_empty());
        assert_eq!(report.effective_license.as_deref(), Some("AGPL-3"));
        assert_eq!(report.effective_license_source.as_deref(), Some("sale_x"));
        assert_eq!(report.licenses.get("LGPL-3"), Some(&1));
    }

    #[test]
    fn lgpl_root_on_agpl_dependency_is_upgraded() {
        let report = run(
            &[
                (1, "mis_report_partner_details", "LGPL-3"),
                (2, "mis_builder", "AGPL-3"),
            ],
            &[(1, 2)],
            &[1],
        );
        assert_eq!(
            codes(&report),
            vec![(
                "warning",
                "LICENSE_EFFECTIVE_UPGRADE",
                "mis_report_partner_details"
            )]
        );
        assert_eq!(
            report.effective_license_source.as_deref(),
            Some("mis_builder")
        );
    }

    #[test]
    fn proprietary_module_with_transitive_copyleft_is_a_conflict() {
        // opl -> lgpl -> agpl: the AGPL code still ends up in the combination.
        let report = run(
            &[
                (1, "opl", "OPL-1"),
                (2, "base_lgpl", "LGPL-3"),
                (3, "agpl", "AGPL-3"),
            ],
            &[(1, 2), (2, 3)],
            &[1],
        );
        assert_eq!(report.findings[0].code, "LICENSE_PROPRIETARY_ON_COPYLEFT");
        assert_eq!(report.findings[0].related, vec!["agpl (AGPL-3)"]);
        assert_eq!(report.proprietary, vec!["opl (OPL-1)"]);
        // base_lgpl isn't a root, so its own AGPL upgrade isn't reported.
        assert!(!codes(&report).contains(&("warning", "LICENSE_EFFECTIVE_UPGRADE", "base_lgpl")));
    }

    #[test]
    fn copyleft_and_lgpl_modules_on_proprietary_code() {
        let report = run(
            &[
                (1, "agpl", "AGPL-3"),
                (2, "lgpl", "LGPL-3"),
                (3, "enterprise", "OEEL-1"),
            ],
            &[(1, 3), (2, 3)],
            &[1, 2],
        );
        assert_eq!(
            codes(&report),
            vec![
                ("error", "LICENSE_COPYLEFT_ON_PROPRIETARY", "agpl"),
                ("warning", "LICENSE_NEEDS_PROPRIETARY", "lgpl"),
            ]
        );
    }

    #[test]
    fn gpl2_only_and_v3_unknown_and_cycles() {
        let report = run(
            &[
                (1, "old", "GPL-2"),
                (2, "base", "LGPL-3"),
                (3, "odd", "Other OSI approved licence"),
            ],
            // cycle old <-> base must not loop forever
            &[(1, 2), (2, 1), (1, 3)],
            &[1],
        );
        let codes = codes(&report);
        assert!(codes.contains(&("error", "LICENSE_GPL2_ONLY", "old")));
        assert!(codes.contains(&("error", "LICENSE_GPL2_ONLY", "base")));
        assert!(codes.contains(&("info", "LICENSE_UNKNOWN", "odd")));
        assert_eq!(report.findings.last().unwrap().severity, "info");
    }
}
