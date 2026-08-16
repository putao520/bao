// @trace TEST-ENG-007-CRYPTO [req:REQ-ENG-007] [level:integration]
// P0 vector-level regression tests for the two crypto silent-fakes fixed in
// this wave (v-surface audit):
//   1. scryptSync returned all-zero bytes for every input (the KDF result was
//      discarded) and ignored the {N,r,p} options object.
//   2. createSign('sha256')/createVerify with an RSA key silently fell to the
//      HMAC path (bare digest names match no family pattern), so legitimate
//      signatures verified as false.
//
// Vector sources:
//   - scrypt: RFC 7914 sec. 12 test vectors (+ default-args vector computed
//     independently with Python hashlib.scrypt).
//   - RSA / ECDSA interop: fixed keypair + signature produced by `openssl
//     dgst -sha256 -sign` — a real external signer, not self-attestation.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Ok(_) => "[other]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

const RSA_PRIV_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC00RcmTSt4SL3J
x9qPoyzSWRclMcdavnbY98MLtVcS3I24I5ipqnBBh4p0igjVbssX4eud4qt18m9e
MjvQx5cneewgw79tljJX0GySh+xql3Y7O/DK8PHN1AyqLiOqkgzpWmc+D01lPJOw
oT5DCyMbKb6rj5uzbkQBskolPgITkIEdR3ZVStWzNPd33DXpRtgrmEf73cmrC7V7
nUquIQo9hJLANiJTnwOY0b7H7Obs/HVTOHpChXKI3pfjV2mjgIKI3AGrke+UPGm1
IHcLP/0AidueXUacttn7VyEz580ySGA6V1sUPBdpFQsBl3PC0vvGg7qxBieR40DW
x4WwwhgvAgMBAAECggEANsPLgKjH3T8e6IIVEwMnnLAqH/RTPotIgM+N7jpm3Iob
jGWPo/fA10AfsctrAIX1kk61Z9US/H7It118m3AQOn8lgwj2rlDa/5jbgYgUlXY5
c5hkhnryqdYrXdHqsItayMS+V2AYH2z5CHrV2kWBxQTgQKMW1AI2K9NdvKjqxRSx
iezLT43CtHiHLRlZ7InY0HPS4iM+3kNIqGA3/YIWf2E55+Bi2EEe3Zd3p7ct8Sy0
PHDpCn2cgT6Fi9r8ATajXOgP3IEVSNy4us2ww7LSlIl4UXIhkNND8bQ4OZBWcGPi
GIOwWY/CnrSmDIXI+dEKbvzLkDb8awgP1u4dOqg/LQKBgQDYTJYxJxXqTfQxmwue
stsCj6F0okzjJQ7+/5X7UDIvzm0uJkKGf0/2lBGtb6km/uns6aCkW5omHaPmmPtJ
WrAJt8JIgyKQUSIneqTF9XUzK0QQ2cwF4+oBWYUOSlAsWEbf3hrRVSoXfdhHa0gZ
N5mJ/9pLWS5GQt4d6myWuBioJQKBgQDWAUPQ/jCo/58KwCGkUuhGBBzNlduqI6h4
Zwij4Nzk3+Ewfveam9bpzOh/L8UhgsyefYex8+kz6pSygsVrX/0OWd/qTSJiykht
0E8p7pIy8iRbu1VxTj2QHmxgBteNce3QciNxMY/9FqOQKG+JzcQ582YV/F0matOM
hj/2tMC0wwKBgQCGX33m50s8FlWgA5xCaQaaHrTFCpcNfdZFIG8Cg53KCUnWo7os
aCc7Hl2lC3tgWHjmz3UW5jlreHp8JYnm1koKn3g5KA5u7Zh0QkLfIFBBC53rggK3
nhGf6Qc6C3ynL+hH52ltpqTRl0Kni8RsthfSnXn12V9gEuZ+W0Y+k/vtaQKBgHwF
/aR4PAElK6nSUWznM3+oUH0A1W5T/gXRSJuY7Mujx+EQJDUxDasvuqpDKA7Uu/s6
KtMB1WvmDkkqKnmhBoozoeYqz7vLGZCywb4+afImjNWwysLAokMMrqg0LuXlWfqM
u2eVXqpBXYdlN8b4PjmLiuVA/UcPcAynnRhABtJ7AoGAGyBczI4Z2iCLkjcnwNyG
BfIb6tKKfb9iYewKg21osNaLjBbgW2EnehwtFvUqBC4ISXex5uqqQ6Dz8cvqyZtQ
ZHl4R70YRDmC9gF8F/RBA9BXf1zyhZB+mJpPc/ZQlElixR8kiNuiLHO0YSUAgWfJ
sOLsU9OqjfRH/GiT1iEQUSs=
-----END PRIVATE KEY-----";

