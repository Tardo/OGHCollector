// Copyright Alexandre D. Díaz
//! Web links of a collected repository. The collector only speaks GitHub
//! (github.com) and GitLab (any host), so a host other than github.com is
//! GitLab, whose web routes live under `/-/`.
//!
//! Repo web links from the stored clone URL: `clone_url` (github.com fallback), `files_url` (append
//! `/blob|tree|raw/{branch}/{path}`; non-github.com hosts are GitLab → `/-/`), `pull_request_url`
//! (`/pull/N` vs `/-/merge_requests/N`). Also `pull_request_ref` (`refs/pull/N/head` vs
//! `refs/merge-requests/N/head`, doodba merges). The server exposes `files_url` as the `repo_files`
//! template filter; pages listing many repos load `sqlitedb::models::gh_repository::clone_urls`
//! once (`org/repo` → stored URL) and pass `.get()` to `clone_url`. Doodba JSON carries `repo_urls`
//! (repo name → clone URL) and `merge_ref` on pending PRs for the JS `repos.yaml`. GitLab repos are
//! stored by project `path` (slug), not display `name`.

/// Clone URL of a repository: the one the collector stored, else the
/// github.com one (rows collected before the URL was stored).
pub fn clone_url(stored: Option<&str>, org: &str, repo: &str) -> String {
    match stored {
        Some(url) if !url.is_empty() => url.to_string(),
        _ => format!("https://github.com/{org}/{repo}.git"),
    }
}

fn web_url(clone_url: &str) -> &str {
    clone_url.trim_end_matches('/').trim_end_matches(".git")
}

fn is_github(clone_url: &str) -> bool {
    let host = clone_url.split_once("://").map_or("", |(_, rest)| rest);
    let host = host.split('/').next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com")
}

/// Prefix for file routes: append `/blob|tree|raw/{branch}/{path}`.
pub fn files_url(clone_url: &str) -> String {
    if is_github(clone_url) {
        web_url(clone_url).to_string()
    } else {
        format!("{}/-", web_url(clone_url))
    }
}

/// Link to a pull request (GitHub) / merge request `iid` (GitLab).
pub fn pull_request_url(clone_url: &str, number: i64) -> String {
    if is_github(clone_url) {
        format!("{}/pull/{number}", web_url(clone_url))
    } else {
        format!("{}/-/merge_requests/{number}", web_url(clone_url))
    }
}

/// Git ref a PR/MR head can be fetched from (doodba `repos.yaml` merges).
pub fn pull_request_ref(clone_url: &str, number: i64) -> String {
    if is_github(clone_url) {
        format!("refs/pull/{number}/head")
    } else {
        format!("refs/merge-requests/{number}/head")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_github_and_gitlab_links() {
        let gh = clone_url(None, "OCA", "web");
        assert_eq!(gh, "https://github.com/OCA/web.git");
        assert_eq!(files_url(&gh), "https://github.com/OCA/web");
        assert_eq!(
            pull_request_url(&gh, 7),
            "https://github.com/OCA/web/pull/7"
        );
        let gl = clone_url(
            Some("https://git.acme.io/erp/addons/sales.git"),
            "erp",
            "sales",
        );
        assert_eq!(files_url(&gl), "https://git.acme.io/erp/addons/sales/-");
        assert_eq!(
            pull_request_url(&gl, 3),
            "https://git.acme.io/erp/addons/sales/-/merge_requests/3"
        );
        // A GitLab host that merely contains "github.com" is still GitLab.
        assert!(files_url("https://github.com.acme.io/a/b.git").ends_with("/-"));
        assert_eq!(clone_url(Some(""), "OCA", "web"), gh);
        assert_eq!(pull_request_ref(&gh, 7), "refs/pull/7/head");
        assert_eq!(pull_request_ref(&gl, 3), "refs/merge-requests/3/head");
    }
}
