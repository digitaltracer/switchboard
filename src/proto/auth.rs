//! Runner tokens: presented as a bearer token at the WebSocket upgrade, stored as SHA-256 hex.

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Compares the token's hash with a stored hash in constant time.
pub fn token_matches(token: &str, stored_hash_hex: &str) -> bool {
    let Ok(stored) = hex::decode(stored_hash_hex) else {
        return false;
    };
    let actual = Sha256::digest(token.as_bytes());
    stored.len() == actual.len() && bool::from(actual.as_slice().ct_eq(&stored))
}

/// Extracts the token from an `Authorization: Bearer <token>` header value.
pub fn bearer(header: &str) -> Option<&str> {
    let (scheme, token) = header.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

/// The machine whose stored hash matches, checking every entry so timing doesn't reveal which.
pub fn authenticate<'a>(
    token: &str,
    machines: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<&'a str> {
    let mut found = None;
    for (id, hash) in machines {
        if token_matches(token, hash) && found.is_none() {
            found = Some(id);
        }
    }
    found
}
