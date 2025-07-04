// Copyright 2025 Alexandre D. Díaz
use cached::proc_macro::cached;
use serde::{Deserialize, Serialize};
use rusqlite::{Result, ToSql, params};
use std::collections::HashMap;

use crate::models::{gh_organization, gh_repository, author, maintainer, module_author, module_maintainer, module_committer, system_event};
use crate::utils::date::get_sqlite_utc_now;
use oghutils::version::odoo_version_u8_to_string;

use super::system_event::LogUpdateModuleInfo;

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub static TABLE_NAME: &str = "module";


#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    pub id: i64,
    pub technical_name: String,
    pub version_odoo: u8,
    pub name: String,
    pub version_module: String,
    pub description: String,
    pub website: String,
    pub license: String,
    pub category: String,
    pub auto_install: bool,
    pub application: bool,
    pub installable: bool,
    pub gh_repository_id: (i64, String),
    pub create_date: String,
    pub update_date: String,
    pub folder_size: u64,
    pub last_commit_hash: String,
    pub last_commit_author: String,
    pub last_commit_date: String,
    pub last_commit_name: String,
    pub last_commit_partof: String,
}

#[derive(Clone)]
pub struct ManifestInfo {
    pub technical_name: String,
    pub version_odoo: u8,
    pub name: String,
    pub version_module: String,
    pub description: String,
    pub author: String,
    pub website: String,
    pub license: String,
    pub category: String,
    pub auto_install: bool,
    pub application: bool,
    pub installable: bool,
    pub maintainer: String,
    pub git_org: String,
    pub git_repo: String,
    pub depends: Vec<String>,
    pub external_depends_python: Vec<String>,
    pub external_depends_bin: Vec<String>,
    pub folder_size: u64,
    pub last_commit_hash: String,
    pub last_commit_author: String,
    pub last_commit_date: String,
    pub last_commit_name: String,
    pub last_commit_partof: String,
    pub committers: HashMap<String, u32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleInfo {
    pub technical_name: String,
    pub name: String,
    pub version_odoo: u8,
    pub organization: String,
    pub repository: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleGenericInfo {
    pub technical_name: String,
    pub versions: String,
    pub src: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleCountInfo {
    pub count: u32,
    pub version_odoo: u8,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleCountByOrganizationInfo {
    pub count: u32,
    pub version_odoo: u8,
    pub org_name: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleRankContributorInfo {
    pub count: u32,
    pub version_odoo: u8,
    pub contrib_name: String,
    pub rank: u16,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleRankCommitterInfo {
    pub count: u32,
    pub version_odoo: u8,
    pub committer_name: String,
    pub rank: u16,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleRepositoryInfo {
    pub technical_name: String,
    pub repository_name: String,
}


pub fn create_table(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!("CREATE TABLE IF NOT EXISTS {0} (
            id integer primary key,
            technical_name text not null,
            version_odoo integer not null,
            name text not null,
            version_module text not null,
            description text,
            website text,
            license text default 'LGPL-3',
            category text default 'Uncategorized',
            auto_install boolean not null default false,
            application boolean not null default false,
            installable boolean not null default true,
            gh_repository_id integer not null references {1}(id),
            create_date text not null,
            update_date text not null,
            folder_size integer not null,
            last_commit_hash text not null,
            last_commit_author text not null,
            last_commit_name text not null,
            last_commit_date text not null,
            last_commit_partof text,
            CONSTRAINT fk_gh_repository
                FOREIGN KEY (gh_repository_id)
                REFERENCES {1}(id)
                ON DELETE CASCADE
        )", &TABLE_NAME, &gh_repository::TABLE_NAME).as_str(),
        params![],
    )?;
    conn.execute(
        format!("CREATE UNIQUE INDEX IF NOT EXISTS uniq_tech_name_ver_odoo_gh_repository_id ON {}(technical_name, version_odoo, gh_repository_id)", &TABLE_NAME).as_str(),
        params![],
    )
}

fn query(conn: &Connection, extra_sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Model>, rusqlite::Error> {
    let sql = format!("SELECT mod.id, mod.technical_name, mod.version_odoo, mod.name, \
    mod.version_module, mod.description, \
    mod.website, mod.license, mod.category, \
    mod.auto_install, mod.application, mod.installable, \
    mod.gh_repository_id, gh_repo.name, mod.create_date, mod.update_date, \
    mod.folder_size, mod.last_commit_hash, mod.last_commit_author, \
    mod.last_commit_date, mod.last_commit_name, mod.last_commit_partof \
    FROM {} as mod \
    INNER JOIN {} as gh_repo \
    ON gh_repo.id = mod.gh_repository_id \
    {}", &TABLE_NAME, &gh_repository::TABLE_NAME, &extra_sql);
    let mut stmt = conn.prepare(&sql)?;
    let module_rows = stmt.query_map(
        params, 
        |row| {
            Ok(Model {
                id: row.get(0)?,
                technical_name: row.get(1)?,
                version_odoo: row.get(2)?,
                name: row.get(3)?,
                version_module: row.get(4)?,
                description: row.get(5)?,
                website: row.get(6)?,
                license: row.get(7)?,
                category: row.get(8)?,
                auto_install: row.get(9)?,
                application: row.get(10)?,
                installable: row.get(11)?,
                gh_repository_id: (row.get(12)?, row.get(13)?),
                create_date: row.get(14)?,
                update_date: row.get(15)?,
                folder_size: row.get(16)?,
                last_commit_hash: row.get(17)?,
                last_commit_author: row.get(18)?,
                last_commit_date: row.get(19)?,
                last_commit_name: row.get(20)?,
                last_commit_partof: row.get(21)?,
            })
    })?;
    let modules_iter = module_rows.map(|x| x.unwrap());
    let modules = modules_iter.collect::<Vec<Model>>();
    Ok(modules)
}

#[cached(
    key = "String",
    time = 3600,
    option = true,
    convert = r#"{ format!("{}", id) }"#
)]
pub fn get_by_id(conn: &Connection, id: &i64) -> Option<Model> {
    let modules = query(conn, "WHERE mod.id = ?1", params![&id]).unwrap();
    if modules.is_empty() {
        return None;
    }
    Some(modules[0].clone())
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("{}", version_odoo) }"#
)]
pub fn get_by_odoo_version(conn: &Connection, version_odoo: &u8) -> Vec<Model> {
    
    query(conn, "WHERE mod.version_odoo = ?1", params![&version_odoo]).unwrap()
}

#[cached(
    key = "String",
    time = 3600,
    option = true,
    convert = r#"{ format!("{}{}{}", technical_name, version_odoo, gh_repo_id) }"#
)]
pub fn get_by_technical_name(conn: &Connection, technical_name: &str, version_odoo: &u8, gh_repo_id: &i64) -> Option<Model> {
    let modules = query(conn, "WHERE mod.technical_name = ?1 AND mod.version_odoo = ?2 AND mod.gh_repository_id = ?3 LIMIT 1", params![&technical_name, &version_odoo, &gh_repo_id]).unwrap();
    if modules.is_empty() {
        return None;
    }
    Some(modules[0].clone())
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("{}{}", technical_name, version_odoo) }"#
)]
pub fn get_by_technical_name_odoo_version(conn: &Connection, technical_name: &str, version_odoo: &u8) -> Vec<Model> {
    
    query(conn, "WHERE mod.technical_name = ?1 AND mod.version_odoo = ?2", params![&technical_name, &version_odoo]).unwrap()
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("{}{}{}{}", technical_name, version_odoo, org_name, repo_name) }"#
)]
pub fn get_by_technical_name_odoo_version_organization_name_repository_name(conn: &Connection, technical_name: &str, version_odoo: &u8, org_name: &str, repo_name: &str) -> Vec<Model> {
    let modules = query(conn, format!("INNER JOIN {} as gh_org \
        on gh_org.id = gh_repo.gh_organization_id \
        WHERE mod.technical_name = ?1 AND mod.version_odoo = ?2 AND gh_repo.name = ?3 AND gh_org.name = ?4", 
        &gh_organization::TABLE_NAME).as_str(), 
        params![&technical_name, &version_odoo, &repo_name, &org_name]).unwrap();
    modules
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("{}{}{}", technical_name, version_odoo, org_name) }"#
)]
pub fn get_by_technical_name_odoo_version_organization_name(conn: &Connection, technical_name: &str, version_odoo: &u8, org_name: &str) -> Vec<Model> {
    let modules = query(conn, format!("INNER JOIN {} as gh_org \
        on gh_org.id = gh_repo.gh_organization_id \
        WHERE mod.technical_name = ?1 AND mod.version_odoo = ?2 AND gh_org.name = ?3",
        &gh_organization::TABLE_NAME).as_str(), 
        params![&technical_name, &version_odoo, &org_name]).unwrap();
    modules
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("{}{}{}", technical_name, version_odoo, repo_name) }"#
)]
pub fn get_by_technical_name_odoo_version_repository_name(conn: &Connection, technical_name: &str, version_odoo: &u8, repo_name: &str) -> Vec<Model> {
    
    query(conn,
        "WHERE mod.technical_name = ?1 AND mod.version_odoo = ?2 AND gh_repo.name = ?3",
        params![&technical_name, &version_odoo, &repo_name]).unwrap()
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("{}", technical_name) }"#
)]
pub fn get_generic_info(conn: &Connection, technical_name: &str) -> Vec<ModuleGenericInfo> {
    let mut stmt = conn.prepare(
        format!("SELECT mod.technical_name, GROUP_CONCAT(mod.version_odoo, ','), gh_org.name || '/' || gh_repo.name as src \
        FROM {} as mod \
        INNER JOIN {} as gh_repo \
        ON gh_repo.id = mod.gh_repository_id \
        INNER JOIN {} as gh_org \
        on gh_org.id = gh_repo.gh_organization_id \
        WHERE mod.technical_name LIKE ?1 \
        GROUP BY mod.technical_name, src",
        &TABLE_NAME, &gh_repository::TABLE_NAME, &gh_organization::TABLE_NAME).as_str(),
    ).unwrap();
    let module_rows = stmt.query_map(
        params![format!("%{}%", &technical_name)], 
        |row: &rusqlite::Row<'_>| {
            Ok(ModuleGenericInfo {
                technical_name: row.get(0)?,
                versions: row.get(1)?,
                src: row.get(2)?
            })
    }).unwrap();
    let modules_iter = module_rows.map(|x| x.unwrap());
    
    modules_iter.collect::<Vec<ModuleGenericInfo>>()
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("{}{}", technical_name, version_odoo) }"#
)]
pub fn get_generic_info_by_odoo_version(conn: &Connection, technical_name: &str, version_odoo: &u8) -> Vec<ModuleGenericInfo> {
    let mut stmt = conn.prepare(
        format!("SELECT mod.technical_name, GROUP_CONCAT(mod.version_odoo, ','), gh_org.name || '/' || gh_repo.name as src \
        FROM {} as mod \
        INNER JOIN {} as gh_repo \
        ON gh_repo.id = mod.gh_repository_id \
        INNER JOIN {} as gh_org \
        on gh_org.id = gh_repo.gh_organization_id \
        WHERE mod.technical_name LIKE ?1 AND mod.version_odoo = ?2\
        GROUP BY mod.technical_name, src",
        &TABLE_NAME, &gh_repository::TABLE_NAME, &gh_organization::TABLE_NAME).as_str(),
    ).unwrap();
    let module_rows = stmt.query_map(
        params![format!("%{}%", &technical_name), &version_odoo], 
        |row: &rusqlite::Row<'_>| {
            Ok(ModuleGenericInfo {
                technical_name: row.get(0)?,
                versions: row.get(1)?,
                src: row.get(2)?
            })
    }).unwrap();
    let modules_iter = module_rows.map(|x| x.unwrap());
    
    modules_iter.collect::<Vec<ModuleGenericInfo>>()
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("{}{}{}", technical_name, version_odoo, installable) }"#
)]
pub fn get_generic_info_by_odoo_version_installable(conn: &Connection, technical_name: &str, version_odoo: &u8, installable: &bool) -> Vec<ModuleGenericInfo> {
    let mut stmt = conn.prepare(
        format!("SELECT mod.technical_name, GROUP_CONCAT(mod.version_odoo, ','), gh_org.name || '/' || gh_repo.name as src \
        FROM {} as mod \
        INNER JOIN {} as gh_repo \
        ON gh_repo.id = mod.gh_repository_id \
        INNER JOIN {} as gh_org \
        on gh_org.id = gh_repo.gh_organization_id \
        WHERE mod.technical_name LIKE ?1 AND mod.version_odoo = ?2 AND mod.installable = ?3\
        GROUP BY mod.technical_name, src",
        &TABLE_NAME, &gh_repository::TABLE_NAME, &gh_organization::TABLE_NAME).as_str(),
    ).unwrap();
    let module_rows = stmt.query_map(
        params![format!("%{}%", &technical_name), &version_odoo, &installable], 
        |row: &rusqlite::Row<'_>| {
            Ok(ModuleGenericInfo {
                technical_name: row.get(0)?,
                versions: row.get(1)?,
                src: row.get(2)?
            })
    }).unwrap();
    let modules_iter = module_rows.map(|x| x.unwrap());
    
    modules_iter.collect::<Vec<ModuleGenericInfo>>()
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("{}{}", technical_name, installable) }"#
)]
pub fn get_generic_info_by_installable(conn: &Connection, technical_name: &str, installable: &bool) -> Vec<ModuleGenericInfo> {
    let mut stmt = conn.prepare(
        format!("SELECT mod.technical_name, GROUP_CONCAT(mod.version_odoo, ','), gh_org.name || '/' || gh_repo.name as src \
        FROM {} as mod \
        INNER JOIN {} as gh_repo \
        ON gh_repo.id = mod.gh_repository_id \
        INNER JOIN {} as gh_org \
        on gh_org.id = gh_repo.gh_organization_id \
        WHERE mod.technical_name LIKE ?1 AND mod.installable = ?2\
        GROUP BY mod.technical_name, src",
        &TABLE_NAME, &gh_repository::TABLE_NAME, &gh_organization::TABLE_NAME).as_str(),
    ).unwrap();
    let module_rows = stmt.query_map(
        params![format!("%{}%", &technical_name), &installable], 
        |row: &rusqlite::Row<'_>| {
            Ok(ModuleGenericInfo {
                technical_name: row.get(0)?,
                versions: row.get(1)?,
                src: row.get(2)?
            })
    }).unwrap();
    let modules_iter = module_rows.map(|x| x.unwrap());
    
    modules_iter.collect::<Vec<ModuleGenericInfo>>()
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("{}", technical_name) }"#
)]
pub fn get_info(conn: &Connection, technical_name: &str) -> Vec<ModuleInfo> {
    let mut stmt = conn.prepare(
        format!("SELECT mod.technical_name, mod.name, mod.version_odoo, gh_org.name, gh_rep.name \
        FROM {} as mod \
        INNER JOIN {} as gh_rep \
        ON gh_rep.id = mod.gh_repository_id \
        INNER JOIN {} as gh_org \
        ON gh_org.id = gh_rep.gh_organization_id \
        WHERE technical_name = ?1",
        &TABLE_NAME, &gh_repository::TABLE_NAME, &gh_organization::TABLE_NAME).as_str(),
    ).unwrap();
    let module_rows = stmt.query_map(
        params![&technical_name], 
        |row: &rusqlite::Row<'_>| {
            Ok(ModuleInfo {
                technical_name: row.get(0)?,
                name: row.get(1)?,
                version_odoo: row.get(2)?,
                organization: row.get(3)?,
                repository: row.get(4)?
            })
    }).unwrap();
    let modules_iter = module_rows.map(|x| x.unwrap());
    
    modules_iter.collect::<Vec<ModuleInfo>>()
}

