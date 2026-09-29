// @trace REQ-ENG-001 [test:W22D-ERROR-MODEL] [level:integration]
// W22d (#17-D) error-model contract: golden variant lock + taxonomy checks.
//
// Golden: tests/golden/errors.json — the variant-name set and Display
// prefixes of the 7 pub error types. Any variant added/removed/renamed, or a
// Display prefix reworded, without a matching golden update fails here
// (contract lock, same philosophy as COMMAND_SURFACE).
//
// Taxonomy asserted:
//   1. every pub error type is `impl std::error::Error` (dyn-compatible);
//   2. kind discrimination is by VARIANT, not by parsing a String payload —
//      pinned by asserting distinct variants for distinct failure kinds;
//   3. source chain: the CURRENT state is locked honestly. FINDINGS recorded
//      (reported, not fixed — w22d scope is observational):
//      - #w22d-2: CdpError::IoError carries io::Error but source() is None
//        (the hand-written `impl Error for CdpError {}` has no source match);
//      - #w22d-1: ConnectError's From<io::Error> materializes to_string()
//        into ConnectionFailed — the io source is unrecoverable.
//      Both are asserted AS-IS so a future fix flips this test loudly.

use std::error::Error as StdError;

use bao::browser::BrowserError;
use bao::cdp_client::{CdpError, ConnectError};
use bao_crypto::CryptoError;
use bun_sm::JsError;
#[allow(unused_imports)]
use bun_sm::ErrorCode;
use bun_transpiler::TranspileError;

fn golden() -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/errors.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read golden {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("golden errors.json must be valid JSON")
}

/// Extract the variant name from the Debug form (`Variant(payload...)`).
fn variant_name(err: &dyn StdError) -> String {
    let dbg = format!("{err:?}");
    let name: String = dbg
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    assert!(!name.is_empty(), "Debug form must start with a variant name: {dbg}");
    name
}

/// One constructed representative per variant, with its expected Display
/// output prefix — `(dyn_error, variant, expected_prefix)`.
fn browser_cases() -> Vec<(Box<dyn StdError>, &'static str, &'static str)> {
    vec![
        (Box::new(BrowserError::Init("x".into())), "Init", "browser init error: x"),
        (Box::new(BrowserError::Navigation("x".into())), "Navigation", "navigation error: x"),
        (Box::new(BrowserError::Rendering("x".into())), "Rendering", "rendering error: x"),
        (Box::new(BrowserError::JavaScript("x".into())), "JavaScript", "javascript error: x"),
        (Box::new(BrowserError::CDP("x".into())), "CDP", "cdp error: x"),
    ]
}

fn connect_cases() -> Vec<(Box<dyn StdError>, &'static str, &'static str)> {
    vec![
        (Box::new(ConnectError::InvalidUrl), "InvalidUrl", "invalid URL (empty or missing scheme)"),
        (Box::new(ConnectError::InvalidScheme("wsx".into())), "InvalidScheme", "invalid URL scheme:"),
        (Box::new(ConnectError::LaunchError("x".into())), "LaunchError", "browser launch failed: x"),
        (Box::new(ConnectError::ConnectionFailed("x".into())), "ConnectionFailed", "connection failed: x"),
        (Box::new(ConnectError::Timeout("x".into())), "Timeout", "connect timeout: x"),
    ]
}

fn cdp_cases() -> Vec<(Box<dyn StdError>, &'static str, &'static str)> {
    vec![
        (Box::new(CdpError::ProtocolError("x".into())), "ProtocolError", "CDP protocol error: x"),
        (Box::new(CdpError::JsonError("x".into())), "JsonError", "JSON error: x"),
        (Box::new(CdpError::IoError(std::io::Error::other("boom"))), "IoError", "I/O error: boom"),
        (Box::new(CdpError::ConnectionClosed), "ConnectionClosed", "connection closed"),
        (Box::new(CdpError::Timeout("x".into())), "Timeout", "timeout: x"),
        (Box::new(CdpError::TransportError("x".into())), "TransportError", "transport error: x"),
        (Box::new(CdpError::HandshakeError("x".into())), "HandshakeError", "WebSocket handshake error: x"),
    ]
}

