// Copyright 2025 Alexandre D. Díaz
#[derive(Debug)]
pub struct PypiClient {
    client: reqwest::Client,
}

impl PypiClient {
    pub fn new() -> Self {
        let client_result = reqwest::Client::builder().build();
        let client = match client_result {
            Ok(cl) => cl,
            Err(e) => panic!("Problem creating the client: {e:?}")
        };
        Self { client }
    }

    async fn request(&self, package: &str, version: Option<&str>) -> Result<reqwest::Response, reqwest::Error> {
        let full_url = if version.is_some() {
            let ver = version.unwrap();
            format!("https://pypi.python.org/pypi/{package}/{ver}/json")
        } else {
            format!("https://pypi.python.org/pypi/{package}/json")
        };
        let res = self.client
            .get(full_url)
            .header(reqwest::header::USER_AGENT, "OGHCollector")
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await?;
        Ok(res)
    }

    async fn request_json(&self, package: &str, version: Option<&str>) -> Result<serde_json::Value, reqwest::Error> {
        let req = self.request(package, version).await?;
        req.json().await
    }

    pub async fn get_package_info(&self, package: &str, version: Option<&str>) -> Result<serde_json::Value, reqwest::Error> {
        let res = self.request_json(package, version).await?;
        Ok(res)
    }

    pub async fn get_nearest_version(&self, package: &str, version: &str) -> Result<Option<String>, reqwest::Error> {
        let values = self.get_package_info(package, None).await?;
        let ver_orig_parts = version.split(".").collect::<Vec<&str>>().iter().map(|&x| x.parse::<i16>().unwrap_or(-1)).collect::<Vec<i16>>();
        let releases_opt = values["releases"].as_object();
        if releases_opt.is_none() {
            return Ok(None);
        }
        let releases = releases_opt.unwrap();
        let mut res: Option<String> = None;
        for (rel_ver, _) in releases {
            let ver_str = rel_ver;
            let ver_parts = ver_str.split(".").collect::<Vec<&str>>().iter().map(|&x| x.parse::<i16>().unwrap_or(-1)).collect::<Vec<i16>>();
            
            for (index, ver_part) in ver_parts.iter().enumerate() {
                if index >= ver_orig_parts.len() || ver_part > &ver_orig_parts[index]  {
                    break;
                }
                if *ver_part == -1 || ver_orig_parts[index] == -1 {
                    continue;
                }
                
                log::info!("Check if '{}' < '{}'...", &ver_part, &ver_orig_parts[index]);
                if ver_part < &ver_orig_parts[index] {
                    res = Some(ver_str.to_string());
                }
            }
        }
        Ok(res)
    }
}
