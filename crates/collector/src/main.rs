// Copyright 2025 Alexandre D. Díaz
mod analyzer;
mod config;
mod github;
mod pypi;

use r2d2_sqlite::{self, SqliteConnectionManager};
use regex::Regex;
use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::path::Path;
use std::time::Instant;

use analyzer::OGHCollectorAnalyzer;
use config::OGHCollectorConfig;
use github::{GithubClient, RepoInfo};
use oghutils::version::odoo_version_u8_to_string;
use pypi::PypiClient;
use sqlitedb::{models, Pool};

#[tokio::main]
async fn main() {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));
    let args: Vec<String> = env::args().collect();
    let config = OGHCollectorConfig::new(&args);
    let gh_client = GithubClient::new(config.get_token());
    let pypi_client = PypiClient::new();

    let db_path = "data/data.db";
    if let Some(parent) = Path::new(db_path).parent() {
        fs::create_dir_all(parent).unwrap();
    }
    if !Path::new(db_path).exists() {
        File::create(db_path).unwrap();
    }
    let manager = SqliteConnectionManager::file(db_path);
    let pool = Pool::new(manager).unwrap();
    let conn = pool.get().unwrap();

    models::prepare_schema(&conn).expect("Can't create the database");
    models::populate_basics(&conn).expect("Can't initialize the database");

    let odoo_ver = config.get_version_odoo();
    let odoo_ver_str = odoo_version_u8_to_string(odoo_ver);
    let start_time = Instant::now();
    log::info!("Cloning/Updating ({})...", &odoo_ver_str);
    let mut repo_infos: Vec<RepoInfo> = Vec::new();
    if config.get_mode() == "org" {
        repo_infos = gh_client
            .clone_org_repos(
                config.get_source(),
                config.get_branch(),
                config.get_repos_path(),
            )
            .await;
    } else if config.get_mode() == "repo" {
        let src_parts = config.get_source().split("/").collect::<Vec<&str>>();
        let user_name = src_parts[0].to_string();
        let repo_name = src_parts[1].to_string();
        let repo_url = format!("https://github.com/{user_name}/{repo_name}.git");
        let res_opt = gh_client.clone_or_update_repo(
            &user_name,
            &repo_name,
            &repo_url,
            config.get_branch(),
            config.get_repos_path(),
        );
        if let Some(res) = res_opt {
            repo_infos.push(res);
        } else {
            log::info!("'{repo_url}' Is not a valid Odoo modules repository!");
        }
    }

    log::info!("Analazyng '{}' repos...", repo_infos.len());
    let analyzer = OGHCollectorAnalyzer::new(odoo_ver);
    let manifest_infos = analyzer.get_module_info(config.get_read_paths(), &repo_infos);
    let manifest_count = &manifest_infos.len();
    if manifest_count.gt(&0) {
        log::info!("Saving '{}' repos info...", manifest_infos.len());
        let mut module_ids_by_repo: HashMap<i64, Vec<i64>> = HashMap::new();
        let dep_type_module = models::dependency_type::get_by_name_no_cache(&conn, "module")
            .expect("Can't found the module dependecy type");
        let dep_type_python = models::dependency_type::get_by_name_no_cache(&conn, "python")
            .expect("Can't found the python dependecy type");
        let dep_type_bin = models::dependency_type::get_by_name_no_cache(&conn, "bin")
            .expect("Can't found the bin dependecy type");
        let re = Regex::new(r"^([^><=]+).+?([^><=]+)$").unwrap();
        for manifest in manifest_infos {
            let mut new_module_info = manifest.clone();
            new_module_info.version_odoo = *odoo_ver; // It is forced because some modules do not have this data correctly.
            let new_module = models::module::add(&conn, &new_module_info).unwrap();
            let module_ids = module_ids_by_repo
                .entry(new_module.gh_repository_id.0)
                .or_default();
            module_ids.push(new_module.id);

            // Check Odoo Version
            if manifest.version_odoo.ne(odoo_ver) && manifest.installable {
                let _ = models::system_event::register_problem_module_version(
                    &conn,
                    &new_module.technical_name,
                    &new_module.name,
                    &new_module.gh_repository_id.1,
                    odoo_version_u8_to_string(&manifest.version_odoo).as_str(),
                    &odoo_ver_str,
                );
            }

            // Add Odoo deps.
            let module_depends = models::dependency_module::get_names_no_cache(
                &conn,
                &new_module.id,
                &dep_type_module.id,
            );
            let module_depends_to_remove: Vec<&String> = module_depends
                .iter()
                .filter(|item| !manifest.depends.contains(item))
                .collect();
            let module_depends_to_add: Vec<&String> = manifest
                .depends
                .iter()
                .filter(|item| !module_depends.contains(&item.to_string()))
                .collect();
            for module_depend_name in module_depends_to_remove {
                let module_depend_id_opt = models::dependency::get_by_name_no_cache(
                    &conn,
                    &dep_type_module.id,
                    module_depend_name,
                );
                if let Some(module_depend_id) = module_depend_id_opt {
                    let _ = models::dependency_module::delete_by_module_id_dependecy_id(
                        &conn,
                        &new_module.id,
                        &module_depend_id.id,
                    );
                    let _ = models::system_event::register_delete_module_dependency(
                        &conn,
                        &module_depend_id.name,
                        "Odoo",
                        &new_module.name,
                        &new_module.technical_name,
                        odoo_version_u8_to_string(&new_module.version_odoo).as_str(),
                    );
                }
            }
            for module_depend_name in module_depends_to_add {
                models::dependency_module::add(
                    &conn,
                    &dep_type_module.id,
                    module_depend_name,
                    &new_module.id,
                )
                .unwrap();
            }

            // Add python deps.
            let module_depends_python = models::dependency_module::get_names_no_cache(
                &conn,
                &new_module.id,
                &dep_type_python.id,
            );
            let module_depends_python_to_remove: Vec<&String> = module_depends_python
                .iter()
                .filter(|item| !manifest.external_depends_python.contains(item))
                .collect();
            let module_depends_python_to_add: Vec<&String> = manifest
                .external_depends_python
                .iter()
                .filter(|item| !module_depends_python.contains(&item.to_string()))
                .collect();
            for module_depends_python_name in module_depends_python_to_remove {
                let module_depend_python_id_opt = models::dependency::get_by_name_no_cache(
                    &conn,
                    &dep_type_python.id,
                    module_depends_python_name,
                );
                if let Some(module_depend_id) = module_depend_python_id_opt {
                    let _ = models::dependency_module::delete_by_module_id_dependecy_id(
                        &conn,
                        &new_module.id,
                        &module_depend_id.id,
                    );
                    let _ = models::system_event::register_delete_module_dependency(
                        &conn,
                        &module_depend_id.name,
                        "Python",
                        &new_module.name,
                        &new_module.technical_name,
                        odoo_version_u8_to_string(&new_module.version_odoo).as_str(),
                    );
                }
            }
            for module_depends_python_name in module_depends_python_to_add {
                let dep_mod = models::dependency_module::add(
                    &conn,
                    &dep_type_python.id,
                    module_depends_python_name,
                    &new_module.id,
                )
                .unwrap();
                // Check OSV
                if module_depends_python_name.contains("==")
                    || module_depends_python_name.contains("<")
                {
                    let caps = re.captures(module_depends_python_name).unwrap();
                    let package_name = caps
                        .get(1)
                        .map_or(String::new(), |m| m.as_str().trim().to_string());
                    let mut package_ver = caps
                        .get(2)
                        .map_or(String::new(), |m| m.as_str().trim().to_string());
                    if !module_depends_python_name.contains("<=")
                        && module_depends_python_name.contains("<")
                    {
                        let package_ver_opt = pypi_client
                            .get_nearest_version(&package_name, &package_ver)
                            .await
                            .unwrap();
                        if package_ver_opt.is_none() {
                            log::info!(
                                "No valid release version found for '{}': '{}' ({}). Skipping...",
                                &module_depends_python_name,
                                &package_name,
                                &package_ver
                            );
                            continue;
                        }
                        package_ver = package_ver_opt.unwrap();
                    }
                    let package_info = pypi_client
                        .get_package_info(&package_name, Some(&package_ver))
                        .await
                        .unwrap();
                    let vulns_opt = package_info["vulnerabilities"].as_array();
                    if let Some(vulns) = vulns_opt {
                        for vuln in vulns {
                            let fixed_in: String = vuln["fixed_in"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|x| x.as_str().unwrap())
                                .collect::<Vec<&str>>()
                                .join(", ");
                            models::dependency_osv::add(
                                &conn,
                                &dep_mod.id,
                                vuln["id"].as_str().unwrap(),
                                vuln["details"].as_str().unwrap(),
                                fixed_in.as_str(),
                            )
                            .unwrap();
                        }
                    }
                }
            }

            // Add bin deps.
            let module_depends_bin = models::dependency_module::get_names_no_cache(
                &conn,
                &new_module.id,
                &dep_type_bin.id,
            );

            let module_depends_bin_to_remove: Vec<&String> = module_depends_bin
                .iter()
                .filter(|item| !manifest.external_depends_bin.contains(item))
                .collect();
            let module_depends_bin_to_add: Vec<&String> = manifest
                .external_depends_bin
                .iter()
                .filter(|item| !module_depends_bin.contains(&item.to_string()))
                .collect();
            for module_depends_bin_name in module_depends_bin_to_remove {
                let module_depend_bin_id_opt = models::dependency::get_by_name_no_cache(
                    &conn,
                    &dep_type_bin.id,
                    module_depends_bin_name,
                );
                if let Some(module_depend_id) = module_depend_bin_id_opt {
                    let _ = models::dependency_module::delete_by_module_id_dependecy_id(
                        &conn,
                        &new_module.id,
                        &module_depend_id.id,
                    );
                    let _ = models::system_event::register_delete_module_dependency(
                        &conn,
                        &module_depend_id.name,
                        "Bin",
                        &new_module.name,
                        &new_module.technical_name,
                        odoo_version_u8_to_string(&new_module.version_odoo).as_str(),
                    );
                }
            }
            for module_depends_bin_name in module_depends_bin_to_add {
                models::dependency_module::add(
                    &conn,
                    &dep_type_bin.id,
                    module_depends_bin_name,
                    &new_module.id,
                )
                .unwrap();
            }
        }

        log::info!("Removing outdated modules info...");
        for (key, value) in module_ids_by_repo {
            if !value.is_empty() {
                models::module::delete_outdated(&conn, &key, config.get_version_odoo(), &value)
                    .unwrap();
            }
        }
        let _ = models::system_event::register_finished_task_collector(
            &conn,
            &start_time.elapsed().as_secs().to_string(),
            &manifest_count.to_string(),
            repo_infos[0].get_org(),
            odoo_version_u8_to_string(config.get_version_odoo()).as_str(),
        );
    } else {
        log::info!("Nothing to do!");
    }

    log::info!("All done. Bye!");
}
