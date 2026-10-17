//! Wire-level coverage for `bao_crypto::certificate::X509Certificate` —
//! the PEM/DER parse face that `node_crypto.rs`'s `X509Certificate` binding
//! delegates to (REQ-ENG-007 crypto entity chain).
//!
//! Fixture: one fixed RSA-2048 self-signed cert (openssl, CN=c7cov.local,
//! O=bao-cov, serial 78CF…556, 2026-10-16 → 2036-10-13), embedded as PEM plus
//! its DER byte form so the two constructors can be cross-checked against the
//! same underlying certificate — `fingerprint_sha256` of the PEM-parsed cert
//! must equal the SHA-256 of the literal DER bytes (the digest oracle), and
//! the DER-parsed cert must reproduce every field the PEM parse reports.

use bao_crypto::certificate::X509Certificate;
use bao_crypto::CryptoError;

/// openssl req -x509 -newkey rsa:2048 -nodes -subj "/CN=c7cov.local/O=bao-cov"
///   -days 3650 -sha256 (generated 2026-10-16 for this suite).
const CERT_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIDMTCCAhmgAwIBAgIUeM95qWqJ+mzLwF8qAGGB8wuYNVYwDQYJKoZIhvcNAQEL
BQAwKDEUMBIGA1UEAwwLYzdjb3YubG9jYWwxEDAOBgNVBAoMB2Jhby1jb3YwHhcN
MjYxMDE2MTg1NTM2WhcNMzYxMDEzMTg1NTM2WjAoMRQwEgYDVQQDDAtjN2Nvdi5s
b2NhbDEQMA4GA1UECgwHYmFvLWNvdjCCASIwDQYJKoZIhvcNAQEBBQADggEPADCC
AQoCggEBAIqpu1LSDpnyWA9wNAd8Qdz3HowG1hlgW4EMmkv+u0BqF568snPZHivO
29k+xyWLYQqEJau5jh1S8L8eX8S9UCveF58aZ0QTdFWYU1p0FK2NZOHBO9Yro2uq
nHK1P80YN4gO1AXePi0R6JXprNK06+z/0LCuKQmYEGs7WS9oz0uxZoGHfsjokJU+
XPlzDaRR7R8yKYS8YDy7kRKlrQf8tFFAiXH4Esq7Ul9dzzjsQsj65o129YV9JDYj
HMi0sVNKpffTGIzgSrZ/m74NYM43MISzLOvKJ2lH6al5+JeGDsZqWrhzkVL70yHp
BTyVoLKQlNFCDDCXzg6BRYwS7O37J90CAwEAAaNTMFEwHQYDVR0OBBYEFPbv5FzL
vYngdoC0VHzst6wr3z6cMB8GA1UdIwQYMBaAFPbv5FzLvYngdoC0VHzst6wr3z6c
MA8GA1UdEwEB/wQFMAMBAf8wDQYJKoZIhvcNAQELBQADggEBADWxecQJ449xTofo
aRMC7esIVWmVMMC0sxRtUN2Z0yLqutjY9jHd9bBjQSl00aTosp6AeKb67R4Db0FJ
mlPEYpbO5OEPBTNZ/uLpYt68Y4R5Um1F4KIPnKMMK97XITbhBieNMMXRJkAzsb92
4vHSeSLtJmzlThk/Dz4ipbN9U7PiB3aNKIiyKkLxFHBoxxHKIK3wnXJ1/kLPpKRw
ucJmUcKwAtrLSMqRb4gK/drB3yPHHYzSMh36tWJ+DcuRuMp0lhb6yi5zy5gpYI95
tasnGH19YjQbjA1hild9U8bR7vYQGqc1IngE7i8yskaR1J0tet0BlQ5Ev9tyPwR6
k70r2iQ=
-----END CERTIFICATE-----";