const RSA_PUB_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAtNEXJk0reEi9ycfaj6Ms
0lkXJTHHWr522PfDC7VXEtyNuCOYqapwQYeKdIoI1W7LF+HrneKrdfJvXjI70MeX
J3nsIMO/bZYyV9Bskofsapd2OzvwyvDxzdQMqi4jqpIM6VpnPg9NZTyTsKE+Qwsj
Gym+q4+bs25EAbJKJT4CE5CBHUd2VUrVszT3d9w16UbYK5hH+93Jqwu1e51KriEK
PYSSwDYiU58DmNG+x+zm7Px1Uzh6QoVyiN6X41dpo4CCiNwBq5HvlDxptSB3Cz/9
AInbnl1GnLbZ+1chM+fNMkhgOldbFDwXaRULAZdzwtL7xoO6sQYnkeNA1seFsMIY
LwIDAQAB
-----END PUBLIC KEY-----";

const EC_P256_PRIV_PEM: &str = "-----BEGIN EC PRIVATE KEY-----
MHcCAQEEID2FAQ3dd2jW54IFrtn7aIiMzr6TizDeT2t5jLtMwvxGoAoGCCqGSM49
AwEHoUQDQgAEel0ncDvImfWbfpE3LK2dKSMiUJ+Bq+Roxac/EqaWyjZ+ggznE7jQ
wD9F8vJasIfRBY7+/4D1bUz5fk/aAHnuVw==
-----END EC PRIVATE KEY-----";

const EC_P256_PUB_PEM: &str = "-----BEGIN PUBLIC KEY-----
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEel0ncDvImfWbfpE3LK2dKSMiUJ+B
q+Roxac/EqaWyjZ+ggznE7jQwD9F8vJasIfRBY7+/4D1bUz5fk/aAHnuVw==
-----END PUBLIC KEY-----";

/// openssl dgst -sha256 -sign over b"bao-sign-verify-interop-vector".
const OPENSSL_RSA_SIG_B64: &str = "Zpr7f4SH4AH93rdOMcpYy2M1lC4/1P14F+kasxahtokbtF1aOAh4pdLre9jeIXbB9splVIxVMciPgw4KlEPCdmoA6L/M/yontyvrkXKNbFt1GY6d+RJPhsn+wsolLacY4Vl6wgjl+KS54LlqRFErCMc+Hbpb2ebjcMWWyqbvsM7Eog0cCskay9+aG3bTCC8z+9DxQZ7axfB/T5K6+0qqIhcyUP6ZXdaiKTsLemjxF/QNfx6bo3z0NnFMeS6Tc0qR9eZcTbRqMEX3MQ0MrcHfQFapQSYN0CTK4CNTMbtLF9s03fiC06pGXc4vvntKtN+o71ib7zuFAyUDA4r4KmPmGg==";
/// openssl dgst -sha256 -sign (ECDSA P-256, DER) over the same message.
const OPENSSL_EC_SIG_B64: &str = "MEUCIQD7ZlPkZIDCldBw0RFrQ6ZwHiYMRiUh174OJjKC/UcN3QIgPnHA1UzZYagw8cr3sA6OOLyGfUtnbIRl6JNkh/xIeh0=";

