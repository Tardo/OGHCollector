// Copyright Alexandre D. Díaz
//! API-key gate of `/mcp`. A request needs `Authorization: Bearer ogh_<id>.<mac>` (see
//! `oghutils::mcp_key`); on top of the signature, GitLab is asked whether that user id is still an
//! active member of the group (`GET /groups/:group/members/all/:id`, read-only token), cached for
//! `OGHCOLLECTOR_MCP_GITLAB_CACHE` seconds (default 300): that is how a removed user loses access.
//! Any GitLab failure denies and logs an error (fail closed). Off unless the key secret is set;
//! with the secret set the GitLab settings are mandatory (a signature alone can't be revoked).
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

pub struct Gate {
    secret: String,
    gitlab: String,
    token: String,
    group: String,
    ttl: Duration,
    http: reqwest::Client,
    cache: Mutex<HashMap<String, (Instant, bool)>>,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl Gate {
    /// `None` = no key secret, the endpoint stays open. `Err` = secret without GitLab settings.
    pub fn from_env() -> Result<Option<Gate>, String> {
        let Some(secret) = oghutils::mcp_key::secret() else {
            return Ok(None);
        };
        let need = |name: &str| env(name).ok_or(format!("{name} is required with a key secret"));
        let token = env("OGHCOLLECTOR_MCP_GITLAB_TOKEN")
            .or_else(|| {
                std::fs::read_to_string("/run/secrets/mcp_gitlab_token")
                    .ok()
                    .map(|t| t.trim().to_string())
            })
            .filter(|t| !t.is_empty())
            .ok_or("OGHCOLLECTOR_MCP_GITLAB_TOKEN (or Docker secret mcp_gitlab_token) is required with a key secret")?;
        let ttl = env("OGHCOLLECTOR_MCP_GITLAB_CACHE")
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);
        Ok(Some(Gate {
            secret,
            gitlab: need("OGHCOLLECTOR_MCP_GITLAB_URL")?
                .trim_end_matches('/')
                .to_string(),
            token,
            group: need("OGHCOLLECTOR_MCP_GITLAB_GROUP")?,
            ttl: Duration::from_secs(ttl),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .map_err(|e| e.to_string())?,
            cache: Mutex::default(),
        }))
    }

    async fn is_member(&self, id: &str) -> bool {
        if let Some((at, ok)) = self.cache.lock().unwrap().get(id) {
            if at.elapsed() < self.ttl {
                return *ok;
            }
        }
        let url = format!(
            "{}/api/v4/groups/{}/members/all/{id}",
            self.gitlab,
            self.group.replace('/', "%2F")
        );
        let ok = match self
            .http
            .get(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r
                .json::<serde_json::Value>()
                .await
                .map(|v| v["state"] == "active")
                .unwrap_or(false),
            Ok(r) if r.status() == StatusCode::NOT_FOUND => false,
            Ok(r) => {
                log::error!("GitLab membership check for user {id}: HTTP {}", r.status());
                return false;
            }
            Err(e) => {
                log::error!("GitLab membership check for user {id}: {e}");
                return false;
            }
        };
        self.cache
            .lock()
            .unwrap()
            .insert(id.to_string(), (Instant::now(), ok));
        ok
    }
}

pub async fn require_key(
    State(gate): State<std::sync::Arc<Gate>>,
    req: Request,
    next: Next,
) -> Response {
    let id = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(|key| oghutils::mcp_key::verify(&gate.secret, key.trim()));
    match id {
        Some(id) if gate.is_member(&id).await => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "Missing, invalid or revoked API key",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::Path, routing::get, Json, Router};

    async fn gate_with_fake_gitlab() -> Gate {
        let app = Router::new().route(
            "/api/v4/groups/{g}/members/all/{id}",
            get(|Path((_, id)): Path<(String, String)>| async move {
                match id.as_str() {
                    "1" => (StatusCode::OK, Json(serde_json::json!({"state": "active"}))),
                    "4" => (
                        StatusCode::OK,
                        Json(serde_json::json!({"state": "blocked"})),
                    ),
                    "3" => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({})),
                    ),
                    _ => (StatusCode::NOT_FOUND, Json(serde_json::json!({}))),
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await });
        Gate {
            secret: "s".into(),
            gitlab: format!("http://{addr}"),
            token: "t".into(),
            group: "isonor".into(),
            ttl: Duration::from_secs(60),
            http: reqwest::Client::new(),
            cache: Mutex::default(),
        }
    }

    #[tokio::test]
    async fn only_active_members_pass_and_errors_deny() {
        let g = gate_with_fake_gitlab().await;
        assert!(g.is_member("1").await, "active member");
        assert!(!g.is_member("2").await, "not a member");
        assert!(!g.is_member("4").await, "blocked");
        assert!(!g.is_member("3").await, "GitLab error fails closed");
        assert!(
            g.cache.lock().unwrap().get("3").is_none(),
            "errors aren't cached"
        );
    }
}
