// Copyright 2025 Alexandre D. Díaz
use fs_extra::dir::get_size;
use pyo3::prelude::*;
use pyo3::types::*;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

use oghutils::version::OdooVersion;
use sqlitedb::models::module::ManifestInfo;

use crate::github::RepoInfo;

fn count_element_function<I>(it: I) -> HashMap<I::Item, u32>
where
    I: IntoIterator,
    I::Item: Eq + core::hash::Hash,
{
    let mut result = HashMap::new();

    for item in it {
        *result.entry(item).or_insert(0) += 1;
    }

    result
}

#[derive(Debug)]
pub struct GitInfo {
    pub last_commit_hash: String,
    pub last_commit_author: String,
    pub last_commit_date: String,
    pub last_commit_name: String,
    pub last_commit_partof: String,
}

#[derive(Debug)]
pub struct OGHCollectorAnalyzer {
    version_odoo: u8,
}

impl OGHCollectorAnalyzer {
    pub fn new(version_odoo: &u8) -> OGHCollectorAnalyzer {
        OGHCollectorAnalyzer {
            version_odoo: *version_odoo,
        }
    }

    fn is_odoo_module_folder(
        &self,
        mod_path: &std::path::PathBuf,
    ) -> Result<Option<String>, io::Error> {
        if !mod_path.is_dir() {
            return Ok(None);
        };
        for entry in fs::read_dir(mod_path)? {
            let path = entry?.path();
            if !path.is_dir() {
                if path.ends_with("__manifest__.py") {
                    return Ok(Some("__manifest__.py".to_string()));
                } else if path.ends_with("__openerp__.py") {
                    return Ok(Some("__openerp__.py".to_string()));
                }
            }
        }
        Ok(None)
    }

    fn get_git_info(&self, folder_path: &std::path::PathBuf) -> Result<GitInfo, ExitStatus> {
        log::info!("Get git info...");
        let output_fetch = Command::new("git")
            .current_dir(folder_path)
            .arg("--no-pager")
            .arg("log")
            .arg("--pretty=%H~~%an~~%aD~~%s~~%b")
            .arg("-1")
            .arg("--")
            .arg(".")
            .output()
            .unwrap_or_else(|_| panic!("{}", "Can't get git info".to_string()));
        if !output_fetch.status.success() {
            return Err(output_fetch.status);
        }

        let output = String::from_utf8_lossy(&output_fetch.stdout).to_string();
        let re =
            Regex::new(r"([0-9a-f]+)~~([^\n]+)~~([^\n]+)~~(.+)~~(?:[\S\s]+Part-of:\s([^\n]+))?")
                .unwrap();
        let caps = re.captures(&output).unwrap();
        Ok(GitInfo {
            last_commit_hash: caps
                .get(1)
                .map_or(String::new(), |m| m.as_str().trim().to_string()),
            last_commit_author: caps
                .get(2)
                .map_or(String::new(), |m| m.as_str().trim().to_string()),
            last_commit_date: caps
                .get(3)
                .map_or(String::new(), |m| m.as_str().trim().to_string()),
            last_commit_name: caps
                .get(4)
                .map_or(String::new(), |m| m.as_str().trim().to_string()),
            last_commit_partof: caps
                .get(5)
                .map_or(String::new(), |m| m.as_str().trim().to_string()),
        })
    }

    fn get_git_committers(
        &self,
        folder_path: &std::path::PathBuf,
    ) -> Result<HashMap<String, u32>, ExitStatus> {
        log::info!("Get git committer info...");
        let output_fetch = Command::new("git")
            .current_dir(folder_path)
            .arg("--no-pager")
            .arg("log")
            .arg("--pretty=%cn")
            .arg("--")
            .arg(".")
            .output()
            .unwrap_or_else(|_| panic!("{}", "Can't get git info".to_string()));
        if !output_fetch.status.success() {
            return Err(output_fetch.status);
        }

        let output = String::from_utf8_lossy(&output_fetch.stdout).to_string();
        let counter: HashMap<String, u32> =
            count_element_function(output.lines().map(|l| l.to_string()));
        Ok(counter)
    }

