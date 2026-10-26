// Copyright Alexandre D. Díaz
//! A small seeded DB for the server/mcp contract tests (their
//! dev-dependency enables the `fixture` feature). Always the same rows: their
//! `cached` stores are process-wide and keyed on query params only, so two
//! tests asking the same thing of different data would see each other's.
use std::collections::HashMap;
use std::path::PathBuf;

use diesel::sqlite::SqliteConnection;

use crate::models::module::{CommitterActivity, ManifestInfo};
use crate::models::{
    dependency_module, dependency_type, doodba_project, gh_organization, gh_repository, module,
    module_version,
};
use crate::Pool;

/// The seeded DB file, deleted on drop.
pub struct Fixture {
    dir: PathBuf,
    pub path: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A freshly seeded DB and a read pool over it. `name` must be unique per
/// test (they run in parallel).
pub fn demo(name: &str) -> (Fixture, Pool) {
    let dir = std::env::temp_dir().join(format!("ogh-fixture-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("data.db").to_string_lossy().into_owned();
    {
        let pool = crate::new_write_pool(&path);
        let mut conn = pool.get().unwrap();
        crate::run_migrations(&mut conn).unwrap();
        seed(&mut conn);
    }
    let pool = crate::new_read_pool(&path, 2);
    (Fixture { dir, path }, pool)
}

/// One stored module: what the collector's `store_module` writes, minus the
/// code analysis (only its `module_version` snapshot).
struct Seed {
    org: &'static str,
    repo: &'static str,
    technical_name: &'static str,
    version_odoo: u8,
    version_module: &'static str,
    license: &'static str,
    depends: &'static [&'static str],
    python: &'static [&'static str],
}

// `sale_demo` (AGPL-3) depends on a module of another repo and of another
// org; `web_widget_demo` exists at two Odoo versions.
const SEEDS: &[Seed] = &[
    Seed {
        org: "odoo",
        repo: "odoo",
        technical_name: "sale",
        version_odoo: 180,
        version_module: "1.2",
        license: "LGPL-3",
        depends: &[],
        python: &[],
    },
    Seed {
        org: "OCA",
        repo: "web",
        technical_name: "web_widget_demo",
        version_odoo: 170,
        version_module: "1.0.0",
        license: "LGPL-3",
        depends: &[],
        python: &[],
    },
    Seed {
        org: "OCA",
        repo: "web",
        technical_name: "web_widget_demo",
        version_odoo: 180,
        version_module: "1.1.0",
        license: "LGPL-3",
        depends: &[],
        python: &[],
    },
    Seed {
        org: "OCA",
        repo: "sale-workflow",
        technical_name: "sale_demo",
        version_odoo: 180,
        version_module: "1.0.0",
        license: "AGPL-3",
        depends: &["sale", "web_widget_demo"],
        python: &["requests"],
    },
];

fn seed(conn: &mut SqliteConnection) {
    let module_type = dependency_type::get_by_name(conn, "module").unwrap();
    let python_type = dependency_type::get_by_name(conn, "python").unwrap();
    for s in SEEDS {
        let committers = HashMap::from([(
            "Ana Dev".to_string(),
            CommitterActivity {
                total: 3,
                insertions: 10,
                deletions: 2,
                periods: HashMap::from([((2024, 1), 3)]),
            },
        )]);
        let m = module::add(
            conn,
            &ManifestInfo {
                technical_name: s.technical_name.to_string(),
                source_path: s.technical_name.to_string(),
                version_odoo: s.version_odoo,
                name: s.technical_name.replace('_', " "),
                version_module: s.version_module.to_string(),
                description: format!("The {} demo module.", s.technical_name),
                license: s.license.to_string(),
                category: "Sales".to_string(),
                installable: true,
                git_org: s.org.to_string(),
                git_repo: s.repo.to_string(),
                last_commit_hash: "abc".to_string(),
                last_commit_author: "Ana Dev".to_string(),
                last_commit_date: "2024-01-01".to_string(),
                committers,
                ..Default::default()
            },
        )
        .unwrap();
        module_version::get_or_create(conn, &m.id, s.version_module).unwrap();
        for dep in s.depends {
            dependency_module::add(conn, &module_type.id, dep, &m.id).unwrap();
        }
        for dep in s.python {
            dependency_module::add(conn, &python_type.id, dep, &m.id).unwrap();
        }
    }
    // A Doodba project (no modules of its own) enabling two of them.
    let org = gh_organization::add(conn, "acme").unwrap();
    let repo = gh_repository::add(conn, &org.id, "client-a").unwrap();
    let project = doodba_project::Model {
        gh_repository_id: repo.id,
        version_odoo: 180,
        last_commit_date: "2026-10-01T10:00:00+02:00".to_string(),
        template_src: Some("https://github.com/Tecnativa/doodba-copier-template.git".to_string()),
        template_commit: Some("v9.0.0".to_string()),
        template_commit_date: Some("2026-01-01T10:00:00+02:00".to_string()),
        template_behind: Some(1),
        template_log: r#"["abc1234 Bump pre-commit hooks"]"#.to_string(),
        answers: r#"{"postgres_version":"16"}"#.to_string(),
        base_image: Some("ghcr.io/tecnativa/doodba:18.0-onbuild".to_string()),
        repos: r#"[{"path":"web","target":"origin 18.0","remotes":{"origin":"https://github.com/OCA/web.git"},
            "patches":[{"remote":"origin","ref":"refs/pull/1/head","state":"merged"},{"remote":"origin","ref":"18.0-fix"}]}]"#
            .to_string(),
        linters: r#"[{"repo":"https://github.com/OCA/pylint-odoo","rev":"v9.0.0"}]"#.to_string(),
        dependencies: r#"{"pip":["requests==2.0.0"]}"#.to_string(),
        exposed: r#"["odoo/custom/src/repos.yaml forge.example"]"#.to_string(),
        advisories: r#"[{"package":"requests","version":"2.0.0","id":"PYSEC-2014-14","fixed_in":"2.3.0","summary":"Leaks credentials."}]"#
            .to_string(),
    };
    let addons = [
        ("web".to_string(), "web_widget_demo".to_string()),
        ("sale-workflow".to_string(), "sale_demo".to_string()),
    ];
    doodba_project::replace(conn, repo.id, 180, Some((&project, &addons))).unwrap();
}