pub fn add(conn: &Connection, module_info: &ManifestInfo) -> Result<Model, rusqlite::Error> {
    let gh_org = gh_organization::add(conn, module_info.git_org.as_str())?;
    let gh_repo = gh_repository::add(conn, &gh_org.id, module_info.git_repo.as_str())?;
    let module_opt = get_by_technical_name(conn, module_info.technical_name.as_str(), &module_info.version_odoo, &gh_repo.id);
    if module_opt.is_none() {
        let create_date: String = get_sqlite_utc_now();
        conn.execute(
            format!("INSERT INTO {}(technical_name, version_odoo, name, \
                version_module, description, \
                website, license, category, \
                auto_install, application, installable, \
                gh_repository_id, \
                create_date, update_date, folder_size, \
                last_commit_hash, last_commit_author, \
                last_commit_date, last_commit_name, \
                last_commit_partof) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                &TABLE_NAME).as_str(),
                params![
                    &module_info.technical_name, 
                    &module_info.version_odoo, 
                    &module_info.name, 
                    &module_info.version_module, 
                    &module_info.description, 
                    &module_info.website, 
                    &module_info.license, 
                    &module_info.category, 
                    &module_info.auto_install, 
                    &module_info.application, 
                    &module_info.installable, 
                    &gh_repo.id, 
                    &create_date, 
                    &module_info.folder_size,
                    &module_info.last_commit_hash,
                    &module_info.last_commit_author,
                    &module_info.last_commit_date,
                    &module_info.last_commit_name,
                    &module_info.last_commit_partof
                ],
        )?;
        let new_module = Model { 
            id: conn.last_insert_rowid(), 
            technical_name: module_info.technical_name.to_string(),
            version_odoo: module_info.version_odoo,
            name: module_info.name.to_string(),
            version_module: module_info.version_module.to_string(),
            description: module_info.description.to_string(),
            website: module_info.website.to_string(),
            license: module_info.license.to_string(),
            category: module_info.category.to_string(),
            auto_install: module_info.auto_install,
            application: module_info.application,
            installable: module_info.installable,
            gh_repository_id: (gh_repo.id, gh_repo.name.clone()),
            create_date: create_date.clone(),
            update_date: create_date.clone(),
            folder_size: module_info.folder_size,
            last_commit_hash: module_info.last_commit_hash.to_string(),
            last_commit_author: module_info.last_commit_author.to_string(),
            last_commit_date: module_info.last_commit_date.to_string(),
            last_commit_name: module_info.last_commit_name.to_string(),
            last_commit_partof: module_info.last_commit_partof.to_string(),
        };
        let author_iter = module_info.author.split(",").map(|x| x.trim()).filter(|x| !x.is_empty());
        let authors = author_iter.collect::<Vec<&str>>();
        for item in authors {
            if !item.is_empty() {
                module_author::add(conn, &new_module.id, item)?;
            }
        }
        let maintainer_iter = module_info.maintainer.split(",").map(|x| x.trim()).filter(|x| !x.is_empty());
        let maintainers = maintainer_iter.collect::<Vec<&str>>();
        for item in maintainers {
            module_maintainer::add(conn, &new_module.id, item)?;
        }

        for (com_name, com_count) in &module_info.committers {
            module_committer::add(conn, &new_module.id, com_name.as_str(), com_count)?;
        }

        let _ = system_event::register_new_module(conn, module_info.technical_name.as_str(), module_info.name.as_str(), module_info.version_module.as_str(), module_info.git_org.as_str(), module_info.git_repo.as_str(), odoo_version_u8_to_string(&module_info.version_odoo).as_str());
        return Ok(new_module);
    }

    let module = module_opt.unwrap();

    // Update Committers
    for (com_name, com_count) in &module_info.committers {
        module_committer::add(conn, &module.id, com_name.as_str(),com_count)?;
    }

    // Check Authors
    let author_iter = module_info.author.split(",").map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    let authors: Vec<String> = author_iter.collect::<Vec<String>>();
    let module_authors = module_author::get_names_by_module_id(conn, &module.id);
    let authors_to_remove: Vec<&String> = module_authors.iter().filter(|item| !authors.contains(item)).collect();
    let authors_to_add: Vec<&String> = authors.iter().filter(|item| !module_authors.contains(&item.to_string())).collect();
    for author_name in authors_to_remove {
        let author_id_opt = author::get_by_name(conn, author_name);
        if author_id_opt.is_some() {
            let author_id = author_id_opt.unwrap();
            module_author::delete_by_module_id_author_id(conn, &module.id, &author_id.id)?;
            let _ = system_event::register_delete_module_author(conn, &author_id.name, &module.technical_name, &module.name, odoo_version_u8_to_string(&module.version_odoo).as_str());
        }
    }
    for author_name in authors_to_add {
        module_author::add(conn, &module.id, author_name)?;
    }

    // Check Maintainers
    let maintainer_iter = module_info.maintainer.split(",").map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    let maintainers: Vec<String> = maintainer_iter.collect::<Vec<String>>();
    let module_maintainers = module_maintainer::get_names_by_module_id(conn, &module.id);
    let maintainers_to_remove: Vec<&String> = module_maintainers.iter().filter(|item| !maintainers.contains(item)).collect();
    let maintainers_to_add: Vec<&String> = maintainers.iter().filter(|item| !module_maintainers.contains(&item.to_string())).collect();
    for maintainer_name in maintainers_to_remove {
        let maintainer_id_opt = maintainer::get_by_name(conn, maintainer_name);
        if maintainer_id_opt.is_some() {
            let maintainer_id = maintainer_id_opt.unwrap();
            module_maintainer::delete_by_module_id_maintainer_id(conn, &module.id, &maintainer_id.id)?;
            let _ = system_event::register_delete_module_maintainer(conn, &maintainer_id.name, &module.technical_name, &module.name, odoo_version_u8_to_string(&module.version_odoo).as_str());
        }
    }
    for maintainer_name in maintainers_to_add {
        module_maintainer::add(conn, &module.id, maintainer_name)?;
    }

    // 

    let mut changes: Vec<(&str, &str, &str)> = Vec::new();
    let module_auto_install_str = module.auto_install.to_string();
    let auto_install_str = module_info.auto_install.to_string();
    let module_installable_str = module.installable.to_string();
    let installable_str = module_info.installable.to_string();
    let module_application_str = module.application.to_string();
    let application_str = module_info.application.to_string();
    let module_folder_size_str = module.folder_size.to_string();
    let folder_size_str = module_info.folder_size.to_string();

    if !module.name.eq(&module_info.name) {
        changes.push(("Name", &module.name, module_info.name.as_str()));
    }
    if !module.version_module.eq(&module_info.version_module) {
        changes.push(("Version Module", &module.version_module, module_info.version_module.as_str()));
    }
    if !module.description.eq(&module_info.description) {
        changes.push(("Description", &module.description, module_info.description.as_str()));
    }
    if !module.website.eq(&module_info.website) {
        changes.push(("Website", &module.website, module_info.website.as_str()));
    }
    if !module.license.eq(&module_info.license) {
        changes.push(("License", &module.license, module_info.license.as_str()));
    }
    if !module.category.eq(&module_info.category) {
        changes.push(("Category", &module.category, module_info.category.as_str()));
    }
    if !module.auto_install.eq(&module_info.auto_install) {
        changes.push(("Auto Install", &module_auto_install_str, &auto_install_str));
    }
    if !module.installable.eq(&module_info.installable) {
        changes.push(("Installable", &module_installable_str, &installable_str));
    }
    if !module.application.eq(&module_info.application) {
        changes.push(("Application", &module_application_str, &application_str));
    }
    if !module.folder_size.eq(&module_info.folder_size) {
        changes.push(("Folder Size", &module_folder_size_str, &folder_size_str));
    }

    if changes.is_empty() {
        return Ok(module);
    }
    let update_date: String = get_sqlite_utc_now();
    conn.execute(
        format!("UPDATE {} SET name = ?1, \
        version_module = ?2, description = ?3, \
        website = ?4, license = ?5, category = ?6, \
        auto_install = ?7, application = ?8, installable = ?9, \
        update_date = ?10, folder_size = ?11 WHERE id = ?12", &TABLE_NAME).as_str(),
        params![&module_info.name, &module_info.version_module, &module_info.description, &module_info.website, &module_info.license, &module_info.category, &module_info.auto_install, &module_info.application, &module_info.installable, &update_date, &module_info.folder_size, &module.id],
    )?;
    let new_module_opt = get_by_technical_name(conn, module_info.technical_name.as_str(), &module_info.version_odoo, &gh_repo.id);    
    let odoo_ver = odoo_version_u8_to_string(&module_info.version_odoo);
    let log_upd_module_info = LogUpdateModuleInfo {
        module_technical_name: module_info.technical_name.as_str(), 
        module_name: module_info.name.as_str(), 
        module_version: module_info.version_module.as_str(), 
        org_name: module_info.git_org.as_str(), 
        repo_name: module_info.git_repo.as_str(), 
        module_version_odoo: odoo_ver.as_str(), 
        module_changes: &changes,
        last_commit_hash: &module.last_commit_hash,
        last_commit_author: &module.last_commit_author,
        last_commit_date: &module.last_commit_date,
        last_commit_name: &module.last_commit_name,
        last_commit_partof: &module.last_commit_partof,
    };
    let _ = system_event::register_update_module(
        conn, 
        &log_upd_module_info);
    Ok(new_module_opt.unwrap())
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("") }"#
)]
pub fn count(conn: &Connection) -> Vec<ModuleCountInfo> {
    let mut stmt = conn.prepare(
        format!("SELECT version_odoo, count(*) as num FROM {} GROUP BY version_odoo", &TABLE_NAME).as_str()
    ).unwrap();
    let module_rows = stmt.query_map(
        params![], 
        |row: &rusqlite::Row<'_>| {
            Ok(ModuleCountInfo {
                version_odoo: row.get(0)?,
                count: row.get(1)?,
            })
    }).unwrap();
    let modules_iter = module_rows.map(|x| x.unwrap());
    
    modules_iter.collect::<Vec<ModuleCountInfo>>()
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("") }"#
)]
pub fn count_organization(conn: &Connection) -> Vec<ModuleCountByOrganizationInfo> {
    let mut stmt = conn.prepare(
        format!("SELECT version_odoo, count(*) as num, org.name \
        FROM {} as mod \
        INNER JOIN gh_repository as repo \
        ON mod.gh_repository_id = repo.id \
        INNER JOIN gh_organization as org \
        ON repo.gh_organization_id = org.id \
        GROUP BY org.id, version_odoo \
        ORDER BY num DESC", &TABLE_NAME).as_str()
    ).unwrap();
    let module_rows = stmt.query_map(
        params![], 
        |row: &rusqlite::Row<'_>| {
            Ok(ModuleCountByOrganizationInfo {
                version_odoo: row.get(0)?,
                count: row.get(1)?,
                org_name: row.get(2)?,
            })
    }).unwrap();
    let modules_iter = module_rows.map(|x| x.unwrap());
    
    modules_iter.collect::<Vec<ModuleCountByOrganizationInfo>>()
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("") }"#
)]
pub fn rank_contributor(conn: &Connection) -> Vec<ModuleRankContributorInfo> {
    let mut stmt = conn.prepare(
        format!("SELECT * FROM (
            SELECT version_odoo, count(*) as num, au.name, RANK() OVER (PARTITION BY version_odoo ORDER BY count(*) DESC) AS contribRank
            FROM {} as mod
            INNER JOIN module_author as mod_au
            ON mod.id = mod_au.module_id
            INNER JOIN author as au
            ON mod_au.author_id = au.id
            WHERE au.name NOT LIKE '% (OCA)' AND au.name NOT LIKE 'OpenERP %' AND au.name NOT LIKE 'Odoo %'
            GROUP BY au.id, version_odoo 
            ORDER BY num DESC
        ) WHERE contribRank <= 5 ORDER BY contribRank ASC", &TABLE_NAME).as_str()
    ).unwrap();
    let module_rows = stmt.query_map(
        params![], 
        |row: &rusqlite::Row<'_>| {
            Ok(ModuleRankContributorInfo {
                version_odoo: row.get(0)?,
                count: row.get(1)?,
                contrib_name: row.get(2)?,
                rank: row.get(3)?,
            })
    }).unwrap();
    let modules_iter = module_rows.map(|x| x.unwrap());
    
    modules_iter.collect::<Vec<ModuleRankContributorInfo>>()
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("") }"#
)]
pub fn rank_committer(conn: &Connection) -> Vec<ModuleRankCommitterInfo> {
    let mut stmt = conn.prepare(
        format!("SELECT * FROM (
            SELECT version_odoo, SUM(commits), com.name, RANK() OVER (PARTITION BY version_odoo ORDER BY SUM(commits) DESC) AS commitsRank
            FROM {} as mod
            INNER JOIN module_committer as mod_com
            ON mod.id = mod_com.module_id
            INNER JOIN committer as com
            ON mod_com.committer_id = com.id
            WHERE com.name NOT IN ('Odoo Translation Bot', 'OCA-git-bot', 'Weblate', 'oca-ci')
            GROUP BY com.id, version_odoo 
        ) WHERE commitsRank <= 5 ORDER BY commitsRank ASC", &TABLE_NAME).as_str()
    ).unwrap();
    let module_rows = stmt.query_map(
        params![], 
        |row: &rusqlite::Row<'_>| {
            Ok(ModuleRankCommitterInfo {
                version_odoo: row.get(0)?,
                count: row.get(1)?,
                committer_name: row.get(2)?,
                rank: row.get(3)?,
            })
    }).unwrap();
    let modules_iter = module_rows.map(|x| x.unwrap());
    
    modules_iter.collect::<Vec<ModuleRankCommitterInfo>>()
}

