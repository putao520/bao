//! C15 coverage wave — additive `bao_crypto::certificate` faces (REQ-ENG-010
//! TLS 证书解析分支). The C7 `certificate_tests` own the happy/error/drop
//! matrix over one RSA fixture; this file pins the parser-input-shape arms
//! that file leaves open — multi-block PEM (first cert wins), leading prose,
//! CRLF line endings, DER with trailing bytes — plus a second fixture over a
//! different key algorithm (EC P-256) so the parse/digest face is proven
//! key-type-independent.

use bao_crypto::certificate::X509Certificate;
use bao_crypto::CryptoError;

/// Fixture 1: the C7 RSA-2048 self-signed cert (CN=c7cov.local, O=bao-cov),
/// embedded verbatim so the two suites prove the same parser contract.
const RSA_PEM: &str = "-----BEGIN CERTIFICATE-----
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

/// Fixture 2: EC P-256 self-signed cert (openssl req -x509 -newkey ec
/// -pkeyopt ec_paramgen_curve:P-256, CN=c15ec.local, O=bao-c15, generated
/// 2026-10-18 for this suite). Serial `4923331F…A310D`, notBefore
/// 261018051329Z, notAfter 361015051329Z.
const EC_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBpDCCAUugAwIBAgIUSSMzH6HrGcWBUewgx1x2xf8aMQ0wCgYIKoZIzj0EAwIw
KDEUMBIGA1UEAwwLYzE1ZWMubG9jYWwxEDAOBgNVBAoMB2Jhby1jMTUwHhcNMjYx
MDE4MDUxMzI5WhcNMzYxMDE1MDUxMzI5WjAoMRQwEgYDVQQDDAtjMTVlYy5sb2Nh
bDEQMA4GA1UECgwHYmFvLWMxNTBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABFqf
5i0dZp8xaeIKpEKdsUxFGyc+jonMzwZCmT+EWp5ixHiDkCHZ3qOtlxu0UssAwJPD
ygr6/UlKm46us35fzkejUzBRMB0GA1UdDgQWBBQyoe+h/lCjjlw6m8/CpW9d9eMI
ZzAfBgNVHSMEGDAWgBQyoe+h/lCjjlw6m8/CpW9d9eMIZzAPBgNVHRMBAf8EBTAD
AQH/MAoGCCqGSM49BAMCA0cAMEQCIH+HBle7o2/aUMjawn1Qkolcg3YCaaex3Gek
Xvxenp+mAiAaZM/ioYWoPf+fzspGPKleybfFwuYm9TkwYHhpEju6Sg==
-----END CERTIFICATE-----";

/// Fixture 2 in DER form (openssl x509 -outform DER), hex-wrapped like the
/// C7 suite's DER fixture.
const EC_DER_HEX: &str = concat!(
    "308201a43082014ba00302010202144923331fa1eb19c58151ec20c75c76c5ff",
    "1a310d300a06082a8648ce3d04030230283114301206035504030c0b63313565",
    "632e6c6f63616c3110300e060355040a0c0762616f2d633135301e170d323631",
    "3031383035313332395a170d3336313031353035313332395a30283114301206",
    "035504030c0b63313565632e6c6f63616c3110300e060355040a0c0762616f2d",
    "6331353059301306072a8648ce3d020106082a8648ce3d030107034200045a9f",
    "e62d1d669f3169e20aa4429db14c451b273e8e89cccf0642993f845a9e62c478",
    "839021d9dea3ad971bb452cb00c093c3ca0afafd494a9b8eaeb37e5fce47a353",
    "3051301d0603551d0e0416041432a1efa1fe50a38e5c3a9bcfc2a56f5df5e308",
    "67301f0603551d2304183016801432a1efa1fe50a38e5c3a9bcfc2a56f5df5e3",
    "0867300f0603551d130101ff040530030101ff300a06082a8648ce3d04030203",
    "4700304402207f870657bba36fda50c8dac27d5092895c83760269a7b1dc67a4",
    "5efc5e9e9fa602201a64cfe2a185a83dff9fceca463ca95ec9b7c5c2e626f539",
    "30607869123bba4a",
);

