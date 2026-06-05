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
pub mod module_committer;
pub mod module_maintainer;
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
pub struct IntRow {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub value: i32,
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

        let info = super::module::ManifestInfo {
            technical_name: "test_module".to_string(),
            version_odoo: 16,
            name: "Test Module".to_string(),
            version_module: "16.0.1.0.0".to_string(),
            description: "A test module".to_string(),
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
        };

        let module = super::module::add(&mut conn, &info).unwrap();
        assert_eq!(module.technical_name, "test_module");
        assert_eq!(module.version_odoo, 16);
        assert!(module.id > 0);

        let found = super::module::get_by_id(&mut conn, &module.id);
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "Test Module");
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
        let event = super::system_event::add(&mut conn, "internal", "Test event").unwrap();
        assert_eq!(event.message, "Test event");
        assert_eq!(event.event_type_name, "internal");
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
}
