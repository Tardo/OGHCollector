// Copyright Alexandre D. Díaz
use config::Config;
use lazy_static::lazy_static;
use url::Url;

#[derive(Debug)]
pub struct OGHServerConfig {
    bind_address: String,
    port: u16,
    workers: usize,
    template_autoreload: bool,
    allowed_origins: Vec<Url>,
    cookie_key_bytes: Vec<u8>,
    upload_limit: usize,
    cache_ttl: u64,
    db_pool_max_size: u32,
}

impl OGHServerConfig {
    pub fn new() -> OGHServerConfig {
        let settings = Config::builder()
            .add_source(config::File::with_name("./server").required(false))
            .add_source(config::Environment::with_prefix("OGHCOLLECTOR_"))
            .build()
            .unwrap();

        let bind_address = settings
            .get_string("bind_address")
            .unwrap_or("0.0.0.0".to_string());
        let port = settings.get_int("port").unwrap_or(8080) as u16;
        let workers = settings.get_int("workers").unwrap_or(2) as usize;
        let template_autoreload = settings.get_bool("template_autoreload").unwrap_or(false);
        let allowed_origins = settings
            .get_array("allowed_origins")
            .unwrap_or_else(|_| Vec::new())
            .iter()
            .map(|x| Url::parse(&x.to_string()).unwrap())
            .collect::<Vec<Url>>();
        let cookie_key = settings.get_string("cookie_key").unwrap_or_default();
        let cookie_key_bytes = cookie_key.into_bytes();
        let upload_limit = settings.get_int("upload_limit").unwrap_or(2 * 1024 * 1024) as usize;
        let cache_ttl = settings.get_int("cache_ttl").unwrap_or(3600) as u64;
        let db_pool_max_size = settings.get_int("db_pool_max_size").unwrap_or(15) as u32;
        OGHServerConfig {
            bind_address,
            port,
            workers,
            template_autoreload,
            allowed_origins,
            cookie_key_bytes,
            upload_limit,
            cache_ttl,
            db_pool_max_size,
        }
    }

    pub fn get_bind_address(&self) -> &String {
        &self.bind_address
    }

    pub fn get_port(&self) -> &u16 {
        &self.port
    }

    pub fn get_workers(&self) -> &usize {
        &self.workers
    }

    pub fn get_template_autoreload(&self) -> bool {
        self.template_autoreload
    }

    pub fn get_allowed_origins(&self) -> &Vec<Url> {
        &self.allowed_origins
    }
    pub fn is_allowed_origin(&self, origin: &str) -> bool {
        if self.get_allowed_origins().is_empty() {
            return true;
        }
        let url = Url::parse(origin).unwrap();
        for origin_url in self.get_allowed_origins() {
            if origin_url.scheme() == url.scheme()
                && origin_url.domain() == url.domain()
                && origin_url.port() == url.port()
            {
                return true;
            }
        }
        false
    }

    pub fn get_cookie_key_bytes(&self) -> &Vec<u8> {
        &self.cookie_key_bytes
    }

    pub fn get_upload_limit(&self) -> &usize {
        &self.upload_limit
    }

    pub fn get_cache_ttl(&self) -> &u64 {
        &self.cache_ttl
    }

    pub fn get_db_pool_max_size(&self) -> &u32 {
        &self.db_pool_max_size
    }
}

lazy_static! {
    pub static ref SERVER_CONFIG: OGHServerConfig = OGHServerConfig::new();
}
