/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! DBSC key management and DBSC proof JWT signing.
//!
//! BAO PATCH (REQ-BRW-051): each session owns a long-lived key pair; the
//! public half travels in the registration JWT's `jwk` header member and the
//! private half signs the `Secure-Session-Response` proof JWT on every
//! registration/refresh POST.
//!
//! SECURITY BOUNDARY (registered, not silent): this slice stores private key
//! material in process memory (`MemoryKeyStore`). That is the WPT acceptance
//! shape — the draft requires a non-exportable, device-bound key (TPM/OS
//! keychain) for product use. The `KeyStore` trait is the seam where a
//! secure backend plugs in; product wiring must install one before shipping
//! DBSC to real sites. This is design D1's explicit contract
//! (`d-design.md` §3.3.2 keys.rs, R4), not an accidental downgrade.
//!
//! Algorithm coverage: RS256 is fully implemented (keygen + RSASSA-PKCS1-v1_5
//! signing via the `rsa` crate, the same dependency the script crate's
//! WebCrypto uses). `ES256`/`none` parse at the header level but fail key
//! generation fail-closed (`None`) — a registration header demanding them is
//! not acted on rather than silently downgraded.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use rsa::RsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::traits::PublicKeyParts;
use sha2::{Digest, Sha256};

use crate::dbsc::model::DbscAlgorithm;

/// A session's proof-of-possession key material.
#[derive(Clone)]
pub(crate) enum DbscKeyPair {
    Rsa(RsaPrivateKey),
}

/// The public JWK form sent in the registration JWT header.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PublicJwk {
    pub(crate) kty: &'static str,
    pub(crate) n: String,
    pub(crate) e: String,
}

impl PublicJwk {
    pub(crate) fn to_json(&self) -> String {
        format!(
            r#"{{"kty":"{}","n":"{}","e":"{}"}}"#,
            self.kty, self.n, self.e
        )
    }

    /// The RFC 7638 JWK thumbprint: base64url (no padding) of SHA-256 over
    /// the canonical JWK JSON — the required members in lexicographic order
    /// (`e`, `kty`, `n`) with no whitespace. DBSC federated registration
    /// compares this value against the `provider_key` header parameter
    /// (draft §8.11 step 13).
    pub(crate) fn thumbprint(&self) -> String {
        let canonical = format!(
            r#"{{"e":"{}","kty":"{}","n":"{}"}}"#,
            self.e, self.kty, self.n
        );
        URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()).to_vec())
    }
}

impl DbscKeyPair {
    /// Generate a key pair for the given algorithm. `None` = the algorithm
    /// has no backend in this slice (ES256/none) — fail-closed, the caller
    /// aborts the registration.
    pub(crate) fn generate(algorithm: DbscAlgorithm) -> Option<DbscKeyPair> {
        match algorithm {
            DbscAlgorithm::Rs256 => {
                // 2048-bit modulus with the standard 65537 exponent (the
                // draft's recommendation; generation cost is acceptable at
                // session-creation time).
                let mut rng = rand::rng();
                let private_key = RsaPrivateKey::new(&mut rng, 2048).ok()?;
                Some(DbscKeyPair::Rsa(private_key))
            },
            DbscAlgorithm::Es256 | DbscAlgorithm::None => None,
        }
    }

    /// The RFC 7638 thumbprint of this key pair's public key (the value the
    /// `provider_key` parameter carries in federated registration).
    pub(crate) fn thumbprint(&self) -> String {
        self.public_jwk().thumbprint()
    }

    /// The public JWK for this key pair.
    pub(crate) fn public_jwk(&self) -> PublicJwk {
        match self {
            DbscKeyPair::Rsa(private_key) => {
                let public_key = private_key.to_public_key();
                PublicJwk {
                    kty: "RSA",
                    n: URL_SAFE_NO_PAD.encode(public_key.n().to_be_bytes_trimmed_vartime()),
                    e: URL_SAFE_NO_PAD.encode(public_key.e().to_be_bytes_trimmed_vartime()),
                }
            },
        }
    }

