// Copyright Alexandre D. Díaz
use duct::cmd;
use regex::Regex;
use std::fs;
use std::path::Path;

pub struct RepoInfo {
    pub name: String,
    pub org: String,
    pub clone_path: String,
    pub full_path: String,
}

impl RepoInfo {
    pub fn get_name(&self) -> &str {
        &self.name
    }
    pub fn get_org(&self) -> &str {
        &self.org
    }
    pub fn get_clone_path(&self) -> &str {
        &self.clone_path
    }
    pub fn get_full_path(&self) -> &str {
        &self.full_path
    }
}

#[derive(Debug, Clone)]
pub struct PullRequestInfo {
    pub number: i64,
    pub title: String,
    pub module_technical_name: String,
    /// PR/MR creation date, in sqlite text format (`%Y-%m-%d %H:%M:%S`),
    /// `None` if the provider's `created_at` was missing/unparseable.
    pub created_at: Option<String>,
    /// Last-activity date, in the same format. Providers only expose this for
    /// free as the PR/MR's own `updated_at` (bumped by commits, comments,
    /// label changes, etc.) in the same list response already fetched here -
    /// a real "last comment" date would need one extra API call per PR, so
    /// this is used as the closest available proxy for "last message".
    pub last_message_at: Option<String>,
    /// Normalized CI signal: `Some("success")`, `Some("pending")`,
    /// `Some("failure")`, or `None` when no CI is configured for the head
    /// commit / the provider reported nothing usable.
    pub ci_status: Option<String>,
}

/// Parses a provider's RFC3339 `created_at` into the sqlite text format
/// used to store dates in this project (see `sqlitedb::utils::date`).
pub fn parse_created_at(raw: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| sqlitedb::utils::date::to_sqlite_datetime(dt.with_timezone(&chrono::Utc)))
}

/// OCA migration convention: head branch `{version}-mig-{module_technical_name}`,
/// optionally followed by a `-{discard}` suffix (e.g. `16.0-mig-sale_commission-wip`).
/// Returns the module technical name if it matches.
///
/// Odoo technical names never contain `-`, only `_`. So if the captured tail
/// already has an `_` in it, it's a real name with one discard segment tacked
/// on the end - drop everything after the last `-`. If it has no `_` at all,
/// the whole thing was typed with `-` instead of `_` and there's no way to
/// tell a discard suffix apart from the name - just normalize every `-` to `_`.
/// ponytail: ambiguous when both a hyphenated name AND a discard suffix are
/// present (e.g. `sale-order-line-wip`) - the discard ends up folded into the
/// name. Add a real module-name lookup if this proves common in practice.
pub fn extract_migration_module_name(head_ref: &str) -> Option<String> {
    let re = Regex::new(r"^[0-9]+\.[0-9]+-mig-(.+)$").unwrap();
    re.captures(head_ref).and_then(|caps| caps.get(1)).map(|m| {
        let raw = m.as_str();
        if raw.contains('_') {
            raw.rsplit_once('-')
                .map_or(raw, |(name, _discard)| name)
                .to_string()
        } else {
            raw.replace('-', "_")
        }
    })
}

pub trait GitClient {
    fn new(token: &str, base_url: &str) -> Self;

