// Copyright Alexandre D. Díaz
mod analyzer;
mod anygitclient;
mod clients;
mod config;
mod gitclient;
mod pypi;
mod security;

use named_lock::NamedLock;
use regex::Regex;
use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::path::Path;
use std::time::Instant;

use analyzer::OGHCollectorAnalyzer;
use anygitclient::AnyGitClient;
use clients::github::GithubClient;
use clients::gitlab::GitlabClient;
use config::{GitType, OGHCollectorConfig};
use gitclient::{GitClient, RepoInfo};
use oghutils::version::odoo_version_u8_to_string;
use pypi::PypiClient;
use sqlitedb::models;

// The guard must stay alive for the whole run: dropping it releases the lock,
// which is why this returns it instead of letting it die inside the function.
fn try_lock(config: &OGHCollectorConfig) -> named_lock::NamedLockGuard {
    let source_info = config.get_source().split('/').collect::<Vec<&str>>();
    let org = source_info[0];
    let lock_name = format!("OGHCollector::{org}");
    let lock = NamedLock::create(lock_name.as_str()).expect("Can't create the collector lock");
    match lock.try_lock() {
        Ok(guard) => guard,
        Err(_) => {
            eprintln!(
                "There is already an instance of OGHCollector working with '{org}'. Exiting..."
            );
            std::process::exit(1);
        }
    }
}

