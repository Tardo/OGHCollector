// Copyright 2025 Alexandre D. Díaz
pub mod author;
pub mod dependency_module;
pub mod dependency_type;
pub mod dependency;
pub mod dependency_osv;
pub mod gh_organization;
pub mod gh_repository;
pub mod maintainer;
pub mod committer;
pub mod module_author;
pub mod module_maintainer;
pub mod module_committer;
pub mod module;
pub mod system_event_type;
pub mod system_event;

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub fn prepare_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    gh_organization::create_table(&conn)?;
    gh_repository::create_table(&conn)?;
    module::create_table(&conn)?;
    dependency_type::create_table(&conn)?;
    dependency::create_table(&conn)?;
    dependency_osv::create_table(&conn)?;
    dependency_module::create_table(&conn)?;
    author::create_table(&conn)?;
    maintainer::create_table(&conn)?;
    committer::create_table(&conn)?;
    module_author::create_table(&conn)?;
    module_maintainer::create_table(&conn)?;
    module_committer::create_table(&conn)?;
    system_event_type::create_table(&conn)?;
    system_event::create_table(&conn)?;
    Ok(())
}

pub fn populate_basics(conn: &Connection) -> Result<(), rusqlite::Error> {
    dependency_type::populate(&conn)?;
    system_event_type::populate(&conn)?;
    Ok(())
}