fn crypto_cases() -> Vec<(Box<dyn StdError>, &'static str, &'static str)> {
    vec![
        (Box::new(CryptoError::InvalidKeyLength { expected: 1, got: 2 }), "InvalidKeyLength", "Invalid key length:"),
        (Box::new(CryptoError::InvalidNonceLength { expected: 1, got: 2 }), "InvalidNonceLength", "Invalid nonce length:"),
        (Box::new(CryptoError::InvalidKey("x".into())), "InvalidKey", "Invalid key: x"),
        (Box::new(CryptoError::InvalidCurve("x".into())), "InvalidCurve", "Invalid curve: x"),
        (Box::new(CryptoError::InvalidCertificate("x".into())), "InvalidCertificate", "Invalid certificate: x"),
        (Box::new(CryptoError::InvalidSignature("x".into())), "InvalidSignature", "Invalid signature: x"),
        (Box::new(CryptoError::InvalidLength("x".into())), "InvalidLength", "Invalid length: x"),
        (Box::new(CryptoError::InvalidFormat("x".into())), "InvalidFormat", "Invalid format: x"),
        (Box::new(CryptoError::UnsupportedAlgorithm("x".into())), "UnsupportedAlgorithm", "Unsupported algorithm: x"),
        (Box::new(CryptoError::EncryptionFailed("x".into())), "EncryptionFailed", "Encryption failed: x"),
        (Box::new(CryptoError::DecryptionFailed("x".into())), "DecryptionFailed", "Decryption failed: x"),
        (Box::new(CryptoError::SignFailed("x".into())), "SignFailed", "Signing failed: x"),
        (Box::new(CryptoError::VerifyFailed("x".into())), "VerifyFailed", "Verification failed: x"),
        (Box::new(CryptoError::EncodingFailed("x".into())), "EncodingFailed", "Encoding failed: x"),
        (Box::new(CryptoError::DecodingFailed("x".into())), "DecodingFailed", "Decoding failed: x"),
        (Box::new(CryptoError::CertificateError("x".into())), "CertificateError", "Certificate error: x"),
        (Box::new(CryptoError::KdfError("x".into())), "KdfError", "KDF error: x"),
        (Box::new(CryptoError::KeyExchangeError("x".into())), "KeyExchangeError", "Key exchange error: x"),
        (Box::new(CryptoError::KeyPairError("x".into())), "KeyPairError", "Key pair generation error: x"),
        (Box::new(CryptoError::KeyGenerationFailed("x".into())), "KeyGenerationFailed", "Key generation failed: x"),
        (Box::new(CryptoError::SharedSecretFailed("x".into())), "SharedSecretFailed", "Shared secret computation failed: x"),
        (Box::new(CryptoError::RandomFailed("x".into())), "RandomFailed", "Random generation failed: x"),
        (Box::new(CryptoError::OperationFailed("x".into())), "OperationFailed", "Operation failed: x"),
    ]
}

/// Golden lock: for every type, the constructed-variant NAME set equals the
/// golden variant set (both directions: a case missing from the test and a
/// variant missing from the golden both fail), and every Display matches its
/// golden prefix.
#[test]
fn error_golden_variant_and_display_lock() {
    let g = golden();

    let groups: Vec<(&str, Vec<(Box<dyn StdError>, &'static str, &'static str)>)> = vec![
        ("BrowserError", browser_cases()),
        ("ConnectError", connect_cases()),
        ("CdpError", cdp_cases()),
        ("CryptoError", crypto_cases()),
    ];

    for (type_name, cases) in &groups {
        let golden_variants: Vec<String> = g[type_name]["variants"]
            .as_array()
            .expect("golden variants array")
            .iter()
            .map(|v| v.as_str().expect("variant string").to_string())
            .collect();

        // Forward: every constructed case's variant must be in the golden.
        for (err, variant, prefix) in cases {
            assert!(
                golden_variants.iter().any(|gv| gv == variant),
                "{type_name}::{variant} is constructed but missing from golden variants {:?}",
                golden_variants
            );
            let disp = err.to_string();
            assert!(
                disp.starts_with(prefix),
                "{type_name}::{variant} Display drifted: got {disp:?}, expected prefix {prefix:?}"
            );
            assert_eq!(
                &variant_name(err.as_ref()), variant,
                "{type_name} Debug head must equal the variant name"
            );
        }
        // Reverse: every golden variant must have a constructed case.
        for gv in &golden_variants {
            assert!(
                cases.iter().any(|(_, v, _)| v == gv),
                "golden variant {type_name}::{gv} has no constructed case in this test"
            );
        }
        // Count parity: no duplicate constructions masking a golden entry.
        assert_eq!(
            cases.len(),
            golden_variants.len(),
            "{type_name}: constructed case count must equal golden variant count"
        );
    }
}