#[tokio::main]
async fn main() {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));
    let args: Vec<String> = env::args().collect();
    let config = OGHCollectorConfig::new(&args);

    let _lock_guard = try_lock(&config);

    let git_client = match config.get_git_type() {
        GitType::Github => {
            AnyGitClient::Github(GithubClient::new(config.get_token(), config.get_base_url()))
        }
        GitType::Gitlab => {
            AnyGitClient::Gitlab(GitlabClient::new(config.get_token(), config.get_base_url()))
        }
    };
    let pypi_client = PypiClient::new();

    let db_path = "data/data.db";
    if let Some(parent) = Path::new(db_path).parent() {
        fs::create_dir_all(parent).unwrap();
    }
    if !Path::new(db_path).exists() {
        File::create(db_path).unwrap();
    }

    let pool = sqlitedb::new_write_pool(db_path);
    let mut conn = pool.get().unwrap();
    sqlitedb::run_migrations(&mut conn).expect("Can't run migrations");

    let odoo_ver = config.get_version_odoo();
    let odoo_ver_str = odoo_version_u8_to_string(odoo_ver);
    let start_time = Instant::now();
    let _ = models::system_event::register_started_task_collector(
        &mut conn,
        config.get_source(),
        &odoo_ver_str,
    );
    log::info!("Cloning/Updating ({})...", odoo_ver_str);
    let mut repo_infos: Vec<RepoInfo> = Vec::new();
    if config.get_mode() == "org" {
        repo_infos = git_client
            .clone_org_repos(
                config.get_source(),
                config.get_branch(),
                config.get_repos_path(),
            )
            .await;
    } else if config.get_mode() == "repo" {
        let Some((user_name, repo_name)) = config.get_source().split_once('/') else {
            eprintln!(
                "Invalid source '{}': repo mode expects '<user>/<repo>'",
                config.get_source()
            );
            std::process::exit(1);
        };
        let user_name = user_name.to_string();
        let repo_name = repo_name.to_string();
        let repo_url = format!("https://github.com/{user_name}/{repo_name}.git");
        let res_opt = git_client.clone_or_update_repo(
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

    log::info!("Scanning '{}' repos for migration PRs...", repo_infos.len());
    for repo_info in &repo_infos {
        let gh_org = models::gh_organization::add(&mut conn, repo_info.get_org()).unwrap();
        let gh_repo =
            models::gh_repository::add(&mut conn, &gh_org.id, repo_info.get_name()).unwrap();
        let migration_prs = git_client
            .get_open_migration_pull_requests(repo_info.get_full_path(), config.get_branch())
            .await;
        let mut prids: Vec<i64> = Vec::with_capacity(migration_prs.len());
        for pr in &migration_prs {
            prids.push(pr.number);
            models::pull_request::add(
                &mut conn,
                &pr.title,
                &pr.module_technical_name,
                &pr.number,
                odoo_ver,
                &gh_repo.id,
                pr.created_at.as_deref(),
                pr.ci_status.as_deref(),
                pr.last_message_at.as_deref(),
            )
            .unwrap();
        }
        let outdated =
            models::pull_request::find_outdated(&mut conn, &gh_repo.id, odoo_ver, &prids)
                .unwrap_or_default();
        let mut merged_prids: Vec<i64> = Vec::with_capacity(outdated.len());
        for pr in &outdated {
            if let Some(true) = git_client
                .is_pull_request_merged(repo_info.get_full_path(), &pr.prid)
                .await
            {
                merged_prids.push(pr.prid);
            }
        }
        let _ = models::pull_request::delete_outdated(
            &mut conn,
            &gh_repo.id,
            odoo_ver,
            &prids,
            &merged_prids,
        );
    }

    log::info!("Analazyng '{}' repos...", repo_infos.len());
    let analyzer = OGHCollectorAnalyzer::new(odoo_ver);
    let manifest_infos = analyzer.get_module_info(&mut conn, config.get_read_paths(), &repo_infos);
    let manifest_count = &manifest_infos.len();
    if manifest_count.gt(&0) {
        log::info!("Saving '{}' repos info...", manifest_infos.len());
        let mut module_ids_by_repo: HashMap<i64, Vec<i64>> = HashMap::new();
        let dep_type_module = models::dependency_type::get_by_name(&mut conn, "module")
            .expect("Can't found the module dependecy type");
        let dep_type_python = models::dependency_type::get_by_name(&mut conn, "python")
            .expect("Can't found the python dependecy type");
        let dep_type_bin = models::dependency_type::get_by_name(&mut conn, "bin")
            .expect("Can't found the bin dependecy type");
        let re = Regex::new(r"^([^><=]+).+?([^><=]+)$").unwrap();
        for manifest in manifest_infos {
            let mut new_module_info = manifest.clone();
            new_module_info.version_odoo = *odoo_ver; // It is forced because some modules do not have this data correctly.
            let new_module = models::module::add(&mut conn, &new_module_info).unwrap();
            let module_ids = module_ids_by_repo
                .entry(new_module.gh_repository_id)
                .or_default();
            module_ids.push(new_module.id);

            // The analyzer already skipped re-parsing this module's source
            // (see OGHCollectorAnalyzer::get_module_info) when its last
            // commit hash matched what's already stored, so `analysis` here
            // is empty - replacing stored data with it would wipe it.
            if !new_module_info.source_unchanged {
                // Resolve (or start) the history entry for this manifest version,
                // then replace the module's code analysis (views touched, models
                // defined/extended with their fields and public methods, and
                // every other record it touches - access groups, record rules,
                // access rights, ...) scoped to that version, on every run,
                // independent of whether any manifest field changed. A prior
                // version's snapshot is left untouched - only its own
                // module_version_id gets wiped/rebuilt.
                let module_version = models::module_version::get_or_create(
                    &mut conn,
                    &new_module.id,
                    &new_module_info.version_module,
                )
                .unwrap();
                models::module_view::replace_for_module(
                    &mut conn,
                    &new_module.id,
                    &module_version.id,
                    &new_module_info.analysis.views,
                )
                .unwrap();
                models::module_model::replace_for_module(
                    &mut conn,
                    &new_module.id,
                    &module_version.id,
                    &new_module_info.analysis.models,
                )
                .unwrap();
                models::module_record::replace_for_module(
                    &mut conn,
                    &new_module.id,
                    &module_version.id,
                    &new_module_info.analysis.records,
                )
                .unwrap();
                models::module_controller::replace_for_module(
                    &mut conn,
                    &new_module.id,
                    &module_version.id,
                    &new_module_info.analysis.controllers,
                )
                .unwrap();

                // Static security checks over the records and HTTP controllers
                // just analyzed: grave findings land in module_security_warning
                // (shown on the module detail page), minor ones only leave a
                // system_event log line.
                let mut sec_warnings = security::analyze_records(&new_module_info.analysis.records);
                sec_warnings.extend(security::analyze_controllers(
                    &new_module_info.analysis.controllers,
                ));
                for w in sec_warnings
                    .iter()
                    .filter(|w| w.severity != models::module_security_warning::SEVERITY_ERROR)
                {
                    let _ = models::system_event::register_security_warning(
                        &mut conn,
                        &new_module.technical_name,
                        &new_module.name,
                        &odoo_ver_str,
                        w.xml_id.as_deref(),
                        &w.message,
                    );
                }
                models::module_security_warning::replace_for_module(
                    &mut conn,
                    &new_module.id,
                    &module_version.id,
                    &sec_warnings,
                )
                .unwrap();
            }

            // Check Odoo Version
            if manifest.version_odoo.ne(odoo_ver) && manifest.installable {
                let repo_name =
                    models::gh_repository::get_by_id(&mut conn, &new_module.gh_repository_id)
                        .map(|r| r.name)
                        .unwrap_or_default();
                let _ = models::system_event::register_problem_module_version(
                    &mut conn,
                    &new_module.technical_name,
                    &new_module.name,
                    &repo_name,
                    odoo_version_u8_to_string(&manifest.version_odoo).as_str(),
                    &odoo_ver_str,
                );
            }

            // Add Odoo deps.
            let module_depends = models::dependency_module::get_names(
                &mut conn,
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
                let module_depend_id_opt = models::dependency::get_by_name(
                    &mut conn,
                    &dep_type_module.id,
                    module_depend_name,
                );
                if let Some(module_depend_id) = module_depend_id_opt {
                    let _ = models::dependency_module::delete_by_module_id_dependecy_id(
                        &mut conn,
                        &new_module.id,
                        &module_depend_id.id,
                    );
                    let _ = models::system_event::register_delete_module_dependency(
                        &mut conn,
                        &module_depend_id.name,
                        "Odoo",
                        &new_module.technical_name,
                        &new_module.name,
                        odoo_version_u8_to_string(&(new_module.version_odoo as u8)).as_str(),
                    );
                }
            }
            for module_depend_name in module_depends_to_add {
                models::dependency_module::add(
                    &mut conn,
                    &dep_type_module.id,
                    module_depend_name,
                    &new_module.id,
                )
                .unwrap();
            }

            // Add python deps.
            let module_depends_python = models::dependency_module::get_names(
                &mut conn,
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
                let module_depend_python_id_opt = models::dependency::get_by_name(
                    &mut conn,
                    &dep_type_python.id,
                    module_depends_python_name,
                );
                if let Some(module_depend_id) = module_depend_python_id_opt {
                    let _ = models::dependency_module::delete_by_module_id_dependecy_id(
                        &mut conn,
                        &new_module.id,
                        &module_depend_id.id,
                    );
                    let _ = models::system_event::register_delete_module_dependency(
                        &mut conn,
                        &module_depend_id.name,
                        "Python",
                        &new_module.technical_name,
                        &new_module.name,
                        odoo_version_u8_to_string(&(new_module.version_odoo as u8)).as_str(),
                    );
                }
            }
            for module_depends_python_name in module_depends_python_to_add {
                let dep_mod = models::dependency_module::add(
                    &mut conn,
                    &dep_type_python.id,
                    module_depends_python_name,
                    &new_module.id,
                )
                .unwrap();
                // Check OSV
                if module_depends_python_name.contains("==")
                    || module_depends_python_name.contains("<")
                {
                    let Some(caps) = re.captures(module_depends_python_name) else {
                        log::warn!(
                            "Can't parse python dependency '{module_depends_python_name}'. Skipping OSV check..."
                        );
                        continue;
                    };
                    let package_name = caps
                        .get(1)
                        .map_or(String::new(), |m| m.as_str().trim().to_string());
                    let mut package_ver = caps
                        .get(2)
                        .map_or(String::new(), |m| m.as_str().trim().to_string());
                    if !module_depends_python_name.contains("<=")
                        && module_depends_python_name.contains("<")
                    {
                        // A PyPI/network hiccup only skips this dep's OSV check,
                        // never the whole collector run.
                        let package_ver_opt = match pypi_client
                            .get_nearest_version(&package_name, &package_ver)
                            .await
                        {
                            Ok(res) => res,
                            Err(err) => {
                                log::warn!(
                                    "Can't query PyPI for '{package_name}': {err}. Skipping OSV check..."
                                );
                                continue;
                            }
                        };
                        let Some(nearest_ver) = package_ver_opt else {
                            log::info!(
                                "No valid release version found for '{}': '{}' ({}). Skipping...",
                                module_depends_python_name,
                                package_name,
                                package_ver
                            );
                            continue;
                        };
                        package_ver = nearest_ver;
                    }
                    let package_info = match pypi_client
                        .get_package_info(&package_name, Some(&package_ver))
                        .await
                    {
                        Ok(res) => res,
                        Err(err) => {
                            log::warn!(
                                "Can't query PyPI for '{package_name}': {err}. Skipping OSV check..."
                            );
                            continue;
                        }
                    };
                    let vulns_opt = package_info["vulnerabilities"].as_array();
                    if let Some(vulns) = vulns_opt {
                        for vuln in vulns {
                            let Some(vuln_id) = vuln["id"].as_str() else {
                                continue;
                            };
                            let fixed_in: String = vuln["fixed_in"]
                                .as_array()
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|x| x.as_str())
                                        .collect::<Vec<&str>>()
                                        .join(", ")
                                })
                                .unwrap_or_default();
                            models::dependency_osv::add(
                                &mut conn,
                                &dep_mod.id,
                                vuln_id,
                                vuln["details"].as_str().unwrap_or(""),
                                fixed_in.as_str(),
                            )
                            .unwrap();
                        }
                    }
                }
            }

            // Add bin deps.
            let module_depends_bin =
                models::dependency_module::get_names(&mut conn, &new_module.id, &dep_type_bin.id);

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
                let module_depend_bin_id_opt = models::dependency::get_by_name(
                    &mut conn,
                    &dep_type_bin.id,
                    module_depends_bin_name,
                );
                if let Some(module_depend_id) = module_depend_bin_id_opt {
                    let _ = models::dependency_module::delete_by_module_id_dependecy_id(
                        &mut conn,
                        &new_module.id,
                        &module_depend_id.id,
                    );
                    let _ = models::system_event::register_delete_module_dependency(
                        &mut conn,
                        &module_depend_id.name,
                        "Bin",
                        &new_module.technical_name,
                        &new_module.name,
                        odoo_version_u8_to_string(&(new_module.version_odoo as u8)).as_str(),
                    );
                }
            }
            for module_depends_bin_name in module_depends_bin_to_add {
                models::dependency_module::add(
                    &mut conn,
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
                models::module::delete_outdated(&mut conn, &key, config.get_version_odoo(), &value)
                    .unwrap();
            }
        }
        let _ = models::system_event::register_finished_task_collector(
            &mut conn,
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
