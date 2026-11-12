// Copyright Alexandre D. Díaz
//! MCP API keys: `ogh_<id>.<mac>`, `id` = the user's numeric GitLab id and `mac` = base64url
//! HMAC-SHA256 of it under a secret shared by `server` (mints, on the `/mcp` page) and `mcp`
//! (verifies). Stateless: nothing is stored, the key only says *who*. That the user is still
//! active and in the group is checked by `mcp` against GitLab on use, so removing someone there
//! revokes their key; rotating the secret revokes every key.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use hmac::{Hmac, Mac};
use sha2::Sha256;

const PREFIX: &str = "ogh_";
const DOMAIN: &[u8] = b"ogh-mcp-key:v1:";

/// Shared secret: `OGHCOLLECTOR_MCP_KEY_SECRET`, else the Docker secret `mcp_key_secret`.
/// `None` = keys disabled.
pub fn secret() -> Option<String> {
    let s = std::env::var("OGHCOLLECTOR_MCP_KEY_SECRET")
        .ok()
        .or_else(|| std::fs::read_to_string("/run/secrets/mcp_key_secret").ok())?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn mac(secret: &str, id: &str) -> Hmac<Sha256> {
    let mut m = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key size");
    m.update(DOMAIN);
    m.update(id.as_bytes());
    m
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}

/// The key of a GitLab user id; `None` when `id` isn't a plain number (the proxy must send the
/// OIDC `sub`, see `oauth2-proxy --user-id-claim=sub`).
pub fn mint(secret: &str, id: &str) -> Option<String> {
    valid_id(id).then(|| {
        let tag = B64.encode(mac(secret, id).finalize().into_bytes());
        format!("{PREFIX}{id}.{tag}")
    })
}

/// The GitLab user id a key was minted for, if its signature holds (constant-time compare).
pub fn verify(secret: &str, key: &str) -> Option<String> {
    let (id, tag) = key.strip_prefix(PREFIX)?.split_once('.')?;
    if !valid_id(id) {
        return None;
    }
    let tag = B64.decode(tag).ok()?;
    mac(secret, id).verify_slice(&tag).ok()?;
    Some(id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let k = mint("s3cret", "42").unwrap();
        assert_eq!(verify("s3cret", &k).as_deref(), Some("42"));
    }

    #[test]
    fn rejects_forgeries() {
        let k = mint("s3cret", "42").unwrap();
        assert_eq!(verify("other", &k), None, "wrong secret");
        let swapped = k.replacen("ogh_42.", "ogh_43.", 1);
        assert_eq!(verify("s3cret", &swapped), None, "id swapped");
        assert_eq!(verify("s3cret", "ogh_42."), None);
        assert_eq!(verify("s3cret", "42.abc"), None);
        assert_eq!(verify("s3cret", ""), None);
    }

    #[test]
    fn only_numeric_ids() {
        assert_eq!(mint("s", "alice@x.es"), None);
        assert_eq!(mint("s", ""), None);
        assert_eq!(mint("s", &"9".repeat(21)), None);
    }
}