#[test]
fn test_scrypt_sync_rfc7914_vectors() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(
        &mut ctx,
        r#"
        var crypto = require('crypto');
        var out = [];
        function hex(b) { var s = ''; for (var i = 0; i < b.length; i++) s += ('0' + b[i].toString(16)).slice(-2); return s; }
        function check(name, fn) {
            try { out.push(name + ':' + (fn() ? 'PASS' : 'FAIL')); }
            catch (e) { out.push(name + ':ERROR:' + (e.message || e)); }
        }

        // RFC 7914 sec.12 vector 1: scrypt("", "", N=16, r=1, p=1, dkLen=64)
        check('rfc_v1', function () {
            return hex(crypto.scryptSync('', '', 64, { N: 16, r: 1, p: 1 }))
                === '77d6576238657b203b19ca42c18a0497f16b4844e3074ae8dfdffa3fede21442fcd0069ded0948f8326a753a0fc81f17e8d3e0fb2e0d3628cf35e20c38d18906';
        });
        // RFC 7914 sec.12 vector 2: N=1024, r=8, p=16
        check('rfc_v2', function () {
            return hex(crypto.scryptSync('password', 'NaCl', 64, { N: 1024, r: 8, p: 16 }))
                === 'fdbabe1c9d3472007856e7190d01e9fe7c6ad7cbc8237830e77376634b3731622eaf30d92e22a3886ff109279d9830dac727afb94a83ee6d8360cbdfa2cc0640';
        });
        // RFC 7914 sec.12 vector 3 (the task-mandated N=16384, r=8, p=1)
        check('rfc_v3', function () {
            return hex(crypto.scryptSync('pleaseletmein', 'SodiumChloride', 64, { N: 16384, r: 8, p: 1 }))
                === '7023bdcb3afd7348461c06cd81fd38ebfda8fbba904f8e3ea9b543f6545da1f2d5432955613f0fcf62d49705242a9af9e61e85dc0d651e40dfcf017b45575887';
        });
        // Default options (N=16384, r=8, p=1) — vector computed with
        // Python hashlib.scrypt, byte-for-byte.
        check('default_opts', function () {
            return hex(crypto.scryptSync('pw', 'salt', 32))
                === 'c0b515908e61334cae6d6003c00be60e2e675878c9ac8ba282e1d70c335d3012';
        });
        // Legacy cost/blocksize/parallelization aliases.
        check('legacy_aliases', function () {
            return hex(crypto.scryptSync('', '', 64, { cost: 16, blocksize: 1, parallelization: 1 }))
                === '77d6576238657b203b19ca42c18a0497f16b4844e3074ae8dfdffa3fede21442fcd0069ded0948f8326a753a0fc81f17e8d3e0fb2e0d3628cf35e20c38d18906';
        });
        // Buffer password/salt must equal string inputs; result is a Buffer.
        check('buffer_inputs', function () {
            var a = crypto.scryptSync(Buffer.from('pw'), Buffer.from('salt'), 32);
            var b = crypto.scryptSync('pw', 'salt', 32);
            return Buffer.isBuffer(a) && Buffer.isBuffer(b) && hex(a) === hex(b)
                && hex(a) === 'c0b515908e61334cae6d6003c00be60e2e675878c9ac8ba282e1d70c335d3012';
        });
        // Not all-zero (the original silent-fake symptom).
        check('nonzero', function () {
            var k = crypto.scryptSync('pw', 'salt', 32);
            var allZero = true;
            for (var i = 0; i < k.length; i++) if (k[i] !== 0) { allZero = false; break; }
            return !allZero;
        });
        out.join('|')
        "#,
    );

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(
        all_passed,
        "scryptSync vector tests should pass. Results: {}",
        results
    );
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_sign_verify_roundtrips_and_interop() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let script = format!(
        r#"
        var crypto = require('crypto');
        var out = [];
        function check(name, fn) {{
            try {{ out.push(name + ':' + (fn() ? 'PASS' : 'FAIL')); }}
            catch (e) {{ out.push(name + ':ERROR:' + (e.message || e)); }}
        }}

        var RSA_PRIV = {rsa_priv:?};
        var RSA_PUB = {rsa_pub:?};
        var EC_PRIV = {ec_priv:?};
        var EC_PUB = {ec_pub:?};
        var MSG = 'bao-sign-verify-interop-vector';

        // The exact probe shape from the audit: createSign('sha256') + RSA key.
        check('legacy_probe_shape', function () {{
            var kp = crypto.generateKeyPairSync('rsa', {{ modulusLength: 2048 }});
            var s = crypto.createSign('sha256'); s.update('legacy');
            var sig = s.sign(kp.privateKey);
            var v = crypto.createVerify('sha256'); v.update('legacy');
            return v.verify(kp.publicKey, sig) === true;
        }});

        // sign with our key, verify against openssl-produced signature for the
        // SAME fixed key — byte-identical RSA PKCS#1 v1.5 is deterministic.
        check('rsa_interop_openssl_sig', function () {{
            var sig = Buffer.from('{rsa_sig}', 'base64');
            var v = crypto.createVerify('sha256'); v.update(MSG);
            return v.verify(RSA_PUB, sig) === true;
        }});
        check('rsa_interop_our_sig', function () {{
            var s = crypto.createSign('sha256'); s.update(MSG);
            var sig = s.sign(RSA_PRIV, 'base64');
            return sig === '{rsa_sig}';
        }});
        check('rsa_tampered_rejected', function () {{
            var sig = Buffer.from('{rsa_sig}', 'base64');
            sig[10] ^= 0xff;
            var v = crypto.createVerify('sha256'); v.update(MSG);
            return v.verify(RSA_PUB, sig) === false;
        }});
        check('rsa_wrong_data_rejected', function () {{
            var sig = Buffer.from('{rsa_sig}', 'base64');
            var v = crypto.createVerify('sha256'); v.update(MSG + 'x');
            return v.verify(RSA_PUB, sig) === false;
        }});

        // ECDSA P-256: openssl signature verifies; our signature verifies too
        // (ECDSA is nondeterministic — verify instead of byte-compare).
        check('ec_interop_openssl_sig', function () {{
            var sig = Buffer.from('{ec_sig}', 'base64');
            var v = crypto.createVerify('sha256'); v.update(MSG);
            return v.verify(EC_PUB, sig) === true;
        }});
        check('ec_interop_our_sig', function () {{
            var s = crypto.createSign('sha256'); s.update(MSG);
            var sig = s.sign(EC_PRIV);
            var v = crypto.createVerify('sha256'); v.update(MSG);
            return v.verify(EC_PUB, sig) === true;
        }});

        // Generated-keypair roundtrips across families.
        check('ec_generated_roundtrip', function () {{
            var kp = crypto.generateKeyPairSync('ec', {{ namedCurve: 'P-256' }});
            var s = crypto.createSign('sha384'); s.update('ec-data');
            var sig = s.sign(kp.privateKey);
            var v = crypto.createVerify('sha384'); v.update('ec-data');
            return v.verify(kp.publicKey, sig) === true;
        }});
        check('ed25519_generated_roundtrip', function () {{
            var kp = crypto.generateKeyPairSync('ed25519');
            var s = crypto.createSign('sha256'); s.update('ed-data');
            var sig = s.sign(kp.privateKey);
            var v = crypto.createVerify('sha256'); v.update('ed-data');
            return v.verify(kp.publicKey, sig) === true;
        }});

        // sign() with no output encoding returns a Buffer (Node contract).
        check('sign_returns_buffer', function () {{
            var s = crypto.createSign('sha256'); s.update(MSG);
            return Buffer.isBuffer(s.sign(RSA_PRIV));
        }});
        // Interleaved Sign instances keep independent state (the shared
        // thread-local corrupted s1 when s2 updated in between).
        check('interleaved_instances', function () {{
            var s1 = crypto.createSign('sha256');
            var s2 = crypto.createSign('sha256');
            s1.update('first');
            s2.update('second');
            var sig1 = s1.sign(RSA_PRIV);
            var v = crypto.createVerify('sha256'); v.update('first');
            return v.verify(RSA_PUB, sig1) === true;
        }});
        out.join('|')
        "#,
        rsa_priv = RSA_PRIV_PEM,
        rsa_pub = RSA_PUB_PEM,
        ec_priv = EC_P256_PRIV_PEM,
        ec_pub = EC_P256_PUB_PEM,
        rsa_sig = OPENSSL_RSA_SIG_B64,
        ec_sig = OPENSSL_EC_SIG_B64,
    );

    let results = eval_string(&mut ctx, &script);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(
        all_passed,
        "createSign/createVerify tests should pass. Results: {}",
        results
    );
    bun_runtime::shutdown_thread_sm();
}
