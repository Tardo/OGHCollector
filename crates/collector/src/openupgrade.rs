// Copyright Alexandre D. Díaz
//! The module renames/merges OpenUpgrade records for an Odoo version: the
//! `renamed_modules` / `merged_modules` of its `apriori.py` (a dict, or a
//! list of pairs before 11.0), core and OCA alike. Lets `/migration` and the
//! Overview tell "merged into hr" from "not started". The file is parsed
//! with `ast.literal_eval`, never run. Fetched once per job for the job's
//! version and stored in `openupgrade_rename`. A failure only warns and
//! keeps the stored rows, because the modules were still collected. A branch
//! OpenUpgrade doesn't have (6.1, a version it hasn't started) has no data.
use pyo3::prelude::*;
use sqlitedb::DbSqliteConnection as SqliteConnection;

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::models;

fn apriori_url(version_odoo: u8) -> String {
    let path = match version_odoo {
        ..=90 => "openerp/addons/openupgrade_records/lib/apriori.py",
        91..=130 => "odoo/addons/openupgrade_records/lib/apriori.py",
        _ => "openupgrade_scripts/apriori.py",
    };
    format!(
        "https://raw.githubusercontent.com/OCA/OpenUpgrade/{}/{path}",
        odoo_version_u8_to_string(&version_odoo)
    )
}

/// (old name, new name, merged into an existing module).
pub(crate) fn parse_apriori(src: &str) -> Result<Vec<(String, String, bool)>, String> {
    const SRC: &std::ffi::CStr = c"import ast, json
def load(src):
    out = []
    for node in ast.parse(src).body:
        if not (isinstance(node, ast.Assign) and len(node.targets) == 1):
            continue
        name = getattr(node.targets[0], 'id', None)
        if name not in ('renamed_modules', 'merged_modules'):
            continue
        value = ast.literal_eval(node.value)
        pairs = value.items() if isinstance(value, dict) else value
        out += [[str(old), str(new), name == 'merged_modules'] for old, new in pairs]
    return json.dumps(out)
";
    let json: String = Python::attach(|py| -> PyResult<String> {
        let module = PyModule::from_code(py, SRC, c"ogh_apriori.py", c"ogh_apriori")?;
        module.getattr("load")?.call1((src,))?.extract()
    })
    .map_err(|err| err.to_string())?;
    serde_json::from_str(&json).map_err(|err| err.to_string())
}

pub async fn sync(conn: &mut SqliteConnection, version_odoo: u8) {
    let url = apriori_url(version_odoo);
    let res = crate::gitclient::http_client()
        .get(&url)
        .header(reqwest::header::USER_AGENT, "OGHCollector")
        .send()
        .await;
    let body = match res {
        Ok(res) if res.status() == reqwest::StatusCode::NOT_FOUND => {
            log::info!("OpenUpgrade has no renames for this version ({url})");
            return;
        }
        Ok(res) => match res.error_for_status() {
            Ok(res) => res.text().await,
            Err(err) => Err(err),
        },
        Err(err) => Err(err),
    };
    let renames = match body
        .map_err(|e| e.to_string())
        .and_then(|b| parse_apriori(&b))
    {
        Ok(renames) => renames,
        Err(err) => {
            log::warn!(
                "Can't read OpenUpgrade's module renames ({url}), keeping the stored ones: {err}"
            );
            return;
        }
    };
    log::info!("OpenUpgrade: {} module renames/merges", renames.len());
    if let Err(err) =
        models::openupgrade_rename::replace_for_version(conn, version_odoo as i32, &renames)
    {
        log::error!("Can't store OpenUpgrade's module renames: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_apriori_formats_parse_without_running_them() {
        let src = r#"
import os  # never imported: the file is only parsed
renamed_modules = {
    # OCA/web
    "web_old": 'web_new',
    'long_one':
        "long_new",
}
merged_modules = [
    ('mail_tip', 'mail'),
]
renamed_models = {"a.b": "c.d"}
"#;
        assert_eq!(
            parse_apriori(src).unwrap(),
            [
                ("web_old".into(), "web_new".into(), false),
                ("long_one".into(), "long_new".into(), false),
                ("mail_tip".into(), "mail".into(), true),
            ]
        );
        assert!(parse_apriori("merged_modules = {os.getcwd(): 1}").is_err());
        assert!(apriori_url(180).ends_with("/18.0/openupgrade_scripts/apriori.py"));
        assert!(apriori_url(120).contains("/12.0/odoo/addons/"));
        assert!(apriori_url(80).contains("/8.0/openerp/addons/"));
    }
}