/// The same certificate in DER form (openssl x509 -outform DER), hex-encoded
/// so the const stays ASCII; decoded once per test that needs it.
const CERT_DER_HEX: &str = concat!(
    "3082033130820219A003020102021478",
    "CF79A96A89FA6CCBC05F2A006181F30B",
    "983556300D06092A864886F70D01010B",
    "050030283114301206035504030C0B63",
    "37636F762E6C6F63616C3110300E0603",
    "55040A0C0762616F2D636F76301E170D",
    "3236313031363138353533365A170D33",
    "36313031333138353533365A30283114",
    "301206035504030C0B6337636F762E6C",
    "6F63616C3110300E060355040A0C0762",
    "616F2D636F7630820122300D06092A86",
    "4886F70D01010105000382010F003082",
    "010A02820101008AA9BB52D20E99F258",
    "0F7034077C41DCF71E8C06D619605B81",
    "0C9A4BFEBB406A179EBCB273D91E2BCE",
    "DBD93EC7258B610A8425ABB98E1D52F0",
    "BF1E5FC4BD502BDE179F1A6744137455",
    "98535A7414AD8D64E1C13BD62BA36BAA",
    "9C72B53FCD1837880ED405DE3E2D11E8",
    "95E9ACD2B4EBECFFD0B0AE290998106B",
    "3B592F68CF4BB16681877EC8E890953E",
    "5CF9730DA451ED1F322984BC603CBB91",
    "12A5AD07FCB451408971F812CABB525F",
    "5DCF38EC42C8FAE68D76F5857D243623",
    "1CC8B4B1534AA5F7D3188CE04AB67F9B",
    "BE0D60CE373084B32CEBCA276947E9A9",
    "79F897860EC66A5AB8739152FBD321E9",
    "053C95A0B29094D1420C3097CE0E8145",
    "8C12ECEDFB27DD0203010001A3533051",
    "301D0603551D0E04160414F6EFE45CCB",
    "BD89E07680B4547CECB7AC2BDF3E9C30",
    "1F0603551D23041830168014F6EFE45C",
    "CBBD89E07680B4547CECB7AC2BDF3E9C",
    "300F0603551D130101FF040530030101",
    "FF300D06092A864886F70D01010B0500",
    "038201010035B179C409E38F714E87E8",
    "691302EDEB0855699530C0B4B3146D50",
    "DD99D322EABAD8D8F631DDF5B0634129",
    "74D1A4E8B29E8078A6FAED1E036F4149",
    "9A53C46296CEE4E10F053359FEE2E962",
    "DEBC638479526D45E0A20F9CA30C2BDE",
    "D72136E106278D30C5D1264033B1BF76",
    "E2F1D27922ED266CE54E193F0F3E22A5",
    "B37D53B3E207768D2888B22A42F11470",
    "68C711CA20ADF09D7275FE42CFA4A470",
    "B9C26651C2B002DACB48CA916F880AFD",
    "DAC1DF23C71D8CD2321DFAB5627E0DCB",
    "91B8CA749616FACA2E73CB9829608F79",
    "B5AB27187D7D62341B8C0D618A577D53",
    "C6D1EEF6101AA735227804EE2F32B246",
    "91D49D2D7ADD01950E44BFDB723F047A",
    "93BD2BDA24",
);

fn cert_der() -> Vec<u8> {
    (0..CERT_DER_HEX.len() / 2)
        .map(|i| u8::from_str_radix(&CERT_DER_HEX[i * 2..i * 2 + 2], 16).expect("hex digit"))
        .collect()
}

fn assert_is_invalid_certificate(err: CryptoError, marker: &str) {
    let CryptoError::InvalidCertificate(msg) = err else {
        panic!("{marker}: expected InvalidCertificate, got {err:?}");
    };
    assert!(!msg.is_empty(), "{marker}: error message must be non-empty");
}

/// `expect_err` stand-in: `X509Certificate` carries a raw pointer and
/// implements no `Debug`, so the error paths unpack by match instead.
fn expect_parse_failure(r: Result<X509Certificate, CryptoError>, marker: &str) -> CryptoError {
    match r {
        Ok(_) => panic!("{marker}: expected parse failure, got a certificate"),
        Err(e) => e,
    }
}

// ─── Happy paths ───────────────────────────────────────────────────────────

/// PEM parse must surface the fixture's identity fields verbatim from the
/// underlying X509 (X509_NAME_oneline shape: `/CN=…/O=…`).
#[test]
fn from_pem_exposes_subject_issuer_and_validity() {
    let cert = X509Certificate::from_pem(CERT_PEM).expect("fixture PEM must parse");
    let subject = cert.subject();
    assert!(
        subject.contains("CN") && subject.contains("c7cov.local"),
        "subject missing CN=c7cov.local: {subject:?}"
    );
    assert!(
        subject.contains("bao-cov"),
        "subject missing O=bao-cov: {subject:?}"
    );
    // Self-signed: issuer equals subject.
    assert_eq!(cert.issuer(), subject, "self-signed cert issuer != subject");

    // ASN.1 UTCTime strings as stored (YYMMDDHHMMSSZ), from the fixture's
    // openssl -dates output: notBefore 261016185536Z, notAfter 361013185536Z.
    assert_eq!(cert.valid_from(), "261016185536Z", "notBefore mismatch");
    assert_eq!(cert.valid_to(), "361013185536Z", "notAfter mismatch");
}

