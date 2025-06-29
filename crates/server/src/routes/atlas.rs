use cached::proc_macro::cached;
use minijinja::context;
use actix_web::{web, get, HttpRequest, HttpResponse, Responder, Result, Error as AWError};
use oghutils::version::odoo_version_string_to_u8;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::utils::get_minijinja_context;
use crate::minijinja_renderer::MiniJinjaRenderer;

use sqlitedb::{Pool, models::{self, Connection}};


#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GraphNodeInfo {
    pub key: String,
    pub attributes: HashMap<String, String>,
}
impl GraphNodeInfo {
    fn update(&mut self, key: &str, value: &str) {
        self.attributes.insert(key.to_string(), value.to_string());
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GraphEdgeInfo {
    pub key: String,
    pub source: String,
    pub target: String,
    pub undirected: bool,
    pub attributes: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GraphInfo {
    pub attributes: HashMap<String, String>,
    pub nodes: Vec<GraphNodeInfo>,
    pub edges: Vec<GraphEdgeInfo>,
}

fn set_main_node_attributes(module: &models::module::Model, gh_repo_odoo_id: &i64, node_info: &mut GraphNodeInfo) {
    if module.gh_repository_id.0.eq(gh_repo_odoo_id) {
        if module.application {
            node_info.update("size".into(), "12".into());
            node_info.update("color".into(), "#21B799".into());
        } else {
            node_info.update("size".into(), "10".into());
            node_info.update("color".into(), "#017E84".into());
        }
    } else if module.application {
        node_info.update("size".into(), "9".into());
        node_info.update("color".into(), "#21B799".into());
    } else {
        node_info.update("size".into(), "8".into());
        node_info.update("color".into(), "#E46E78".into());
    }
    node_info.update("label".into(), &module.technical_name);
    node_info.update("repository".into(), &module.gh_repository_id.1);
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("{}", odoo_version) }"#
)]
fn get_graph_data(conn: &Connection, odoo_version: &u8) -> GraphInfo {
    let mut graph_info = GraphInfo {
        attributes: HashMap::new(),
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    let mut gh_repo_odoo_id = 0;
    let gh_org_odoo_opt = models::gh_organization::get_by_name(&conn, "odoo");
    if gh_org_odoo_opt.is_some() {
        let gh_org_odoo = gh_org_odoo_opt.unwrap();
        let gh_repo_odoo_opt = models::gh_repository::get_by_name(&conn, &gh_org_odoo.id, "odoo");
        if gh_repo_odoo_opt.is_some() {
            let gh_repo_odoo = gh_repo_odoo_opt.unwrap();
            gh_repo_odoo_id = gh_repo_odoo.id;
        }
    }
    let main_modules: Vec<models::module::Model> = models::module::get_by_odoo_version(&conn, &odoo_version);
    let main_modules_names: Vec<String> = main_modules.iter().map(|item| item.technical_name.clone()).collect();
    for module in main_modules {
        if module.technical_name == "base" {
            continue
        }
        let module_depends_list: Vec<String> = models::dependency::get_module_external_dependency_names(&conn, &module.id, "module");
        for mod_dep_name in module_depends_list {
            if mod_dep_name == "base" {
                continue
            }
            let node_key: String = format!("o_{}", &mod_dep_name);
            if !main_modules_names.contains(&mod_dep_name) && graph_info.nodes.iter().find(|x| x.key.eq(&node_key)).is_none() {
                let mut node_info = GraphNodeInfo {
                    key: node_key.clone(),
                    attributes: HashMap::new(),
                };
                node_info.attributes.insert("size".into(), "8".into());
                node_info.attributes.insert("color".into(), "#E46E78".into());
                node_info.attributes.insert("label".into(), mod_dep_name.clone());
                graph_info.nodes.push(node_info);
            }
            let edge_key = format!("o_{}__{}", &module.technical_name, &mod_dep_name);
            if graph_info.edges.iter().find(|x| x.key.eq(&edge_key)).is_none() {
                let mut edge_info = GraphEdgeInfo {
                    key: edge_key,
                    source: format!("o_{}", &module.technical_name),
                    target: node_key.clone(),
                    undirected: false,
                    attributes: HashMap::new(),
                };
                edge_info.attributes.insert("size".into(), "2".into());
                graph_info.edges.push(edge_info);
            }
        }
        let pip_depends_list: Vec<String> = models::dependency::get_module_external_dependency_names(&conn, &module.id, "python");
        for pip_dep_name in pip_depends_list {
            let node_key: String = format!("p_{}", &pip_dep_name);
            if graph_info.nodes.iter().find(|x| x.key.eq(&node_key)).is_none() {
                let mut node_info = GraphNodeInfo {
                    key: node_key.clone(),
                    attributes: HashMap::new(),
                };
                node_info.attributes.insert("size".into(), "5".into());
                node_info.attributes.insert("color".into(), "#6c5148".into());
                node_info.attributes.insert("label".into(), pip_dep_name.clone());
                graph_info.nodes.push(node_info);
            }
            let mut edge_info = GraphEdgeInfo {
                key: format!("p_{}__{}", &module.technical_name, &pip_dep_name),
                source: format!("o_{}", &module.technical_name),
                target: node_key.clone(),
                undirected: false,
                attributes: HashMap::new(),
            };
            edge_info.attributes.insert("size".into(), "2".into());
            graph_info.edges.push(edge_info);
        }
        let bin_depends_list: Vec<String> = models::dependency::get_module_external_dependency_names(&conn, &module.id, "bin");   
        for bin_dep_name in bin_depends_list {
            let node_key: String = format!("b_{}", &bin_dep_name);
            if graph_info.nodes.iter().find(|x| x.key.eq(&node_key)).is_none() {
                let mut node_info = GraphNodeInfo {
                    key: node_key.clone(),
                    attributes: HashMap::new(),
                };
                node_info.attributes.insert("size".into(), "5".into());
                node_info.attributes.insert("color".into(), "#335548".into());
                node_info.attributes.insert("label".into(), bin_dep_name.clone());
                graph_info.nodes.push(node_info);
            }
            let mut edge_info = GraphEdgeInfo {
                key: format!("b_{}__{}", &module.technical_name, &bin_dep_name),
                source: format!("o_{}", &module.technical_name),
                target: node_key.clone(),
                undirected: false,
                attributes: HashMap::new(),
            };
            edge_info.attributes.insert("size".into(), "2".into());
            graph_info.edges.push(edge_info);
        }

        let node_key = format!("o_{}", &module.technical_name);
        let cur_node_info_opt = graph_info.nodes.iter().position(|x| x.key.eq(&node_key));
        let mut node_info = GraphNodeInfo {
            key: node_key,
            attributes: HashMap::new(),
        };
        set_main_node_attributes(&module, &gh_repo_odoo_id, &mut node_info);
        if cur_node_info_opt.is_some() {
            graph_info.nodes.remove(cur_node_info_opt.unwrap());
        }
        graph_info.nodes.push(node_info);
    }
    graph_info
}

#[get("/atlas/data/{odoo_version}")]
pub async fn route_atlas_data(pool: web::Data<Pool>, path: web::Path<String>) -> Result<HttpResponse, AWError> {
    let conn = web::block(move || pool.get())
        .await?.unwrap();
    let odoo_version = path.into_inner();
    let result = web::block(move || {
        get_graph_data(&conn, &odoo_version_string_to_u8(&odoo_version))
    }).await?;
    return Ok(HttpResponse::Ok().json(result));
}

#[get("/atlas")]
pub async fn route(tmpl_env: MiniJinjaRenderer, req: HttpRequest) -> Result<impl Responder> {    
    return tmpl_env.render("pages/atlas.html", context!(
        ..get_minijinja_context(&req),
        ..context!(
            page_name => "atlas",
        )
    ))
}