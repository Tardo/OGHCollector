mod config;
mod minijinja_renderer;
mod routes;
mod utils;
mod middlewares;
mod state;
mod scheduler;

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use actix_cors::Cors;
use actix_files as afs;
use actix_web::{
    http::{header, StatusCode},
    middleware::{ErrorHandlers, Logger},
    web, 
    App, 
    HttpServer
};
use actix_session::{SessionMiddleware, storage::CookieSessionStore};
use actix_web::cookie::Key;
use r2d2_sqlite::{self, SqliteConnectionManager};
use minijinja::path_loader;
use minijinja_autoreload::AutoReloader;

use sqlitedb::{Pool, models};
use middlewares::not_found;
use config::SERVER_CONFIG;
use state::AppState;


#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    // MiniJinja
    if SERVER_CONFIG.get_template_autoreload() {
        log::info!("template auto-reloading is enabled");
    } else {
        log::info!(
            "template auto-reloading is disabled; run with TEMPLATE_AUTORELOAD=true to enable"
        );
    }

    // Secret Key
    let cookie_secret_key;
    if SERVER_CONFIG.get_cookie_key_bytes().len() < 64 {
        cookie_secret_key = Key::generate();
    } else {
        cookie_secret_key = Key::from(&SERVER_CONFIG.get_cookie_key_bytes());
    }

    // The closure is invoked every time the environment is outdated to recreate it.
    let tmpl_reloader = AutoReloader::new(move |notifier| {
        let mut env: minijinja::Environment<'static> = minijinja::Environment::new();

        let tmpl_path = PathBuf::from("./web/templates");

        // if watch_path is never called, no fs watcher is created
        if SERVER_CONFIG.get_template_autoreload() {
            notifier.watch_path(&tmpl_path, true);
        }

        env.set_loader(path_loader(tmpl_path));

        Ok(env)
    });
    let tmpl_reloader = web::Data::new(tmpl_reloader);

    // App State
    let app_state = AppState { page_name: "index".to_string() };

    // connect to SQLite DB
    let db_path = "data/data.db";
    if let Some(parent) = Path::new(db_path).parent() {
        fs::create_dir_all(parent)?;
    }
    if !Path::new(db_path).exists() {
        File::create(db_path)?;
    }
    let manager = SqliteConnectionManager::file(db_path);
    let pool = Pool::new(manager).unwrap();
    let conn = pool.get().unwrap();
    models::prepare_schema(&conn).expect("Can't create the database");

    // Start scheduler on a new thread
    // actix_rt::spawn(async move {
    //     start_scheduler().await;
    // });

    log::info!("starting HTTP server at http://{}:{}", &SERVER_CONFIG.get_bind_address(), &SERVER_CONFIG.get_port());

    // start HTTP server
    HttpServer::new(move || {
        let cors = Cors::default()
            .allowed_origin_fn(|origin, _req_head| SERVER_CONFIG.is_allowed_origin(origin.to_str().unwrap_or("")))
            .allowed_methods(vec!["GET", "POST"])
            .allowed_headers(vec![
                header::CONTENT_TYPE,
                header::ACCEPT,
            ])
            .max_age(3600);

        App::new()
            // store db pool as Data object
            .app_data(web::Data::new(pool.clone()))
            .app_data(tmpl_reloader.clone())
            .app_data(web::Data::new(app_state.clone()))
            .service(afs::Files::new("/static", "./static").show_files_listing())
            .service(routes::common::route_odoo_versions)
            .service(routes::common::route_odoo_module_count)
            .service(routes::common::route_odoo_contributor_rank)
            .service(routes::common::route_odoo_committer_rank)
            .service(routes::common::route_doodba_addons)
            .service(routes::dashboard::route)
            .service(routes::api_doc::route)
            .service(routes::logs::route)
            .service(routes::osv::route)
            .service(routes::doodba_converter::route)
            .service(routes::atlas::route)
            .service(routes::atlas::route_atlas_data)
            .service(
                web::scope(&routes::api::v1::PATH)
                    .service(routes::api::v1::module::route)
                    .service(routes::api::v1::module::route_odoo_version)
                    .service(routes::api::v1::repository::route)
                    .service(routes::api::v1::search::route)
            )
            .wrap(
                SessionMiddleware::new(
                    CookieSessionStore::default(),
                    cookie_secret_key.clone()
                )
            )
            .wrap(cors)
            .wrap(ErrorHandlers::new().handler(StatusCode::NOT_FOUND, not_found::handler_fn))
            .wrap(Logger::default())
    })
    .bind((SERVER_CONFIG.get_bind_address().clone(), SERVER_CONFIG.get_port().clone()))?
    .workers(SERVER_CONFIG.get_workers().clone())
    .run()
    .await
}
