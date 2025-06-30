// Copyright 2025 Alexandre D. Díaz
use lazy_static::lazy_static;
use config::Config;
use url::Url;
use chrono_tz::Tz;


#[derive(Debug)]
pub struct OGHServerConfig {
    bind_address: String,
    port: u16,
    workers: usize,
    template_autoreload: bool,
    static_autoreload: bool,
    allowed_origins: Vec<Url>,
    scheduler_time: Vec<u8>,
    timezone: Tz,
    cookie_key_bytes: Vec<u8>,
}

impl OGHServerConfig {
    pub fn new() -> OGHServerConfig {
        let settings = Config::builder()
        .add_source(config::File::with_name("./server").required(false))
        .add_source(config::Environment::with_prefix("OGHCOLLECTOR_"))
        .build()
        .unwrap();

        let bind_address = settings.get_string("bind_address").unwrap_or("0.0.0.0".to_string());
        let port = settings.get_int("port").unwrap_or(8080) as u16;
        let workers = settings.get_int("workers").unwrap_or(2) as usize;
        let template_autoreload = settings.get_bool("template_autoreload").unwrap_or(false);
        let static_autoreload = settings.get_bool("static_autoreload").unwrap_or(false);
        let allowed_origins = settings.get_array("allowed_origins").unwrap_or_else(|_| Vec::new()).iter().map(|x| Url::parse(&x.to_string()).unwrap()).collect::<Vec<Url>>();
        let scheduler_time_str = settings.get_string("scheduler_time").unwrap_or("00:00".to_string());
        let timezone = settings.get_string("timezone").unwrap_or("UTC".to_string());
        let timezone_tz: Tz = timezone.parse().unwrap();
        let mut scheduler_time: Vec<u8> = scheduler_time_str.split(":").map(|x| x.parse::<u8>().unwrap()).collect::<Vec<u8>>();
        if scheduler_time.len() < 2 {
            scheduler_time.push(0);
        }
        let cookie_key = settings.get_string("cookie_key").unwrap_or(String::new());
        let cookie_key_bytes = cookie_key.into_bytes();
        OGHServerConfig { bind_address, port, workers, template_autoreload, static_autoreload, allowed_origins, scheduler_time, timezone: timezone_tz, cookie_key_bytes }
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

    pub fn get_static_autoreload(&self) -> bool {
        self.static_autoreload
    }

    pub fn get_allowed_origins(&self) -> &Vec<Url> {
        &self.allowed_origins
    }
    pub fn is_allowed_origin(&self, origin: &str) -> bool {
        if self.allowed_origins.is_empty() {
            return true;
        }
        let url = Url::parse(origin).unwrap();
        for origin_url in &self.allowed_origins {
            if origin_url.scheme() == url.scheme() && origin_url.domain() == url.domain() && origin_url.port() == url.port() {
                return true;
            }
        }
        false
    }

    pub fn get_timezone(&self) -> &Tz {
        &self.timezone
    }

    pub fn get_scheduler_time(&self) -> &Vec<u8> {
        &self.scheduler_time
    }

    pub fn get_cookie_key_bytes(&self) -> &Vec<u8> {
        &self.cookie_key_bytes
    }
}

lazy_static! {
    pub static ref SERVER_CONFIG: OGHServerConfig = OGHServerConfig::new();
}