#[cached(
    key = "String", 
    time = 3600, 
    convert = r#"{ format!("") }"#
)]
pub fn get_odoo_versions(conn: &Connection) -> Vec<u8> {
    let mut stmt = conn.prepare(
        format!("SELECT version_odoo FROM {} GROUP BY version_odoo ORDER BY version_odoo DESC", &TABLE_NAME).as_str()
    ).unwrap();
    let module_rows = stmt.query_map(
        params![], 
        |row: &rusqlite::Row<'_>| {
            row.get(0)
    }).unwrap();
    let modules_iter = module_rows.map(|x| x.unwrap());
    
    modules_iter.collect::<Vec<u8>>()
}


pub fn get_module_repository(conn: &Connection, modules: &[String]) -> Vec<ModuleRepositoryInfo> {
    let mod_placeholders = modules.iter().map(|s| format!("'{s}'")).collect::<Vec<String>>().join(", ");
    let mut stmt = conn.prepare(
        format!("SELECT mod.technical_name, gh_repo.name 
            FROM {} AS mod 
            INNER JOIN gh_repository as gh_repo 
            ON gh_repo.id = mod.gh_repository_id 
            WHERE mod.technical_name IN ({}) 
            GROUP BY mod.technical_name", &TABLE_NAME, mod_placeholders).as_str()
    ).unwrap();
    let module_rows = stmt.query_map(
        params![], 
        |row: &rusqlite::Row<'_>| {
            Ok(ModuleRepositoryInfo {
                technical_name: row.get(0)?,
                repository_name: row.get(1)?,
            })
    }).unwrap();
    let modules_iter = module_rows.map(|x| x.unwrap());
    
    modules_iter.collect::<Vec<ModuleRepositoryInfo>>()
}

pub fn delete_outdated(conn: &Connection, gh_repo_id: &i64, version_odoo: &u8,  module_ids: &[i64]) -> Result<usize, rusqlite::Error> {
    if !module_ids.is_empty() {
        let ids_str = module_ids.iter()
            .map(|x| x.to_string())
            .collect::<Vec<String>>()
            .join(", ");
        return conn.execute(
            &format!("DELETE FROM {} WHERE gh_repository_id = ?1 AND version_odoo = ?2 AND id not in ({})", &TABLE_NAME, &ids_str),
            params![&gh_repo_id, &version_odoo],
        )
    }
    Ok(0)
}