/// Independent digest oracle over fixture 2's DER (openssl
/// `x509 -outform DER | sha256sum` at fixture-generation time).
const EC_SHA256_FINGERPRINT: &str = "71:A5:79:65:28:A2:5C:88:94:74:F6:AC:B2:63:19:7F:8B:FE:D5:94:48:1A:27:7E:3B:0E:0D:44:8A:97:69:E0";

fn ec_der() -> Vec<u8> {
    (0..EC_DER_HEX.len() / 2)
        .map(|i| u8::from_str_radix(&EC_DER_HEX[i * 2..i * 2 + 2], 16).expect("hex digit"))
        .collect()
}

fn expect_failure(r: Result<X509Certificate, CryptoError>, marker: &str) {
    let Err(e) = r else {
        panic!("{marker}: expected parse failure, got a certificate");
    };
    let CryptoError::InvalidCertificate(msg) = e else {
        panic!("{marker}: expected InvalidCertificate, got {e:?}");
    };
    assert!(!msg.is_empty(), "{marker}: message must be non-empty");
}

// ─── parser input shapes ───────────────────────────────────────────────────

/// A PEM stream with two certificate blocks parses the FIRST one
/// (`PEM_read_bio_X509` reads a single object): the returned cert is the
/// RSA fixture, not the trailing EC one.
#[test]
fn pem_with_two_blocks_parses_the_first_c15() {
    let both = format!("{RSA_PEM}\n{EC_PEM}");
    let cert = X509Certificate::from_pem(&both).expect("multi-block PEM must parse");
    assert_eq!(
        cert.serial_number(),
        "78CF79A96A89FA6CCBC05F2A006181F30B983556",
        "first block (RSA fixture) wins"
    );
    assert!(cert.subject().contains("c7cov.local"), "first block's subject");
    assert!(!cert.subject().contains("c15ec.local"), "second block must not win");
}

/// Leading prose before the BEGIN line is skipped by the PEM scanner
/// (certificates pasted into text bodies / MIME parts are the norm).
#[test]
fn pem_with_leading_prose_still_parses_c15() {
    let decorated = format!(
        "Some server dumped a certificate below.\nDate: 2026-10-18\n\n{RSA_PEM}"
    );
    let cert = X509Certificate::from_pem(&decorated).expect("leading prose must be skipped");
    assert_eq!(
        cert.serial_number(),
        "78CF79A96A89FA6CCBC05F2A006181F30B983556",
        "the decorated stream yields the same certificate"
    );
}

/// CRLF line endings (PEM as stored on Windows / in MIME) parse identically:
/// same serial, same validity, same fingerprints.
#[test]
fn pem_with_crlf_line_endings_parses_identically_c15() {
    let crlf = RSA_PEM.replace('\n', "\r\n");
    let lf = X509Certificate::from_pem(RSA_PEM).expect("LF parse");
    let crlf_cert = X509Certificate::from_pem(&crlf).expect("CRLF parse");
    assert_eq!(crlf_cert.serial_number(), lf.serial_number());
    assert_eq!(crlf_cert.fingerprint_sha256(), lf.fingerprint_sha256());
    assert_eq!(crlf_cert.fingerprint_sha1(), lf.fingerprint_sha1());
    assert_eq!(crlf_cert.valid_from(), lf.valid_from());
}

