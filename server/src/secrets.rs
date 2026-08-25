// Open Cloud Secrets Store client (universe.secret scopes). Secret content never
// travels in the clear: the API only accepts a libsodium sealed box (X25519 +
// XSalsa20-Poly1305) made with the universe's public key, base64-encoded, plus the
// key_id that public key came with. crypto_box is the RustCrypto implementation of
// crypto_box_seal, so the ciphertext is what PyNaCl's SealedBox would produce.
// Same env-only key as cloud.rs.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use crypto_box::aead::OsRng;
use crypto_box::PublicKey;
use reqwest::{Client, Method};
use serde_json::{json, Value};

use crate::env;
use crate::httpx;

const CLOUD: &str = "https://apis.roblox.com/cloud/v2";

pub type SecretsResult = Result<Value, String>;

fn key_and_universe() -> Result<(String, String), String> {
    let key = env::var("ROBLOX_OPEN_CLOUD_KEY").ok_or("Missing env: set ROBLOX_OPEN_CLOUD_KEY.")?;
    let universe = env::var("ROBLOX_UNIVERSE_ID").ok_or("Missing env: set ROBLOX_UNIVERSE_ID.")?;
    Ok((key, universe))
}

fn base(universe: &str) -> String {
    format!("{CLOUD}/universes/{universe}/secrets")
}

/// Metadata only; the API never returns secret content.
pub async fn list_secrets(
    http: &Client,
    limit: Option<i64>,
    cursor: Option<&str>,
) -> SecretsResult {
    let (key, universe) = key_and_universe()?;
    let mut q: Vec<(&str, String)> = Vec::new();
    if let Some(l) = limit {
        q.push(("limit", l.to_string()));
    }
    if let Some(c) = cursor {
        q.push(("cursor", c.to_string()));
    }
    httpx::request_json(http, &key, Method::GET, &base(&universe), &q, None).await
}

pub async fn delete_secret(http: &Client, id: &str) -> SecretsResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{}/{}", base(&universe), id);
    httpx::request_json(http, &key, Method::DELETE, &url, &[], None).await
}

/// Creates (`update = false`) or replaces (`update = true`) a secret. The content is
/// sealed here with a freshly fetched universe public key.
pub async fn put_secret(
    http: &Client,
    id: &str,
    content: &str,
    domain: Option<&str>,
    update: bool,
) -> SecretsResult {
    let (key, universe) = key_and_universe()?;
    let public = httpx::request_json(
        http,
        &key,
        Method::GET,
        &format!("{}/public-key", base(&universe)),
        &[],
        None,
    )
    .await?;
    let key_id = public
        .get("key_id")
        .and_then(Value::as_str)
        .ok_or("public key response had no key_id")?;
    let public_b64 = public
        .get("secret")
        .and_then(Value::as_str)
        .ok_or("public key response had no key material")?;
    let sealed = seal(public_b64, content.as_bytes())?;

    let mut body = serde_json::Map::new();
    body.insert("id".into(), json!(id));
    body.insert("secret".into(), json!(sealed));
    body.insert("key_id".into(), json!(key_id));
    if let Some(d) = domain {
        body.insert("domain".into(), json!(d));
    }
    let (method, url) = if update {
        (Method::PATCH, format!("{}/{}", base(&universe), id))
    } else {
        (Method::POST, base(&universe))
    };
    httpx::request_json(http, &key, method, &url, &[], Some(&Value::Object(body))).await
}

/// Sealed-box encryption of `plaintext` to a base64 X25519 public key; base64 out.
fn seal(public_key_b64: &str, plaintext: &[u8]) -> Result<String, String> {
    let bytes = B64
        .decode(public_key_b64.trim())
        .map_err(|e| format!("public key is not valid base64: {e}"))?;
    let public = PublicKey::from_slice(&bytes)
        .map_err(|_| format!("public key must be 32 bytes, got {}", bytes.len()))?;
    let sealed = public
        .seal(&mut OsRng, plaintext)
        .map_err(|_| "sealed box encryption failed".to_string())?;
    Ok(B64.encode(sealed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto_box::SecretKey;

    #[test]
    fn seal_round_trips_through_the_recipient_key() {
        let recipient = SecretKey::generate(&mut OsRng);
        let public_b64 = B64.encode(recipient.public_key().as_bytes());
        let sealed = seal(&public_b64, b"discord-webhook-token").unwrap();
        let opened = recipient.unseal(&B64.decode(sealed).unwrap()).unwrap();
        assert_eq!(opened, b"discord-webhook-token");
    }

    #[test]
    fn seal_rejects_a_malformed_key() {
        assert!(seal("not base64!", b"x").is_err());
        assert!(seal(&B64.encode([0u8; 16]), b"x").is_err());
    }
}
