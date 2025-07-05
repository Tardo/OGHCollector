pub fn odoo_version_u8_to_string(version: &u8) -> String {
    let float_ver: f32 = (*version as f32) / 10.0;
    format!("{float_ver:.1}")
}

pub fn odoo_version_string_to_u8(version: &str) -> u8 {
    (version.parse::<f32>().unwrap() * 10.0) as u8
}

pub struct OdooVersion {
    raw: String,
    version_odoo: u8,
    version_module: String,
}

impl OdooVersion {
    pub fn new(version: &str, def_version: &u8) -> OdooVersion {
        let version_odoo: u8;
        let version_module: String;
        let ver_parts = version.split(".").collect::<Vec<&str>>();
        if ver_parts.len() > 3 {
            version_odoo = odoo_version_string_to_u8(&ver_parts[..2].join("."));
            version_module = ver_parts[2..].join(".");
        } else {
            version_odoo = *def_version;
            version_module = version.to_string();
        }
        OdooVersion {
            raw: version.to_string(),
            version_odoo,
            version_module,
        }
    }

    pub fn get_raw(&self) -> &String {
        &self.raw
    }

    pub fn get_version_odoo(&self) -> &u8 {
        &self.version_odoo
    }

    pub fn get_version_module(&self) -> &String {
        &self.version_module
    }
}
