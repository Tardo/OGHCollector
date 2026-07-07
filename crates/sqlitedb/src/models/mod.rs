// Copyright Alexandre D. Díaz
pub mod author;
pub mod committer;
pub mod dependency;
pub mod dependency_module;
pub mod dependency_osv;
pub mod dependency_type;
pub mod gh_organization;
pub mod gh_repository;
pub mod maintainer;
pub mod module;
pub mod module_author;
pub mod module_code_analysis;
pub mod module_committer;
pub mod module_committer_period;
pub mod module_controller;
pub mod module_maintainer;
pub mod module_model;
pub mod module_model_field;
pub mod module_model_method;
pub mod module_record;
pub mod module_security_warning;
pub mod module_version;
pub mod module_view;
pub mod pull_request;
pub mod system_event;
pub mod system_event_type;

use diesel::r2d2::{ConnectionManager, PooledConnection};
use diesel::sqlite::SqliteConnection;

pub type Connection = PooledConnection<ConnectionManager<SqliteConnection>>;

// Shared helper types for sql_query results.
#[derive(diesel::QueryableByName)]
pub struct NameRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub name: String,
}

#[derive(diesel::QueryableByName)]
struct LastIdRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    id: i64,
}

/// Returns the rowid of the most recently inserted row on this connection.
pub(crate) fn last_insert_rowid(conn: &mut diesel::sqlite::SqliteConnection) -> i64 {
    use diesel::RunQueryDsl;
    diesel::sql_query("SELECT last_insert_rowid() as id")
        .get_result::<LastIdRow>(conn)
        .expect("Failed to get last_insert_rowid")
        .id
}

