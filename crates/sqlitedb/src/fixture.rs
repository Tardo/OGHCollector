// Copyright Alexandre D. Díaz
//! A small seeded DB for the server/mcp contract tests (their
//! dev-dependency enables the `fixture` feature). Always the same rows: their
//! `cached` stores are process-wide and keyed on query params only, so two
//! tests asking the same thing of different data would see each other's.
use std::collections::HashMap;
use std::path::PathBuf;

use diesel::sqlite::SqliteConnection;

use crate::models::module::{CommitterActivity, ManifestInfo};
use crate::models::{dependency_module, dependency_type, module, module_version};
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
}