    fn read_manifest(
        &self,
        org_name: &str,
        repo_name: &str,
        module_name: &str,
        manifest_path: &str,
    ) -> PyResult<ManifestInfo> {
        log::info!("Reading Manifest: {manifest_path}");
        Python::with_gil(|py| {
            let code = fs::read_to_string(manifest_path).unwrap();
            let manifest: &PyDict = py.eval(&code, None, None)?.extract()?;
            // name
            let name_opt = manifest.get_item("name");
            let name: String = if let Some(name_value) = name_opt {
                name_value.downcast::<PyString>()?.extract::<String>()?
            } else {
                String::new()
            };
            // description
            let description_opt = manifest.get_item("description");
            let description: String = if let Some(description_value) = description_opt {
                description_value
                    .downcast::<PyString>()?
                    .extract::<String>()?
            } else {
                String::new()
            };
            // author
            let author_opt = manifest.get_item("author");
            let author: String = if let Some(author_value) = author_opt {
                match author_value.downcast::<PyString>() {
                    Ok(pyval) => pyval.extract::<String>()?,
                    Err(_) => match author_value.downcast::<PyList>() {
                        Ok(pyval) => {
                            let author_vec = pyval.extract::<Vec<String>>()?;
                            author_vec.join(", ")
                        }
                        Err(_) => String::new(),
                    },
                }
            } else {
                String::new()
            };
            // website
            let website_opt = manifest.get_item("website");
            let website: String = if let Some(website_value) = website_opt {
                website_value.downcast::<PyString>()?.extract::<String>()?
            } else {
                String::new()
            };
            // license
            let license_opt = manifest.get_item("license");
            let license: String = if let Some(license_value) = license_opt {
                license_value.downcast::<PyString>()?.extract::<String>()?
            } else {
                "LGPL-3".to_string()
            };
            // category
            let category_opt = manifest.get_item("category");
            let category: String = if let Some(category_value) = category_opt {
                category_value.downcast::<PyString>()?.extract::<String>()?
            } else {
                "Uncategorized".to_string()
            };
            // auto_install
            let auto_install_opt = manifest.get_item("auto_install");
            let auto_install: bool = if let Some(auto_install_value) = auto_install_opt {
                match auto_install_value.downcast::<PyBool>() {
                    Ok(pyval) => pyval.extract::<bool>()?,
                    Err(_) => true,
                }
            } else {
                false
            };
            // version_odoo, version_module
            let version_opt = manifest.get_item("version");
            let version_odoo: u8;
            let version_module: String = if let Some(version_value) = version_opt {
                let version = version_value.downcast::<PyString>()?.extract::<String>()?;
                let odoo_ver = OdooVersion::new(&version, &self.version_odoo);
                version_odoo = *odoo_ver.get_version_odoo();
                odoo_ver.get_version_module().clone()
            } else {
                version_odoo = self.version_odoo;
                "0.1.0".to_string()
            };
            // application
            let application_opt = manifest.get_item("application");
            let application: bool = if let Some(application_value) = application_opt {
                application_value.downcast::<PyBool>()?.extract::<bool>()?
            } else {
                false
            };
            // installable
            let installable_opt = manifest.get_item("installable");
            let installable: bool = if let Some(installable_value) = installable_opt {
                match installable_value.downcast::<PyBool>() {
                    Ok(pyval) => pyval.extract::<bool>()?,
                    Err(_) => true,
                }
            } else {
                true
            };
            // maintainer
            let maintainer_opt = manifest.get_item("maintainer");
            let maintainer: String = if let Some(maintainer_value) = maintainer_opt {
                match maintainer_value.downcast::<PyString>() {
                    Ok(pyval) => pyval.extract::<String>()?,
                    Err(_) => match maintainer_value.downcast::<PyList>() {
                        Ok(pyval) => {
                            let maintainer_vec = pyval.extract::<Vec<String>>()?;
                            maintainer_vec.join(", ")
                        }
                        Err(_) => author.clone(),
                    },
                }
            } else {
                author.clone()
            };
            // depends
            let depends_opt = manifest.get_item("depends");
            let depends: Vec<String> = if let Some(depends_value) = depends_opt {
                depends_value
                    .downcast::<PyList>()?
                    .extract::<Vec<String>>()?
            } else {
                Vec::new()
            };
            let external_depends_opt = manifest.get_item("external_dependencies");
            let mut external_depends_python_set: HashSet<String> = HashSet::new();
            let mut external_depends_bin_set: HashSet<String> = HashSet::new();
            if let Some(external_depends_value) = external_depends_opt {
                let depends_dict = external_depends_value.downcast::<PyDict>()?;
                let depends_python_opt = depends_dict.get_item("python");
                if depends_python_opt.is_some() {
                    let python_deps = match depends_python_opt {
                        Some(py_any) => match py_any.downcast::<PyList>() {
                            Ok(pyval) => pyval,
                            Err(_) => PyList::empty(py),
                        },
                        None => PyList::empty(py),
                    };
                    for dep_name in python_deps {
                        external_depends_python_set.insert(dep_name.extract()?);
                    }
                }
                let depends_bin_opt = depends_dict.get_item("bin");
                if depends_bin_opt.is_some() {
                    let bin_deps = match depends_bin_opt {
                        Some(py_any) => match py_any.downcast::<PyList>() {
                            Ok(pyval) => pyval,
                            Err(_) => PyList::empty(py),
                        },
                        None => PyList::empty(py),
                    };
                    for dep_name in bin_deps {
                        external_depends_bin_set.insert(dep_name.extract()?);
                    }
                }
                // This is a unofficial way to get "debian" pacakage name (used by OCA CI)
                let depends_deb_opt = depends_dict.get_item("deb");
                if depends_deb_opt.is_some() {
                    let bin_deps = match depends_deb_opt {
                        Some(py_any) => match py_any.downcast::<PyList>() {
                            Ok(pyval) => pyval,
                            Err(_) => PyList::empty(py),
                        },
                        None => PyList::empty(py),
                    };
                    for dep_name in bin_deps {
                        external_depends_bin_set.insert(dep_name.extract()?);
                    }
                }
            }

            let external_depends_python: Vec<String> =
                external_depends_python_set.into_iter().collect();
            let external_depends_bin: Vec<String> = external_depends_bin_set.into_iter().collect();

            Ok(ManifestInfo {
                technical_name: module_name.into(),
                version_odoo,
                name,
                version_module,
                description,
                author,
                website,
                license,
                category,
                auto_install,
                application,
                installable,
                maintainer,
                git_org: org_name.into(),
                git_repo: repo_name.into(),
                depends,
                external_depends_python,
                external_depends_bin,
                folder_size: 0,
                last_commit_hash: String::new(),
                last_commit_author: String::new(),
                last_commit_name: String::new(),
                last_commit_date: String::new(),
                last_commit_partof: String::new(),
                committers: HashMap::new(),
            })
        })
    }