/// Serial is the fixture's openssl-issued serial, uppercased hex without a
/// leading `0x` (BN_bn2hex lowercases; the accessor uppercases).
#[test]
fn serial_number_matches_fixture() {
    let cert = X509Certificate::from_pem(CERT_PEM).expect("fixture PEM must parse");
    assert_eq!(
        cert.serial_number(),
        "78CF79A96A89FA6CCBC05F2A006181F30B983556",
        "serial mismatch"
    );
}

/// SHA-256 fingerprint: 32 bytes as `XX:…` (95 chars), uppercase, and equal
/// to an independent digest of the fixture DER (the digest oracle — proves
/// `der_bytes` is the real certificate, not a re-encoding artifact).
#[test]
fn sha256_fingerprint_is_colon_hex_and_matches_der_digest() {
    let cert = X509Certificate::from_pem(CERT_PEM).expect("fixture PEM must parse");
    let fp = cert.fingerprint_sha256();

    let parts: Vec<&str> = fp.split(':').collect();
    assert_eq!(parts.len(), 32, "sha256 fingerprint must have 32 bytes: {fp}");
    assert_eq!(fp.len(), 32 * 3 - 1, "sha256 fingerprint length: {fp}");
    for p in &parts {
        assert_eq!(p.len(), 2, "fingerprint byte must be 2 hex digits: {p}");
        assert!(
            p.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase()),
            "fingerprint must be uppercase hex: {p}"
        );
    }

    // Independent oracle: openssl dgst -sha256 over the DER fixture.
    let expected = {
        use std::process::Command;
        let der = cert_der();
        let out = Command::new("openssl")
            .args(["dgst", "-sha256", "-r"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(&der)?;
                let o = child.wait_with_output()?;
                Ok(o)
            })
            .expect("openssl digest oracle");
        let s = String::from_utf8(out.stdout).expect("utf8");
        // `"<hex> *stdin"` → take the hex field, colon-separate, uppercase.
        let hex = s.split_whitespace().next().expect("hex field");
        hex.chars()
            .collect::<Vec<_>>()
            .chunks(2)
            .map(|c| c.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join(":")
            .to_uppercase()
    };
    assert_eq!(fp, expected, "fingerprint must equal openssl's DER digest");
}

/// SHA-1 fingerprint: 20 bytes, and distinct from the SHA-256 one (guards
/// against both accessors collapsing onto the same digest).
#[test]
fn sha1_fingerprint_is_20_bytes_and_distinct_from_sha256() {
    let cert = X509Certificate::from_pem(CERT_PEM).expect("fixture PEM must parse");
    let fp1 = cert.fingerprint_sha1();
    let fp256 = cert.fingerprint_sha256();

    assert_eq!(fp1.split(':').count(), 20, "sha1 fingerprint must have 20 bytes: {fp1}");
    assert_eq!(fp1.len(), 20 * 3 - 1, "sha1 fingerprint length: {fp1}");
    assert_ne!(fp1, fp256, "sha1 and sha256 fingerprints must differ");
}

/// Parsing the same cert twice must produce identical fingerprints
/// (deterministic re-encode: `encode_der` round-trips the same bytes).
#[test]
fn repeated_parses_are_deterministic() {
    let a = X509Certificate::from_pem(CERT_PEM).expect("first parse");
    let b = X509Certificate::from_pem(CERT_PEM).expect("second parse");
    assert_eq!(a.fingerprint_sha256(), b.fingerprint_sha256());
    assert_eq!(a.serial_number(), b.serial_number());
}

/// DER constructor on the literal fixture DER must agree with the PEM
/// constructor on every accessor — the two parse faces observe the same
/// certificate (the digest oracle anchors `der_bytes` identity).
#[test]
fn from_der_agrees_with_from_pem_on_every_field() {
    let pem_cert = X509Certificate::from_pem(CERT_PEM).expect("PEM parse");
    let der_cert = X509Certificate::from_der(&cert_der()).expect("DER parse");

    assert_eq!(der_cert.subject(), pem_cert.subject());
    assert_eq!(der_cert.issuer(), pem_cert.issuer());
    assert_eq!(der_cert.serial_number(), pem_cert.serial_number());
    assert_eq!(der_cert.valid_from(), pem_cert.valid_from());
    assert_eq!(der_cert.valid_to(), pem_cert.valid_to());
    assert_eq!(der_cert.fingerprint_sha256(), pem_cert.fingerprint_sha256());
    assert_eq!(der_cert.fingerprint_sha1(), pem_cert.fingerprint_sha1());
}