    fn clone_or_update_repo(
        &self,
        org_name: &str,
        repo_name: &str,
        repo_url: &str,
        branch: &str,
        dest: &str,
    ) -> Option<RepoInfo> {
        let clone_path = format!("{dest}/{org_name}/{repo_name}");
        let clone_path_exists = Path::new(&clone_path).exists();
        if clone_path_exists {
            log::info!("Updating repo: {repo_name} @ {branch}");
            cmd!("git", "fetch", "origin", "--prune")
                .dir(&clone_path)
                .stdin_null()
                .run()
                .ok()?;
            cmd!("git", "reset", "--hard", &format!("origin/{branch}"))
                .dir(&clone_path)
                .stdin_null()
                .run()
                .ok()?;
            cmd!("git", "clean", "-fdx")
                .dir(&clone_path)
                .stdin_null()
                .run()
                .ok()?;
            cmd!("git", "switch", "-C", branch, &format!("origin/{branch}"))
                .dir(&clone_path)
                .stdin_null()
                .run()
                .ok()?;
            log::info!("Repo updated & cleaned: {repo_name} @ {branch}");
        } else {
            log::info!("Cloning repo: {repo_name} @ {branch}");
            let base_dir = format!("{dest}/{org_name}");
            if fs::create_dir_all(&base_dir).is_err() {
                log::error!("Cannot create directory: {base_dir}");
                return None;
            }

            cmd!(
                "git",
                "clone",
                "--no-single-branch",
                "--branch",
                branch,
                repo_url,
            )
            .dir(&base_dir)
            .stdin_null()
            .run()
            .ok()?;
        }
        Some(RepoInfo {
            name: repo_name.into(),
            org: org_name.into(),
            clone_path,
            full_path: format!("{org_name}/{repo_name}"),
        })
    }

    async fn request(&self, url: &str) -> Result<reqwest::Response, reqwest::Error>;

    async fn request_json(&self, url: &str) -> Result<serde_json::Value, reqwest::Error> {
        let req = self.request(url).await?;
        req.json().await
    }

    async fn get_org_repos(
        &self,
        org_name: &str,
        per_page: &usize,
        page: &usize,
    ) -> Result<serde_json::Value, reqwest::Error>;

    async fn clone_org_repos(&self, org_name: &str, branch: &str, dest: &str) -> Vec<RepoInfo>;

    async fn get_repo_pull_requests(
        &self,
        full_path: &str,
        branch: &str,
        per_page: &usize,
        page: &usize,
    ) -> Result<serde_json::Value, reqwest::Error>;

    async fn get_open_migration_pull_requests(
        &self,
        full_path: &str,
        branch: &str,
    ) -> Vec<PullRequestInfo>;

    /// Whether a PR/MR that's no longer open was actually merged - one extra
    /// GET per newly-closed PR, called right before `delete_outdated` so the
    /// "avg. time open before close" stat only counts merged PRs, not rejections.
    /// `None` on a lookup failure (network/rate-limit/404): treated as "not merged"
    /// by the caller, so a transient error undercounts rather than pollutes.
    async fn is_pull_request_merged(&self, full_path: &str, number: &i64) -> Option<bool>;
}

#[cfg(test)]
mod tests {
    use super::extract_migration_module_name;

    #[test]
    fn test_extract_migration_module_name_matches_oca_convention() {
        assert_eq!(
            extract_migration_module_name("16.0-mig-sale_commission"),
            Some("sale_commission".to_string())
        );
        assert_eq!(
            extract_migration_module_name("17.0-mig-account_analytic_tag_taxonomies"),
            Some("account_analytic_tag_taxonomies".to_string())
        );
    }

    #[test]
    fn test_extract_migration_module_name_rejects_non_migration_branches() {
        assert_eq!(extract_migration_module_name("16.0-fix-foo"), None);
        assert_eq!(extract_migration_module_name("master"), None);
        assert_eq!(extract_migration_module_name("16.0-mig-"), None);
        assert_eq!(extract_migration_module_name("mig-sale_commission"), None);
    }

    #[test]
    fn test_extract_migration_module_name_drops_discard_suffix() {
        assert_eq!(
            extract_migration_module_name("16.0-mig-nombre_modulo_algo-BAD"),
            Some("nombre_modulo_algo".to_string())
        );
    }

    #[test]
    fn test_extract_migration_module_name_normalizes_hyphenated_name() {
        assert_eq!(
            extract_migration_module_name("16.0-mig-nombre-modulo-algo"),
            Some("nombre_modulo_algo".to_string())
        );
    }

    #[test]
    fn test_extract_migration_module_name_ambiguous_hyphenated_with_discard() {
        assert_eq!(
            extract_migration_module_name("16.0-mig-nombre-modulo-algo-BAD"),
            Some("nombre_modulo_algo_BAD".to_string())
        );
    }
}