    pub fn get_module_info(
        &self,
        read_paths: &Vec<String>,
        repo_infos: &Vec<RepoInfo>,
    ) -> Vec<ManifestInfo> {
        let mut manifest_infos: Vec<ManifestInfo> = Vec::new();
        for repo_info in repo_infos {
            for read_path in read_paths {
                let base_path =
                    PathBuf::from(format!("{}{}", repo_info.get_clone_path(), read_path));
                log::info!("- Base Path: {}", &base_path.display());
                for entry in fs::read_dir(&base_path).unwrap() {
                    let path = entry.unwrap().path();
                    let manifest_filename_opt = self.is_odoo_module_folder(&path).unwrap();
                    if let Some(manifest_filename) = manifest_filename_opt {
                        let folder_size = get_size(&path).unwrap();
                        let git_info = self.get_git_info(&path).unwrap();
                        let committers = self.get_git_committers(&path).unwrap();
                        let manifest_path = format!("{}/{}", &path.display(), &manifest_filename);
                        let module_name = path.file_name().unwrap().to_str().unwrap();
                        let mut manifest = self
                            .read_manifest(
                                repo_info.get_org(),
                                repo_info.get_name(),
                                module_name,
                                &manifest_path,
                            )
                            .unwrap();
                        manifest.folder_size = folder_size;
                        manifest.last_commit_hash = git_info.last_commit_hash;
                        manifest.last_commit_author = git_info.last_commit_author;
                        manifest.last_commit_name = git_info.last_commit_name;
                        manifest.last_commit_date = git_info.last_commit_date;
                        manifest.last_commit_partof = git_info.last_commit_partof;
                        manifest.committers = committers;
                        manifest_infos.push(manifest);
                    }
                }
            }
        }
        manifest_infos
    }
}
