// Copyright Alexandre D. Díaz
//! What the default module processing leaves in the DB, on a fixed git
//! fixture: the reference the default workflows must reproduce exactly.
//! `OGH_GOLDEN_RECORD=1 cargo test -p collector golden` rewrites the
//! expected snapshot (`golden.snapshot`); review the diff before committing.
use std::fs;
use std::path::{Path, PathBuf};

use diesel::sql_types::Text;
use diesel::{QueryableByName, RunQueryDsl};
use duct::cmd;
use sqlitedb::DbSqliteConnection as SqliteConnection;

use crate::gitclient::RepoInfo;

const SNAPSHOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/golden.snapshot");

/// A repo whose commits get the same hashes on every run.
pub struct Fixture {
    pub dir: PathBuf,
    pub repo: PathBuf,
}

impl Fixture {
    pub fn new(name: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("ogh-golden-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let fixture = Fixture { dir, repo };
        fixture.git(&["init", "-q", "-b", "18.0"]);
        fixture
    }

    pub fn git(&self, args: &[&str]) {
        cmd("git", args)
            .dir(&self.repo)
            // No user config (commit signing, hooks, templates).
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Ana Dev")
            .env("GIT_AUTHOR_EMAIL", "ana@example.com")
            .env("GIT_AUTHOR_DATE", "2024-01-01T00:00:00Z")
            .env("GIT_COMMITTER_NAME", "Ana Dev")
            .env("GIT_COMMITTER_EMAIL", "ana@example.com")
            .env("GIT_COMMITTER_DATE", "2024-01-01T00:00:00Z")
            .stdout_null()
            .stderr_null()
            .run()
            .unwrap();
    }

    pub fn write(&self, path: &str, content: &str) {
        let path = self.repo.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// Commits and moves `origin/18.0` along, like a fetched clone.
    pub fn commit(&self, msg: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
        self.git(&["update-ref", "refs/remotes/origin/18.0", "HEAD"]);
    }

    pub fn repo_infos(&self) -> Vec<RepoInfo> {
        vec![RepoInfo {
            name: "repo".to_string(),
            org: "org".to_string(),
            clone_path: self.repo.to_string_lossy().to_string(),
            full_path: "org/repo".to_string(),
            url: "https://github.com/org/repo.git".to_string(),
        }]
    }

    pub fn db(&self) -> sqlitedb::Pool {
        let pool = sqlitedb::new_write_pool(self.dir.join("data.db").to_str().unwrap());
        sqlitedb::run_migrations(&mut pool.get().unwrap()).unwrap();
        pool
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// First commit: a module with dependencies, a lax ACL, caller-controlled SQL
/// and an old view tag; one with a manifest of another Odoo version; one
/// that the second commit removes.
pub fn first_commit(f: &Fixture) {
    f.write(
        "mod_a/__manifest__.py",
        "{'name': 'Module A', 'version': '18.0.1.0.0', 'author': 'Acme, Ana Dev',
          'maintainers': ['ana'], 'license': 'LGPL-3', 'category': 'Sales',
          'depends': ['base', 'sale'],
          'external_dependencies': {'python': ['requests'], 'bin': ['wkhtmltopdf']},
          'data': ['security/ir.model.access.csv', 'views/a.xml']}",
    );
    f.write(
        "mod_a/models/a.py",
        "from odoo import fields, models\n\n\
         class Thing(models.Model):\n    _name = 'a.thing'\n    name = fields.Char()\n\n\
         \x20   def lookup(self, name):\n        self.env.cr.execute(\"SELECT id FROM a_thing WHERE name = '%s'\" % name)\n",
    );
    f.write(
        "mod_a/security/ir.model.access.csv",
        "id,name,model_id:id,group_id:id,perm_read,perm_write,perm_create,perm_unlink\n\
         access_a_thing_all,a.thing all,model_a_thing,,1,1,1,1\n",
    );
    f.write(
        "mod_a/views/a.xml",
        "<odoo><record id=\"view_a_thing_tree\" model=\"ir.ui.view\"><field name=\"model\">a.thing</field>\
         <field name=\"arch\" type=\"xml\"><tree><field name=\"name\"/></tree></field></record></odoo>\n",
    );
    f.write(
        "mod_b/__manifest__.py",
        "{'name': 'Module B', 'version': '17.0.1.0.0', 'depends': ['mod_a']}",
    );
    f.write(
        "mod_c/__manifest__.py",
        "{'name': 'Module C', 'version': '18.0.1.0.0'}",
    );
    f.commit("first");
}

/// Second commit: `mod_a` bumps its version and swaps dependencies, `mod_c` goes.
pub fn second_commit(f: &Fixture) {
    f.write(
        "mod_a/__manifest__.py",
        "{'name': 'Module A', 'version': '18.0.1.1.0', 'author': 'Acme',
          'license': 'LGPL-3', 'category': 'Sales', 'depends': ['base'],
          'external_dependencies': {'python': ['lxml']},
          'data': ['security/ir.model.access.csv', 'views/a.xml']}",
    );
    fs::remove_dir_all(f.repo.join("mod_c")).unwrap();
    f.commit("second");
}

#[derive(QueryableByName)]
struct Lines {
    #[diesel(sql_type = Text)]
    v: String,
}

/// Every stored row the module processing writes, without dates or ids.
pub fn snapshot(conn: &mut SqliteConnection) -> String {
    let sections = [
        ("module", "SELECT technical_name || ' ' || version_odoo || ' ' || name || ' ' || version_module || ' ' || coalesce(license,'') || ' ' || coalesce(category,'') || ' ' || installable || ' ' || folder_size || ' ' || last_commit_hash || ' ' || last_commit_author || ' ' || last_commit_name || ' ' || last_commit_date || ' ' || source_path || ' ' || analysis_version AS l FROM module ORDER BY l"),
        ("dependency", "SELECT m.technical_name || ' ' || t.name || ' ' || d.name AS l FROM dependency_module dm JOIN module m ON m.id = dm.module_id JOIN dependency d ON d.id = dm.dependency_id JOIN dependency_type t ON t.id = d.dependency_type_id ORDER BY l"),
        ("module_version", "SELECT m.technical_name || ' ' || v.version_module AS l FROM module_version v JOIN module m ON m.id = v.module_id ORDER BY l"),
        ("security", "SELECT m.technical_name || ' ' || v.version_module || ' ' || w.severity || ' ' || w.code || ' ' || coalesce(w.xml_id,'') || ' ' || coalesce(w.file,'') || ':' || coalesce(w.line,'') AS l FROM module_security_warning w JOIN module m ON m.id = w.module_id JOIN module_version v ON v.id = w.module_version_id ORDER BY l"),
        ("migration", "SELECT m.technical_name || ' ' || v.version_module || ' ' || n.code || ' ' || coalesce(n.file,'') || ':' || coalesce(n.line,'') AS l FROM module_migration_note n JOIN module m ON m.id = n.module_id JOIN module_version v ON v.id = n.module_version_id ORDER BY l"),
        ("view", "SELECT m.technical_name || ' ' || x.xml_id || ' ' || coalesce(x.view_type,'') AS l FROM module_view x JOIN module m ON m.id = x.module_id ORDER BY l"),
        ("model", "SELECT m.technical_name || ' ' || x.model_name || ' ' || x.is_new_model AS l FROM module_model x JOIN module m ON m.id = x.module_id ORDER BY l"),
        ("author", "SELECT m.technical_name || ' ' || a.name AS l FROM module_author x JOIN module m ON m.id = x.module_id JOIN author a ON a.id = x.author_id ORDER BY l"),
        ("maintainer", "SELECT m.technical_name || ' ' || a.name AS l FROM module_maintainer x JOIN module m ON m.id = x.module_id JOIN maintainer a ON a.id = x.maintainer_id ORDER BY l"),
        ("committer", "SELECT m.technical_name || ' ' || c.name || ' ' || x.commits || ' ' || x.insertions || ' ' || x.deletions AS l FROM module_committer x JOIN module m ON m.id = x.module_id JOIN committer c ON c.id = x.committer_id ORDER BY l"),
        ("event", "SELECT t.name || ' ' || e.severity || ' ' || e.message AS l FROM system_event e JOIN system_event_type t ON t.id = e.event_type_id ORDER BY e.id"),
    ];
    let mut out = String::new();
    for (name, query) in sections {
        let sql = format!("SELECT coalesce(group_concat(l, char(10)), '') AS v FROM ({query})");
        let lines = diesel::sql_query(sql).get_result::<Lines>(conn).unwrap().v;
        out += &format!("## {name}\n{lines}\n");
    }
    out
}

/// Compares with the recorded snapshot (or records it).
pub fn assert_snapshot(actual: &str) {
    if std::env::var_os("OGH_GOLDEN_RECORD").is_some() {
        fs::write(SNAPSHOT, actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(Path::new(SNAPSHOT)).unwrap_or_default();
    if actual != expected {
        let diff: Vec<String> = actual
            .lines()
            .zip(expected.lines().chain(std::iter::repeat("<missing>")))
            .filter(|(a, e)| a != e)
            .take(10)
            .map(|(a, e)| format!("  got:      {a}\n  expected: {e}"))
            .collect();
        panic!(
            "snapshot differs (OGH_GOLDEN_RECORD=1 to re-record):\n{}",
            diff.join("\n")
        );
    }
}

/// The built-in `default` workflow over the fixture: three runs (all new,
/// nothing changed, a commit).
#[tokio::test]
async fn the_default_workflow_matches_the_snapshot() {
    use crate::workflow;
    let f = Fixture::new("default");
    let pool = f.db();
    let mut conn = pool.get().unwrap();
    let analyzer = crate::analyzer::OGHCollectorAnalyzer::new(&180);
    let pypi = crate::pypi::PypiClient::new();
    let forge = crate::actions::Forge::with(false, "http://127.0.0.1:9", None);
    let workdir = f.dir.join("work");
    let deps = workflow::Deps::for_tests(&analyzer, &pypi, &forge, &workdir, &[]);
    let defs = workflow::with_built_in(&Default::default());
    let engine = workflow::Engine::new(&[workflow::DEFAULT.to_string()], &defs).unwrap();
    let (read_paths, repos) = (vec![String::new()], f.repo_infos());
    let org = sqlitedb::models::gh_organization::add(&mut conn, "org").unwrap();
    let repo_id = sqlitedb::models::gh_repository::add(&mut conn, &org.id, "repo")
        .unwrap()
        .id;
    first_commit(&f);
    for _ in 0..2 {
        // The second run changes nothing.
        workflow::run_modules(
            &mut conn,
            &engine,
            &deps,
            &read_paths,
            &repos,
            &[repo_id],
            &180,
        )
        .await;
    }
    second_commit(&f);
    workflow::run_modules(
        &mut conn,
        &engine,
        &deps,
        &read_paths,
        &repos,
        &[repo_id],
        &180,
    )
    .await;
    assert_snapshot(&snapshot(&mut conn));
}