    /// Sign the DBSC proof JWT: header `typ=dbsc+jwt` with `jwk`
    /// (registration shape) or without it (refresh shape), payload `jti` =
    /// challenge (when the caller holds one) and optional `authorization`.
    ///
    /// BAO PATCH (REQ-BRW-051): `challenge` is `Option` — the first refresh
    /// attempt runs without any challenge (the draft's refresh loop starts
    /// challenge-less; a 403 response supplies one for the retry), and that
    /// JWT's payload must omit `jti` entirely rather than carry an empty
    /// string.
    pub(crate) fn sign_proof_jwt(
        &self,
        algorithm: DbscAlgorithm,
        challenge: Option<&str>,
        authorization: Option<&str>,
        include_jwk: bool,
    ) -> Option<String> {
        match (self, algorithm) {
            (DbscKeyPair::Rsa(private_key), DbscAlgorithm::Rs256) => {
                let mut header = format!(r#"{{"typ":"dbsc+jwt","alg":"RS256""#);
                if include_jwk {
                    header.push_str(&format!(
                        r#","jwk":{}"#,
                        self.public_jwk().to_json()
                    ));
                }
                header.push('}');

                let mut payload = String::new();
                if let Some(challenge) = challenge {
                    payload.push_str(&format!(r#""jti":{}"#, json_string(challenge)));
                }
                if let Some(authorization) = authorization {
                    if !payload.is_empty() {
                        payload.push(',');
                    }
                    payload.push_str(&format!(
                        r#""authorization":{}"#,
                        json_string(authorization)
                    ));
                }
                let payload = format!("{{{}}}", payload);

                let signing_input = format!(
                    "{}.{}",
                    URL_SAFE_NO_PAD.encode(header.as_bytes()),
                    URL_SAFE_NO_PAD.encode(payload.as_bytes())
                );
                let signing_key = SigningKey::<Sha256>::new(private_key.clone());
                let signature = signing_key.sign(signing_input.as_bytes());
                Some(format!(
                    "{}.{}",
                    signing_input,
                    URL_SAFE_NO_PAD.encode(signature.to_bytes())
                ))
            },
            _ => None,
        }
    }
}

/// A JSON string literal with proper escaping (the minimal subset needed for
/// challenge/authorization values, which are SFV strings — printable ASCII
/// by construction — but escaped correctly regardless).
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            },
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Verify our own JWT round-trips through the shape the WPT server
/// verifies (`jwt_helper.py` decodes base64url JSON header/payload and
/// checks the RSASSA-PKCS1-v1_5 signature against the JWK components).
#[cfg(test)]
mod tests {
    use super::*;

    /// The RFC 7638 §3.1 worked example, verbatim: the canonical JSON has
    /// `e`, `kty`, `n` in that order and the thumbprint is the published
    /// value (recomputed here with a local SHA-256 to pin the algorithm).
    #[test]
    fn jwk_thumbprint_matches_rfc7638_vector() {
        let jwk = PublicJwk {
            kty: "RSA",
            n: "0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw".to_owned(),
            e: "AQAB".to_owned(),
        };
        assert_eq!(
            jwk.thumbprint(),
            "NzbLsXh8uDCcd-6MNwXF4W_7noWXFZAfHkxZsRGC9Xs"
        );
        // The JWK sent in the registration header keeps the conventional
        // `kty`,`n`,`e` member order (the thumbprint canonicalizes separately).
        assert_eq!(
            jwk.to_json(),
            format!(r#"{{"kty":"RSA","n":"{}","e":"AQAB"}}"#, jwk.n)
        );
    }

    #[test]
    fn generated_key_thumbprint_is_stable_and_url_safe() {
        let key_pair =
            DbscKeyPair::generate(DbscAlgorithm::Rs256).expect("RS256 must generate");
        let thumbprint = key_pair.thumbprint();
        assert_eq!(thumbprint, key_pair.public_jwk().thumbprint());
        assert!(!thumbprint.contains('='));
        assert!(thumbprint
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn rs256_jwt_roundtrip_shape() {
        let key_pair =
            DbscKeyPair::generate(DbscAlgorithm::Rs256).expect("RS256 must generate");
        let jwt = key_pair
            .sign_proof_jwt(
                DbscAlgorithm::Rs256,
                Some("login_challenge_value"),
                None,
                true,
            )
            .expect("must sign");

        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);

        let header = STANDARD
            .decode(URL_SAFE_NO_PAD.decode(parts[0]).unwrap())
            .unwrap_or_else(|_| STANDARD.decode(parts[0]).unwrap());
        let header: serde_json::Value = serde_json::from_slice(&header).unwrap();
        assert_eq!(header["typ"], "dbsc+jwt");
        assert_eq!(header["alg"], "RS256");
        assert_eq!(header["jwk"]["kty"], "RSA");

        let payload: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD.decode(parts[1]).unwrap(),
        )
        .unwrap();
        assert_eq!(payload["jti"], "login_challenge_value");
        assert!(payload.get("authorization").is_none());
        assert!(payload.get("sub").is_none());
    }

    #[test]
    fn refresh_jwt_omits_jwk_and_carries_authorization() {
        let key_pair =
            DbscKeyPair::generate(DbscAlgorithm::Rs256).expect("RS256 must generate");
        let jwt = key_pair
            .sign_proof_jwt(
                DbscAlgorithm::Rs256,
                Some("refresh_challenge_value"),
                Some("authcode"),
                false,
            )
            .expect("must sign");
        let payload_b64 = jwt.split('.').nth(1).unwrap();
        let payload: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload_b64).unwrap()).unwrap();
        assert_eq!(payload["authorization"], "authcode");

        let header_b64 = jwt.split('.').next().unwrap();
        let header: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header_b64).unwrap()).unwrap();
        assert!(header.get("jwk").is_none());
    }

    #[test]
    fn challenge_less_refresh_jwt_omits_jti() {
        // The first refresh attempt runs without a challenge: the payload
        // must omit `jti` entirely (an empty-string jti would be a invented
        // value, and WPT's refresh-does-not-send-challenge asserts the
        // registration challenge is not replayed).
        let key_pair =
            DbscKeyPair::generate(DbscAlgorithm::Rs256).expect("RS256 must generate");
        let jwt = key_pair
            .sign_proof_jwt(DbscAlgorithm::Rs256, None, None, false)
            .expect("must sign");
        let payload_b64 = jwt.split('.').nth(1).unwrap();
        let payload: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload_b64).unwrap()).unwrap();
        assert!(payload.get("jti").is_none());
        assert!(payload.get("authorization").is_none());
        assert_eq!(payload.as_object().map(|object| object.len()), Some(0));
    }

    #[test]
    fn unsupported_algorithms_fail_closed() {
        assert!(DbscKeyPair::generate(DbscAlgorithm::Es256).is_none());
        assert!(DbscKeyPair::generate(DbscAlgorithm::None).is_none());
    }
}