#[cfg(test)]
mod tests {
    use diesel::sqlite::SqliteConnection;
    use diesel::Connection;
    use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};

    const MIGRATIONS: EmbeddedMigrations = embed_migrations!("../../migrations");

    fn setup_db() -> SqliteConnection {
        let mut conn = SqliteConnection::establish(":memory:").unwrap();
        conn.run_pending_migrations(MIGRATIONS).unwrap();
        conn
    }

    // Simulates a pre-existing database created by the old rusqlite-based
    // prepare_schema() + populate_basics(), which had no __diesel_migrations table.
    // Diesel migrations must apply cleanly on top of it.
    #[test]
    fn test_migration_on_existing_db() {
        use diesel::connection::SimpleConnection;
        let mut conn = SqliteConnection::establish(":memory:").unwrap();

        // Reproduce exactly what the old prepare_schema() created (all CREATE TABLE IF NOT
        // EXISTS statements, plus the old seed data from populate_basics).
        conn.batch_execute(
            "
            CREATE TABLE IF NOT EXISTS gh_organization (id integer primary key autoincrement, name text unique not null);
            CREATE TABLE IF NOT EXISTS gh_repository (
                id integer primary key autoincrement, name text unique not null,
                gh_organization_id integer not null references gh_organization(id),
                create_date text not null, update_date text not null);
            CREATE UNIQUE INDEX IF NOT EXISTS uniq_name_gh_organization_id ON gh_repository(name, gh_organization_id);
            CREATE TABLE IF NOT EXISTS module (
                id integer primary key autoincrement, technical_name text not null,
                version_odoo integer not null, name text not null, version_module text not null,
                description text, website text, license text default 'LGPL-3',
                category text default 'Uncategorized', auto_install boolean not null default false,
                application boolean not null default false, installable boolean not null default true,
                gh_repository_id integer not null references gh_repository(id),
                create_date text not null, update_date text not null,
                folder_size integer not null, last_commit_hash text not null,
                last_commit_author text not null, last_commit_name text not null,
                last_commit_date text not null, last_commit_partof text);
            CREATE UNIQUE INDEX IF NOT EXISTS uniq_tech_name_ver_odoo_gh_repository_id ON module(technical_name, version_odoo, gh_repository_id);
            CREATE TABLE IF NOT EXISTS dependency_type (id integer primary key autoincrement, name text not null unique);
            INSERT OR IGNORE INTO dependency_type(name) VALUES ('module'), ('python'), ('bin');
            CREATE TABLE IF NOT EXISTS dependency (
                id integer primary key autoincrement,
                dependency_type_id integer not null references dependency_type(id),
                name text not null);
            CREATE UNIQUE INDEX IF NOT EXISTS uniq_dep_type_name ON dependency(dependency_type_id, name);
            CREATE TABLE IF NOT EXISTS dependency_module (
                id integer primary key autoincrement,
                dependency_id integer not null references dependency(id),
                module_id integer not null references module(id));
            CREATE UNIQUE INDEX IF NOT EXISTS uniq_dep_module ON dependency_module(dependency_id, module_id);
            CREATE TABLE IF NOT EXISTS dependency_osv (
                id integer primary key autoincrement,
                dependency_module_id integer not null references dependency_module(id),
                osv_id text not null, details text not null, fixed_in text not null);
            CREATE UNIQUE INDEX IF NOT EXISTS uniq_dep_dep_mod_id_osv_id ON dependency_osv(dependency_module_id, osv_id);
            CREATE TABLE IF NOT EXISTS author (id integer primary key autoincrement, name text not null unique);
            CREATE TABLE IF NOT EXISTS maintainer (id integer primary key autoincrement, name text not null unique);
            CREATE TABLE IF NOT EXISTS committer (id integer primary key autoincrement, name text not null unique);
            CREATE TABLE IF NOT EXISTS module_author (
                id integer primary key autoincrement,
                module_id integer not null references module(id),
                author_id integer not null references author(id));
            CREATE UNIQUE INDEX IF NOT EXISTS uniq_module_author ON module_author(module_id, author_id);
            CREATE TABLE IF NOT EXISTS module_maintainer (
                id integer primary key autoincrement,
                module_id integer not null references module(id),
                maintainer_id integer not null references maintainer(id));
            CREATE UNIQUE INDEX IF NOT EXISTS uniq_module_maintainer ON module_maintainer(module_id, maintainer_id);
            CREATE TABLE IF NOT EXISTS module_committer (
                id integer primary key autoincrement,
                module_id integer not null references module(id),
                committer_id integer not null references committer(id),
                commits integer not null);
            CREATE UNIQUE INDEX IF NOT EXISTS uniq_module_committer ON module_committer(module_id, committer_id);
            CREATE TABLE IF NOT EXISTS system_event_type (id integer primary key autoincrement, name text not null unique);
            INSERT OR IGNORE INTO system_event_type(name) VALUES
                ('issue'), ('internal'), ('module'), ('maintainer'),
                ('committer'), ('dependency'), ('author'), ('repository'), ('organization');
            CREATE TABLE IF NOT EXISTS system_event (
                id integer primary key autoincrement, message text not null,
                date text not null, event_type_id integer not null references system_event_type(id));
            ",
        )
        .expect("Failed to create old schema");

        // Now run Diesel migrations on top — must not fail.
        conn.run_pending_migrations(MIGRATIONS)
            .expect("Migrations failed on existing DB");

        // Verify the DB is functional after migration.
        let org = super::gh_organization::add(&mut conn, "TestOrg").unwrap();
        assert_eq!(org.name, "TestOrg");
    }

    #[test]
    fn test_author_add_and_get() {
        let mut conn = setup_db();
        let author = super::author::add(&mut conn, "Alice").unwrap();
        assert_eq!(author.name, "Alice");
        assert!(author.id > 0);

        let found = super::author::get_by_id(&mut conn, &author.id);
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "Alice");
    }

    #[test]
    fn test_author_add_idempotent() {
        let mut conn = setup_db();
        let a1 = super::author::add(&mut conn, "Bob").unwrap();
        let a2 = super::author::add(&mut conn, "Bob").unwrap();
        assert_eq!(a1.id, a2.id);
    }

    #[test]
    fn test_committer_add_and_get() {
        let mut conn = setup_db();
        let com = super::committer::add(&mut conn, "Charlie").unwrap();
        assert_eq!(com.name, "Charlie");
        let found = super::committer::get_by_name(&mut conn, "Charlie");
        assert!(found.is_some());
    }

    #[test]
    fn test_dependency_type_seeded() {
        let mut conn = setup_db();
        let module_type = super::dependency_type::get_by_name(&mut conn, "module");
        assert!(module_type.is_some());
        let python_type = super::dependency_type::get_by_name(&mut conn, "python");
        assert!(python_type.is_some());
        let bin_type = super::dependency_type::get_by_name(&mut conn, "bin");
        assert!(bin_type.is_some());
    }

    #[test]
    fn test_gh_organization_add_idempotent() {
        let mut conn = setup_db();
        let o1 = super::gh_organization::add(&mut conn, "OCA").unwrap();
        let o2 = super::gh_organization::add(&mut conn, "OCA").unwrap();
        assert_eq!(o1.id, o2.id);
        assert_eq!(o1.name, "OCA");
    }

    #[test]
    fn test_gh_repository_add() {
        let mut conn = setup_db();
        let org = super::gh_organization::add(&mut conn, "TestOrg").unwrap();
        let repo = super::gh_repository::add(&mut conn, &org.id, "my-repo").unwrap();
        assert_eq!(repo.name, "my-repo");
        assert_eq!(repo.gh_organization_id, org.id);

        let found = super::gh_repository::get_by_name(&mut conn, &org.id, "my-repo");
        assert!(found.is_some());
    }

    #[test]
    fn test_module_add_and_get() {
        let mut conn = setup_db();
        use std::collections::HashMap;

        let mut info = super::module::ManifestInfo {
            technical_name: "test_module".to_string(),
            version_odoo: 16,
            name: "Test Module".to_string(),
            version_module: "16.0.1.0.0".to_string(),
            description: "A test module".to_string(),
            installation: "pip install foo".to_string(),
            usage: "Go to Settings > Foo".to_string(),
            author: "Alice".to_string(),
            website: "https://example.com".to_string(),
            license: "LGPL-3".to_string(),
            category: "Technical".to_string(),
            auto_install: false,
            application: false,
            installable: true,
            maintainer: "Alice".to_string(),
            git_org: "TestOrg".to_string(),
            git_repo: "test-repo".to_string(),
            depends: vec!["base".to_string()],
            external_depends_python: vec![],
            external_depends_bin: vec![],
            folder_size: 1024,
            last_commit_hash: "abc123".to_string(),
            last_commit_author: "Alice".to_string(),
            last_commit_date: "2024-01-01".to_string(),
            last_commit_name: "Initial commit".to_string(),
            last_commit_partof: "".to_string(),
            committers: HashMap::new(),
            analysis: Default::default(),
        };

        let module = super::module::add(&mut conn, &info).unwrap();
        assert_eq!(module.technical_name, "test_module");
        assert_eq!(module.version_odoo, 16);
        assert!(module.id > 0);

        let found = super::module::get_by_id(&mut conn, &module.id).unwrap();
        assert_eq!(found.name, "Test Module");
        assert_eq!(found.installation_str(), "pip install foo");
        assert_eq!(found.usage_str(), "Go to Settings > Foo");

        // Re-adding with changed install/usage text must update the existing
        // row in place (the UPDATE branch of module::add), not just the
        // INSERT branch exercised above.
        info.installation = "pip install foo --upgrade".to_string();
        info.usage = "Go to Settings > Foo > Bar".to_string();
        let updated = super::module::add(&mut conn, &info).unwrap();
        assert_eq!(updated.id, module.id);
        assert_eq!(updated.installation_str(), "pip install foo --upgrade");
        assert_eq!(updated.usage_str(), "Go to Settings > Foo > Bar");
    }

    #[test]
    fn test_module_add_idempotent() {
        let mut conn = setup_db();
        use std::collections::HashMap;

        let info = super::module::ManifestInfo {
            technical_name: "dup_module".to_string(),
            version_odoo: 17,
            name: "Dup Module".to_string(),
            version_module: "17.0.1.0.0".to_string(),
            description: String::new(),
            installation: String::new(),
            usage: String::new(),
            author: String::new(),
            website: String::new(),
            license: String::new(),
            category: String::new(),
            auto_install: false,
            application: false,
            installable: true,
            maintainer: String::new(),
            git_org: "Org2".to_string(),
            git_repo: "repo2".to_string(),
            depends: vec![],
            external_depends_python: vec![],
            external_depends_bin: vec![],
            folder_size: 512,
            last_commit_hash: "def456".to_string(),
            last_commit_author: "Bob".to_string(),
            last_commit_date: "2024-02-01".to_string(),
            last_commit_name: "Add module".to_string(),
            last_commit_partof: String::new(),
            committers: HashMap::new(),
            analysis: Default::default(),
        };

        let m1 = super::module::add(&mut conn, &info).unwrap();
        let m2 = super::module::add(&mut conn, &info).unwrap();
        assert_eq!(m1.id, m2.id);
    }

    #[test]
    fn test_dependency_module_add() {
        let mut conn = setup_db();
        use std::collections::HashMap;

        let module_info = super::module::ManifestInfo {
            technical_name: "dep_test".to_string(),
            version_odoo: 16,
            name: "Dep Test".to_string(),
            version_module: "16.0.1.0.0".to_string(),
            description: String::new(),
            installation: String::new(),
            usage: String::new(),
            author: String::new(),
            website: String::new(),
            license: String::new(),
            category: String::new(),
            auto_install: false,
            application: false,
            installable: true,
            maintainer: String::new(),
            git_org: "DepOrg".to_string(),
            git_repo: "dep-repo".to_string(),
            depends: vec![],
            external_depends_python: vec![],
            external_depends_bin: vec![],
            folder_size: 256,
            last_commit_hash: "ghi789".to_string(),
            last_commit_author: "Carol".to_string(),
            last_commit_date: "2024-03-01".to_string(),
            last_commit_name: "Add dep".to_string(),
            last_commit_partof: String::new(),
            committers: HashMap::new(),
            analysis: Default::default(),
        };

        let module = super::module::add(&mut conn, &module_info).unwrap();
        let dep_type = super::dependency_type::get_by_name(&mut conn, "python").unwrap();
        let dep_mod =
            super::dependency_module::add(&mut conn, &dep_type.id, "requests", &module.id).unwrap();
        assert_eq!(dep_mod.dependency_name, "requests");
        assert_eq!(dep_mod.module_id, module.id);

        let names = super::dependency_module::get_names(&mut conn, &module.id, &dep_type.id);
        assert!(names.contains(&"requests".to_string()));
    }

    #[test]
    fn test_system_event_add() {
        let mut conn = setup_db();
        let event = super::system_event::add(&mut conn, "internal", "info", "Test event").unwrap();
        assert_eq!(event.message, "Test event");
        assert_eq!(event.event_type_name, "internal");
        assert_eq!(event.severity, "info");
        assert!(!event.is_html);
    }

    #[test]
    fn test_system_event_get_messages_page() {
        let mut conn = setup_db();
        for i in 0..5 {
            super::system_event::add(&mut conn, "internal", "info", &format!("event {i}")).unwrap();
        }

        // Cursor pagination: first page newest-first, second page picks up where it left off.
        let page1 = super::system_event::get_messages_page(&mut conn, i64::MAX, None, None, 2);
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].message, "event 4");
        assert_eq!(page1[1].message, "event 3");

        let page2 = super::system_event::get_messages_page(&mut conn, page1[1].id, None, None, 2);
        assert_eq!(page2.len(), 2);
        assert_eq!(page2[0].message, "event 2");
        assert_eq!(page2[1].message, "event 1");

        // Date filter: today's range includes everything, a past range excludes it all.
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let in_range = super::system_event::get_messages_page(
            &mut conn,
            i64::MAX,
            Some(today.as_str()),
            Some(today.as_str()),
            10,
        );
        assert_eq!(in_range.len(), 5);

        let out_of_range = super::system_event::get_messages_page(
            &mut conn,
            i64::MAX,
            Some("2000-01-01"),
            Some("2000-01-01"),
            10,
        );
        assert!(out_of_range.is_empty());

        // One-sided ranges: only a lower or only an upper bound.
        let from_only = super::system_event::get_messages_page(
            &mut conn,
            i64::MAX,
            Some(today.as_str()),
            None,
            10,
        );
        assert_eq!(from_only.len(), 5);

        let to_only_past = super::system_event::get_messages_page(
            &mut conn,
            i64::MAX,
            None,
            Some("2000-01-01"),
            10,
        );
        assert!(to_only_past.is_empty());
    }

    // Event types used to be a closed, pre-seeded set: logging an unseeded type
    // panicked (see system_event_type::get_by_name/expect before the fix). Now
    // a brand-new type name is created on first use, so introducing a new kind
    // of logged action never requires a seed migration.
    #[test]
    fn test_system_event_add_creates_new_type() {
        let mut conn = setup_db();
        let event = super::system_event::add(&mut conn, "brand_new_type", "warning", "Hi").unwrap();
        assert_eq!(event.event_type_name, "brand_new_type");
        assert_eq!(event.severity, "warning");

        let event_type = super::system_event_type::get_by_name(&mut conn, "brand_new_type");
        assert!(event_type.is_some());
    }

    #[test]
    fn test_module_get_odoo_versions() {
        let mut conn = setup_db();
        use std::collections::HashMap;

        let make_info = |name: &str, ver: u8| super::module::ManifestInfo {
            technical_name: name.to_string(),
            version_odoo: ver,
            name: name.to_string(),
            version_module: format!("{ver}.0.1.0.0"),
            description: String::new(),
            installation: String::new(),
            usage: String::new(),
            author: String::new(),
            website: String::new(),
            license: String::new(),
            category: String::new(),
            auto_install: false,
            application: false,
            installable: true,
            maintainer: String::new(),
            git_org: "VerOrg".to_string(),
            git_repo: "ver-repo".to_string(),
            depends: vec![],
            external_depends_python: vec![],
            external_depends_bin: vec![],
            folder_size: 64,
            last_commit_hash: "yyy".to_string(),
            last_commit_author: "Dev".to_string(),
            last_commit_date: "2024-05-01".to_string(),
            last_commit_name: "commit".to_string(),
            last_commit_partof: String::new(),
            committers: HashMap::new(),
            analysis: Default::default(),
        };

        super::module::add(&mut conn, &make_info("mod_v15", 15)).unwrap();
        super::module::add(&mut conn, &make_info("mod_v17", 17)).unwrap();
        super::module::add(&mut conn, &make_info("mod_v17_b", 17)).unwrap();

        let versions = super::module::get_odoo_versions(&mut conn);
        assert_eq!(versions, vec![17, 15]);
    }

    #[test]
    fn test_module_get_module_repository() {
        let mut conn = setup_db();
        use std::collections::HashMap;

        let make_info = |name: &str| super::module::ManifestInfo {
            technical_name: name.to_string(),
            version_odoo: 16,
            name: name.to_string(),
            version_module: "16.0.1.0.0".to_string(),
            description: String::new(),
            installation: String::new(),
            usage: String::new(),
            author: String::new(),
            website: String::new(),
            license: String::new(),
            category: String::new(),
            auto_install: false,
            application: false,
            installable: true,
            maintainer: String::new(),
            git_org: "RepoOrg".to_string(),
            git_repo: "repo-a".to_string(),
            depends: vec![],
            external_depends_python: vec![],
            external_depends_bin: vec![],
            folder_size: 64,
            last_commit_hash: "zzz".to_string(),
            last_commit_author: "Dev".to_string(),
            last_commit_date: "2024-06-01".to_string(),
            last_commit_name: "commit".to_string(),
            last_commit_partof: String::new(),
            committers: HashMap::new(),
            analysis: Default::default(),
        };

        super::module::add(&mut conn, &make_info("mod_x")).unwrap();
        super::module::add(&mut conn, &make_info("mod_y")).unwrap();
        // Name with a quote must not break the query.
        super::module::add(&mut conn, &make_info("mod'z")).unwrap();

        let names = vec![
            "mod_x".to_string(),
            "mod'z".to_string(),
            "missing".to_string(),
        ];
        let mut infos = super::module::get_module_repository(&mut conn, &16u8, &names);
        infos.sort_by(|a, b| a.technical_name.cmp(&b.technical_name));
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].technical_name, "mod'z");
        assert_eq!(infos[0].repository_name, "repo-a");
        assert_eq!(infos[1].technical_name, "mod_x");

        assert!(super::module::get_module_repository(&mut conn, &16u8, &[]).is_empty());
    }

    #[test]
    fn test_module_delete_outdated() {
        let mut conn = setup_db();
        use std::collections::HashMap;

        let make_info = |name: &str| super::module::ManifestInfo {
            technical_name: name.to_string(),
            version_odoo: 16,
            name: name.to_string(),
            version_module: "16.0.1.0.0".to_string(),
            description: String::new(),
            installation: String::new(),
            usage: String::new(),
            author: String::new(),
            website: String::new(),
            license: String::new(),
            category: String::new(),
            auto_install: false,
            application: false,
            installable: true,
            maintainer: String::new(),
            git_org: "OutdOrg".to_string(),
            git_repo: "outd-repo".to_string(),
            depends: vec![],
            external_depends_python: vec![],
            external_depends_bin: vec![],
            folder_size: 128,
            last_commit_hash: "xxx".to_string(),
            last_commit_author: "Dev".to_string(),
            last_commit_date: "2024-04-01".to_string(),
            last_commit_name: "commit".to_string(),
            last_commit_partof: String::new(),
            committers: HashMap::new(),
            analysis: Default::default(),
        };

        let m1 = super::module::add(&mut conn, &make_info("mod_a")).unwrap();
        let m2 = super::module::add(&mut conn, &make_info("mod_b")).unwrap();
        let m3 = super::module::add(&mut conn, &make_info("mod_c")).unwrap();

        // Keep only m1 and m2, delete m3
        let kept = vec![m1.id, m2.id];
        super::module::delete_outdated(&mut conn, &m1.gh_repository_id, &16u8, &kept).unwrap();

        assert!(super::module::get_by_id(&mut conn, &m1.id).is_some());
        assert!(super::module::get_by_id(&mut conn, &m2.id).is_some());
        assert!(super::module::get_by_id(&mut conn, &m3.id).is_none());
    }

    #[test]
    fn test_pull_request_add_and_upsert() {
        let mut conn = setup_db();
        let org = super::gh_organization::add(&mut conn, "PrOrg").unwrap();
        let repo = super::gh_repository::add(&mut conn, &org.id, "pr-repo").unwrap();

        let pr1 = super::pull_request::add(
            &mut conn,
            "[16.0][MIG] sale_commission",
            "sale_commission",
            &42,
            &16u8,
            &repo.id,
        )
        .unwrap();
        assert_eq!(pr1.prid, 42);
        assert_eq!(pr1.module_technical_name, "sale_commission");

        // Same (repo, prid) must update in place, not duplicate.
        let pr1_updated = super::pull_request::add(
            &mut conn,
            "[16.0][MIG] sale_commission (renamed)",
            "sale_commission",
            &42,
            &16u8,
            &repo.id,
        )
        .unwrap();
        assert_eq!(pr1_updated.id, pr1.id);
        assert_eq!(pr1_updated.name, "[16.0][MIG] sale_commission (renamed)");

        // A PR number in a different repo must not collide with repo A's #42.
        let other_repo = super::gh_repository::add(&mut conn, &org.id, "other-repo").unwrap();
        let pr_other_repo = super::pull_request::add(
            &mut conn,
            "[16.0][MIG] other_module",
            "other_module",
            &42,
            &16u8,
            &other_repo.id,
        )
        .unwrap();
        assert_ne!(pr_other_repo.id, pr1.id);

        let found = super::pull_request::get_by_technical_name_organization_name(
            &mut conn,
            "sale_commission",
            "PrOrg",
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, pr1.id);
    }

    #[test]
    fn test_pull_request_delete_outdated() {
        let mut conn = setup_db();
        let org = super::gh_organization::add(&mut conn, "PrOrg2").unwrap();
        let repo = super::gh_repository::add(&mut conn, &org.id, "pr-repo-2").unwrap();

        let pr1 =
            super::pull_request::add(&mut conn, "mig 1", "mod_1", &1, &16u8, &repo.id).unwrap();
        let _pr2 =
            super::pull_request::add(&mut conn, "mig 2", "mod_2", &2, &16u8, &repo.id).unwrap();

        // Keep only #1, #2 must be removed since it's not in the "still open" list.
        super::pull_request::delete_outdated(&mut conn, &repo.id, &16u8, &[1]).unwrap();
        assert!(super::pull_request::get_by_id(&mut conn, &pr1.id).is_some());
        assert!(super::pull_request::get_by_id(&mut conn, &_pr2.id).is_none());

        // Unlike module::delete_outdated, an empty list must clear everything left
        // (all migration PRs for this repo/version got merged or closed).
        super::pull_request::delete_outdated(&mut conn, &repo.id, &16u8, &[]).unwrap();
        assert!(super::pull_request::get_by_id(&mut conn, &pr1.id).is_none());
    }

    #[test]
    fn test_module_committer_period_written_and_ranked() {
        let mut conn = setup_db();
        use super::module::CommitterActivity;
        use std::collections::HashMap;

        let make_info = |periods: HashMap<(i32, i32), u32>, total: u32| {
            let mut committers = HashMap::new();
            committers.insert(
                "Dave".to_string(),
                CommitterActivity {
                    total,
                    periods,
                    ..Default::default()
                },
            );
            super::module::ManifestInfo {
                technical_name: "period_test".to_string(),
                version_odoo: 16,
                name: "Period Test".to_string(),
                version_module: "16.0.1.0.0".to_string(),
                description: String::new(),
                installation: String::new(),
                usage: String::new(),
                author: String::new(),
                website: String::new(),
                license: String::new(),
                category: String::new(),
                auto_install: false,
                application: false,
                installable: true,
                maintainer: String::new(),
                git_org: "PeriodOrg".to_string(),
                git_repo: "period-repo".to_string(),
                depends: vec![],
                external_depends_python: vec![],
                external_depends_bin: vec![],
                folder_size: 128,
                last_commit_hash: "period1".to_string(),
                last_commit_author: "Dave".to_string(),
                last_commit_date: "2024-02-15".to_string(),
                last_commit_name: "commit".to_string(),
                last_commit_partof: String::new(),
                committers,
                analysis: Default::default(),
            }
        };

        let mut periods = HashMap::new();
        periods.insert((2024, 1), 2);
        periods.insert((2024, 2), 3);
        super::module::add(&mut conn, &make_info(periods, 5)).unwrap();

        let jan = super::module_committer_period::rank_by_period(&mut conn, 2024, Some(1), 10);
        assert_eq!(jan.len(), 1);
        assert_eq!(jan[0].name, "Dave");
        assert_eq!(jan[0].total_commits, 2);

        let year = super::module_committer_period::rank_by_period(&mut conn, 2024, None, 10);
        assert_eq!(year.len(), 1);
        assert_eq!(year[0].total_commits, 5);

        assert!(
            super::module_committer_period::rank_by_period(&mut conn, 2099, None, 10).is_empty()
        );

        // Re-adding the same module with a different breakdown must replace, not
        // accumulate (the collector recomputes the full range on every run).
        let mut new_periods = HashMap::new();
        new_periods.insert((2024, 3), 7);
        super::module::add(&mut conn, &make_info(new_periods, 7)).unwrap();

        let march = super::module_committer_period::rank_by_period(&mut conn, 2024, Some(3), 10);
        assert_eq!(march[0].total_commits, 7);
        assert!(
            super::module_committer_period::rank_by_period(&mut conn, 2024, Some(1), 10).is_empty()
        );
    }

    fn make_bare_module_info(name: &str) -> super::module::ManifestInfo {
        super::module::ManifestInfo {
            technical_name: name.to_string(),
            version_odoo: 16,
            name: name.to_string(),
            version_module: "16.0.1.0.0".to_string(),
            description: String::new(),
            installation: String::new(),
            usage: String::new(),
            author: String::new(),
            website: String::new(),
            license: String::new(),
            category: String::new(),
            auto_install: false,
            application: false,
            installable: true,
            maintainer: String::new(),
            git_org: "AnalysisOrg".to_string(),
            git_repo: "analysis-repo".to_string(),
            depends: vec![],
            external_depends_python: vec![],
            external_depends_bin: vec![],
            folder_size: 1,
            last_commit_hash: "aaa".to_string(),
            last_commit_author: "Dev".to_string(),
            last_commit_date: "2024-07-01".to_string(),
            last_commit_name: "commit".to_string(),
            last_commit_partof: String::new(),
            committers: std::collections::HashMap::new(),
            analysis: Default::default(),
        }
    }

    #[test]
    fn test_module_view_replace_for_module() {
        use super::module_code_analysis::ViewAnalysisInfo;
        let mut conn = setup_db();
        let module = super::module::add(&mut conn, &make_bare_module_info("view_test")).unwrap();
        let module_version =
            super::module_version::get_or_create(&mut conn, &module.id, &module.version_module)
                .unwrap();

        let views = vec![
            ViewAnalysisInfo {
                xml_id: "view_a".to_string(),
                name: Some("A".to_string()),
                model: Some("res.partner".to_string()),
                inherit_xml_id: None,
                view_type: Some("form".to_string()),
            },
            ViewAnalysisInfo {
                xml_id: "view_b".to_string(),
                name: Some("B".to_string()),
                model: Some("res.partner".to_string()),
                inherit_xml_id: Some("base.view_partner_form".to_string()),
                view_type: Some("form".to_string()),
            },
        ];
        super::module_view::replace_for_module(&mut conn, &module.id, &module_version.id, &views)
            .unwrap();
        assert_eq!(
            super::module_view::get_by_module_version_id(&mut conn, &module_version.id).len(),
            2
        );

        // Re-analyzing with a smaller set must replace, not accumulate.
        let views2 = vec![views[0].clone()];
        super::module_view::replace_for_module(&mut conn, &module.id, &module_version.id, &views2)
            .unwrap();
        let found = super::module_view::get_by_module_version_id(&mut conn, &module_version.id);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].xml_id, "view_a");
        assert_eq!(found[0].view_type.as_deref(), Some("form"));
    }

    #[test]
    fn test_module_record_replace_for_module() {
        use super::module_code_analysis::RecordAnalysisInfo;
        let mut conn = setup_db();
        let module = super::module::add(&mut conn, &make_bare_module_info("record_test")).unwrap();
        let module_version =
            super::module_version::get_or_create(&mut conn, &module.id, &module.version_module)
                .unwrap();

        let records = vec![
            RecordAnalysisInfo {
                xml_id: "group_a".to_string(),
                model: "res.groups".to_string(),
                noupdate: true,
                fields: Some(serde_json::json!({"name": "Group A"})),
            },
            RecordAnalysisInfo {
                xml_id: "access_a".to_string(),
                model: "ir.model.access".to_string(),
                noupdate: false,
                fields: Some(serde_json::json!({"perm_read": "1"})),
            },
        ];
        super::module_record::replace_for_module(
            &mut conn,
            &module.id,
            &module_version.id,
            &records,
        )
        .unwrap();
        assert_eq!(
            super::module_record::get_by_module_version_id(&mut conn, &module_version.id).len(),
            2
        );

        // Re-analyzing with a smaller set must replace, not accumulate.
        let records2 = vec![records[0].clone()];
        super::module_record::replace_for_module(
            &mut conn,
            &module.id,
            &module_version.id,
            &records2,
        )
        .unwrap();
        let found = super::module_record::get_by_module_version_id(&mut conn, &module_version.id);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].xml_id, "group_a");
        assert!(found[0].noupdate);
        assert_eq!(found[0].fields_value().unwrap()["name"], "Group A");
    }

    #[test]
    fn test_module_model_replace_for_module_no_orphans() {
        use super::module_code_analysis::{
            FieldAnalysisInfo, MethodAnalysisInfo, ModelAnalysisInfo,
        };
        let mut conn = setup_db();
        let module = super::module::add(&mut conn, &make_bare_module_info("model_test")).unwrap();
        let module_version =
            super::module_version::get_or_create(&mut conn, &module.id, &module.version_module)
                .unwrap();

        let models_v1 = vec![ModelAnalysisInfo {
            model_name: "res.partner".to_string(),
            class_name: "ResPartner".to_string(),
            inherit_from: vec!["res.partner".to_string()],
            is_new_model: false,
            docstring: Some("Extends res.partner with x_foo.".to_string()),
            attrs: Some(serde_json::json!({"kind": "Model"})),
            fields: vec![FieldAnalysisInfo {
                name: "x_foo".to_string(),
                field_type: "Char".to_string(),
                relation: None,
                attrs: Some(serde_json::json!({"required": "True", "help": "'A help text'"})),
            }],
            methods: vec![MethodAnalysisInfo {
                name: "do_thing".to_string(),
                // Deliberately a decorator with a comma-containing argument
                // list: decorators are stored as JSON, not comma-joined text,
                // specifically so this doesn't get shredded on the round trip.
                decorators: vec!["api.depends('x_foo', 'x_bar')".to_string()],
                signature: "(self, vals, force=False)".to_string(),
                docstring: Some("Does the thing.".to_string()),
            }],
        }];
        super::module_model::replace_for_module(
            &mut conn,
            &module.id,
            &module_version.id,
            &models_v1,
        )
        .unwrap();
        let stored = super::module_model::get_by_module_version_id(&mut conn, &module_version.id);
        assert_eq!(stored.len(), 1);
        assert_eq!(
            stored[0].docstring.as_deref(),
            Some("Extends res.partner with x_foo.")
        );
        assert_eq!(
            stored[0].attrs_value(),
            Some(serde_json::json!({"kind": "Model"}))
        );
        let old_model_id = stored[0].id;
        let fields = super::module_model_field::get_by_module_model_id(&mut conn, &old_model_id);
        assert_eq!(fields.len(), 1);
        assert_eq!(
            fields[0].attrs_value(),
            Some(serde_json::json!({"required": "True", "help": "'A help text'"}))
        );
        let methods = super::module_model_method::get_by_module_model_id(&mut conn, &old_model_id);
        assert_eq!(methods.len(), 1);
        assert_eq!(
            methods[0].decorators_vec(),
            vec!["api.depends('x_foo', 'x_bar')".to_string()]
        );
        assert_eq!(methods[0].signature, "(self, vals, force=False)");
        assert_eq!(methods[0].docstring.as_deref(), Some("Does the thing."));

        // Re-analyzing must drop the old fields/methods rather than leaving
        // them orphaned under a module_model id that no longer exists.
        let models_v2 = vec![ModelAnalysisInfo {
            model_name: "res.partner".to_string(),
            class_name: "ResPartner".to_string(),
            inherit_from: vec!["res.partner".to_string()],
            is_new_model: false,
            docstring: None,
            attrs: None,
            fields: vec![],
            methods: vec![],
        }];
        super::module_model::replace_for_module(
            &mut conn,
            &module.id,
            &module_version.id,
            &models_v2,
        )
        .unwrap();
        let stored2 = super::module_model::get_by_module_version_id(&mut conn, &module_version.id);
        assert_eq!(stored2.len(), 1);
        let new_model_id = stored2[0].id;
        assert!(new_model_id != old_model_id);
        assert!(
            super::module_model_field::get_by_module_model_id(&mut conn, &new_model_id).is_empty()
        );
        assert!(
            super::module_model_method::get_by_module_model_id(&mut conn, &new_model_id).is_empty()
        );
        // The old (now-dead) module_model id must not still have children hanging off it.
        assert!(
            super::module_model_field::get_by_module_model_id(&mut conn, &old_model_id).is_empty()
        );
        assert!(
            super::module_model_method::get_by_module_model_id(&mut conn, &old_model_id).is_empty()
        );
    }

    // The correctness gate for the whole module_version feature: re-analyzing
    // after a manifest version bump must start a *new* module_version row and
    // leave the previous version's module_model/field/method rows untouched.
    // Filtering replace_for_module's delete by module_id instead of
    // module_version_id would wipe the old snapshot here while still passing
    // test_module_model_replace_for_module_no_orphans above.
    #[test]
    fn test_module_version_history_preserves_old_snapshot_on_bump() {
        use super::module_code_analysis::{FieldAnalysisInfo, ModelAnalysisInfo};
        let mut conn = setup_db();

        let mut info_v1 = make_bare_module_info("versioned_module");
        info_v1.version_module = "1.0.0".to_string();
        let module_v1 = super::module::add(&mut conn, &info_v1).unwrap();
        let module_version_v1 =
            super::module_version::get_or_create(&mut conn, &module_v1.id, &info_v1.version_module)
                .unwrap();

        let models_v1 = vec![ModelAnalysisInfo {
            model_name: "res.partner".to_string(),
            class_name: "ResPartner".to_string(),
            inherit_from: vec!["res.partner".to_string()],
            is_new_model: false,
            docstring: Some("v1 docstring".to_string()),
            attrs: None,
            fields: vec![FieldAnalysisInfo {
                name: "x_foo".to_string(),
                field_type: "Char".to_string(),
                relation: None,
                attrs: None,
            }],
            methods: vec![],
        }];
        super::module_model::replace_for_module(
            &mut conn,
            &module_v1.id,
            &module_version_v1.id,
            &models_v1,
        )
        .unwrap();

        // Second collector pass: the manifest version bumped to 1.0.1.
        let mut info_v2 = info_v1.clone();
        info_v2.version_module = "1.0.1".to_string();
        let module_v2 = super::module::add(&mut conn, &info_v2).unwrap();
        assert_eq!(module_v2.id, module_v1.id, "same module identity row");
        assert_eq!(module_v2.version_module, "1.0.1");

        let module_version_v2 =
            super::module_version::get_or_create(&mut conn, &module_v2.id, &info_v2.version_module)
                .unwrap();
        assert_ne!(module_version_v2.id, module_version_v1.id);

        let models_v2 = vec![ModelAnalysisInfo {
            model_name: "res.partner".to_string(),
            class_name: "ResPartner".to_string(),
            inherit_from: vec!["res.partner".to_string()],
            is_new_model: false,
            docstring: Some("v2 docstring".to_string()),
            attrs: None,
            fields: vec![],
            methods: vec![],
        }];
        super::module_model::replace_for_module(
            &mut conn,
            &module_v2.id,
            &module_version_v2.id,
            &models_v2,
        )
        .unwrap();

        // Both versions must be permanently on record.
        let history = super::module_version::get_by_module_id(&mut conn, &module_v1.id);
        assert_eq!(history.len(), 2);

        // The v1 snapshot must be untouched by the v2 re-analysis.
        let v1_models =
            super::module_model::get_by_module_version_id(&mut conn, &module_version_v1.id);
        assert_eq!(v1_models.len(), 1);
        assert_eq!(v1_models[0].docstring.as_deref(), Some("v1 docstring"));
        assert_eq!(
            super::module_model_field::get_by_module_model_id(&mut conn, &v1_models[0].id).len(),
            1
        );

        // The v2 snapshot reflects the second pass only.
        let v2_models =
            super::module_model::get_by_module_version_id(&mut conn, &module_version_v2.id);
        assert_eq!(v2_models.len(), 1);
        assert_eq!(v2_models[0].docstring.as_deref(), Some("v2 docstring"));
        assert!(
            super::module_model_field::get_by_module_model_id(&mut conn, &v2_models[0].id)
                .is_empty()
        );

        // Default resolution ("latest") must land on the v2 snapshot.
        let current = super::module_version::resolve_current(&mut conn, &module_v2).unwrap();
        assert_eq!(current.id, module_version_v2.id);
    }
}