// ─── Error paths ───────────────────────────────────────────────────────────

/// Garbage text is not a PEM cert: `PEM_read_bio_X509` fails →
/// `InvalidCertificate("PEM_read_bio_X509 failed")`.
#[test]
fn from_pem_garbage_is_invalid_certificate() {
    let err = expect_parse_failure(
        X509Certificate::from_pem("this is not a certificate at all"),
        "garbage PEM",
    );
    assert_is_invalid_certificate(err, "garbage PEM");
}

/// A well-formed PEM block that is NOT a certificate (a PRIVATE KEY) must
/// fail the X509 read, not panic or misparse.
#[test]
fn from_pem_non_certificate_block_fails() {
    let key_pem = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIIa9\n-----END PRIVATE KEY-----\n";
    let err = expect_parse_failure(
        X509Certificate::from_pem(key_pem),
        "non-cert PEM",
    );
    assert_is_invalid_certificate(err, "non-cert PEM");
}

/// Empty input has no PEM block at all.
#[test]
fn from_pem_empty_input_fails() {
    let err = expect_parse_failure(X509Certificate::from_pem(""), "empty PEM");
    assert_is_invalid_certificate(err, "empty PEM");
}

/// Truncated DER (valid structure header, cut mid-body) must fail the
/// d2i_X509 parse.
#[test]
fn from_der_truncated_fails() {
    let der = cert_der();
    let err = expect_parse_failure(
        X509Certificate::from_der(&der[..der.len() / 2]),
        "truncated DER",
    );
    assert_is_invalid_certificate(err, "truncated DER");
}

/// Random bytes with no ASN.1 structure must fail.
#[test]
fn from_der_garbage_fails() {
    let err = expect_parse_failure(
        X509Certificate::from_der(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0x02, 0x03]),
        "garbage DER",
    );
    assert_is_invalid_certificate(err, "garbage DER");
}

/// Empty DER buffer must fail rather than produce a certificate.
#[test]
fn from_der_empty_fails() {
    let err = expect_parse_failure(X509Certificate::from_der(&[]), "empty DER");
    assert_is_invalid_certificate(err, "empty DER");
}

// ─── Lifetime ──────────────────────────────────────────────────────────────

/// Drop must run `X509_free` exactly once per handle — parse and drop in a
/// scope, then re-parse to prove the allocator/X509 state is undamaged
/// (double-free or leak here aborts or trips the leak sanitizer, not this
/// assertion; the pass condition is clean teardown between parses).
#[test]
fn drop_then_reparse_is_clean() {
    let fp = {
        let cert = X509Certificate::from_pem(CERT_PEM).expect("parse");
        cert.fingerprint_sha256()
        // `cert` dropped here → X509_free
    };
    let again = X509Certificate::from_pem(CERT_PEM).expect("reparse after drop");
    assert_eq!(again.fingerprint_sha256(), fp);

    // Interleave error-path drops (BIO allocated then freed inside from_pem)
    // with success drops — the BIO_free/X509_free pairing must hold across
    // both shapes.
    let _ = expect_parse_failure(X509Certificate::from_pem("nope"), "error drop");
    let _ = expect_parse_failure(X509Certificate::from_der(&[0u8; 8]), "error drop");
    let third = X509Certificate::from_pem(CERT_PEM).expect("parse after error drops");
    assert_eq!(third.fingerprint_sha256(), fp);
}

/// Multiple simultaneously-live handles over the same PEM: independent
/// X509 objects, no shared-state aliasing (each parse owns its own).
#[test]
fn concurrent_handles_are_independent() {
    let a = X509Certificate::from_pem(CERT_PEM).expect("a");
    let b = X509Certificate::from_pem(CERT_PEM).expect("b");
    let c = X509Certificate::from_der(&cert_der()).expect("c");
    assert_eq!(a.fingerprint_sha256(), b.fingerprint_sha256());
    assert_eq!(a.fingerprint_sha256(), c.fingerprint_sha256());
    // Drop one while others live.
    drop(a);
    assert_eq!(b.serial_number(), c.serial_number());
}