/// DER with trailing bytes after the certificate: `d2i_X509` parses the
/// leading object and tolerates (does not require consuming) the tail — the
/// parsed certificate's identity fields are identical. REGISTERED DIVERGENCE
/// (C15 登记项,不修): `from_der` fingerprints digest the RAW INPUT BUFFER
/// (`der_bytes: der.to_vec()`), so a DER-with-tail parse reports a different
/// fingerprint than the same certificate's PEM parse (which re-encodes via
/// i2d). node's X509Certificate.fingerprint digests the canonical DER — this
/// asymmetry is a product-side edge-case gap, pinned here as observed.
#[test]
fn der_with_trailing_bytes_parses_the_head_c15() {
    let der = ec_der();
    let head = X509Certificate::from_der(&der).expect("DER head parses");
    // Unpadded DER: fingerprint equals the openssl digest oracle.
    assert_eq!(head.fingerprint_sha256(), EC_SHA256_FINGERPRINT);

    let mut padded = der.clone();
    padded.extend_from_slice(b"\xde\xad\xbe\xef-garbage-not-asn1");
    let parsed = X509Certificate::from_der(&padded).expect("padded DER parses the head");

    // The parsed certificate identity is unaffected by the tail.
    assert_eq!(parsed.subject(), head.subject());
    assert_eq!(parsed.issuer(), head.issuer());
    assert_eq!(parsed.serial_number(), head.serial_number());
    assert_eq!(parsed.valid_from(), head.valid_from());
    assert_eq!(parsed.valid_to(), head.valid_to());

    // REGISTERED: the fingerprint digests the raw (padded) buffer, so it
    // diverges from both the canonical oracle and the PEM-parse fingerprint.
    let pem_cert = X509Certificate::from_pem(EC_PEM).expect("PEM parse");
    assert_ne!(
        parsed.fingerprint_sha256(),
        EC_SHA256_FINGERPRINT,
        "observed behavior: from_der fingerprint includes trailing bytes"
    );
    assert_ne!(
        parsed.fingerprint_sha256(),
        pem_cert.fingerprint_sha256(),
        "observed behavior: from_der(tail) fingerprint != from_pem fingerprint"
    );
}

/// A DER buffer cut INSIDE the DER parses fine when the cut lands after the
/// signature only if it is a complete object — a one-byte-short DER of the
/// EC fixture must still fail (boundary against the trailing-bytes tolerance
/// above: trailing data is tolerated, missing data is not).
#[test]
fn der_one_byte_short_still_fails_c15() {
    let der = ec_der();
    expect_failure(
        X509Certificate::from_der(&der[..der.len() - 1]),
        "DER truncated by one byte",
    );
}

// ─── EC fixture contract (key-type independence) ───────────────────────────

/// The EC fixture parses with the same field contract as the RSA one: serial
/// uppercased hex, ASN.1 UTCTime validity strings, self-signed issuer, and a
/// SHA-256 fingerprint equal to the openssl digest of its DER.
#[test]
fn ec_p256_fixture_field_contract_c15() {
    let cert = X509Certificate::from_pem(EC_PEM).expect("EC fixture must parse");
    assert_eq!(
        cert.serial_number(),
        "4923331FA1EB19C58151EC20C75C76C5FF1A310D",
        "EC fixture serial"
    );
    let subject = cert.subject();
    assert!(subject.contains("c15ec.local"), "subject: {subject:?}");
    assert!(subject.contains("bao-c15"), "O=: {subject:?}");
    assert_eq!(cert.issuer(), subject, "self-signed");
    assert_eq!(cert.valid_from(), "261018051329Z");
    assert_eq!(cert.valid_to(), "361015051329Z");
    assert_eq!(
        cert.fingerprint_sha256(),
        EC_SHA256_FINGERPRINT,
        "fingerprint must equal the openssl DER digest oracle"
    );
    assert_eq!(cert.fingerprint_sha1().split(':').count(), 20);

    // The DER constructor must land on the identical certificate.
    let der_cert = X509Certificate::from_der(&ec_der()).expect("EC DER parse");
    assert_eq!(der_cert.serial_number(), cert.serial_number());
    assert_eq!(der_cert.fingerprint_sha256(), cert.fingerprint_sha256());
    assert_eq!(der_cert.fingerprint_sha1(), cert.fingerprint_sha1());
}