/// Struct-shaped errors (no enum variants): field face + Display prefix lock.
#[test]
fn error_golden_struct_shapes_lock() {
    let g = golden();

    // TranspileError: single-message struct, prefix "transpile_ts error: ".
    let te = TranspileError { message: "boom".into() };
    assert_eq!(te.to_string(), "transpile_ts error: boom");
    assert_eq!(g["TranspileError"]["display"].as_str(), Some("transpile_ts error: "));

    // JsError (bun_sm): the runtime main-entry typed error — structured
    // fields, location-prefixed Display, optional stack block.
    let je = JsError {
        message: "oops".into(),
        filename: "app.js".into(),
        line: 3,
        column: 7,
        stack: Some("at f".into()),
    };
    let disp = je.to_string();
    assert!(disp.starts_with("app.js:3:7: oops"), "JsError Display drifted: {disp:?}");
    assert!(disp.contains("at f"), "JsError must carry the stack block");
    let no_stack = JsError {
        message: "m".into(),
        filename: "f.js".into(),
        line: 1,
        column: 1,
        stack: None,
    };
    assert_eq!(no_stack.to_string(), "f.js:1:1: m");

    // ErrorCode: repr(u8) JS-kind classifier — discriminant values are the
    // wire/stable face; lock every value.
    let disc = g["ErrorCode"]["discriminants"].as_object().expect("discriminants");
    let pairs = [
        ("NoError", 0u8),
        ("TypeError", 1),
        ("RangeError", 2),
        ("SyntaxError", 3),
        ("ReferenceError", 4),
        ("StackOverflow", 5),
        ("OutOfMemory", 6),
        ("URIError", 7),
        ("EvalError", 8),
        ("InternalError", 9),
        ("GenericError", 10),
    ];
    for (name, value) in pairs {
        assert_eq!(
            disc.get(name).and_then(|v| v.as_u64()),
            Some(value as u64),
            "ErrorCode::{name} discriminant drifted"
        );
        let _ = name;
    }
}

/// Taxonomy 1: every pub error type is dyn-compatible std::error::Error.
/// Taxonomy 3 (source chain): the CURRENT state is locked — including the
/// two chain-loss findings (#w22d-1/#w22d-2) so a fix flips this loudly.
#[test]
fn error_taxonomy_error_trait_and_source_chain() {
    // dyn-compatibility across all 7 types (ErrorCode is a repr(u8)
    // classifier, not a std Error by design — excluded, pinned in golden).
    let browser: Box<dyn StdError> = Box::new(BrowserError::Init("x".into()));
    let connect: Box<dyn StdError> = Box::new(ConnectError::InvalidUrl);
    let cdp: Box<dyn StdError> = Box::new(CdpError::ConnectionClosed);
    let crypto: Box<dyn StdError> = Box::new(CryptoError::InvalidKey("x".into()));
    let transpile: Box<dyn StdError> = Box::new(TranspileError { message: "x".into() });
    let js: Box<dyn StdError> = Box::new(JsError {
        message: "x".into(),
        filename: "f".into(),
        line: 1,
        column: 1,
        stack: None,
    });
    for (name, e) in [
        ("BrowserError", &*browser),
        ("ConnectError", &*connect),
        ("CdpError", &*cdp),
        ("CryptoError", &*crypto),
        ("TranspileError", &*transpile),
        ("JsError", &*js),
    ] {
        assert!(!e.to_string().is_empty(), "{name} must render non-empty Display");
    }

    // FINDING #w22d-2 (locked as-is): CdpError::IoError carries the io::Error
    // in its payload, but source() is None — the hand-written Error impl has
    // no source() arm. A fix must flip this to Some.
    let io_err = CdpError::IoError(std::io::Error::other("root-cause"));
    assert!(
        io_err.source().is_none(),
        "#w22d-2 flipped: CdpError::IoError now EXPOSES a source — update the \
         finding and docs/error-model.md (chain-loss no longer holds)"
    );
    // The payload at least renders the root cause (Display keeps the text).
    assert!(
        io_err.to_string().contains("root-cause"),
        "IoError Display must retain the cause text even while source() is None"
    );

    // FINDING #w22d-1 (locked as-is): ConnectError's From<io::Error>
    // materializes the source to_string() into ConnectionFailed — the typed
    // source is unrecoverable from the value.
    let conn: ConnectError = std::io::Error::other("root-cause").into();
    match conn {
        ConnectError::ConnectionFailed(msg) => assert!(
            msg.contains("root-cause"),
            "From<io::Error> must at least keep the cause TEXT: {msg:?}"
        ),
        other => panic!("From<io::Error> must build ConnectionFailed, got {other:?}"),
    }

    // String-payload variants structurally have no source (pinned: distinct
    // kinds are distinct VARIANTS, never String parsing).
    assert!(CdpError::ConnectionClosed.source().is_none());
    assert!(BrowserError::Init("x".into()).source().is_none());
}
