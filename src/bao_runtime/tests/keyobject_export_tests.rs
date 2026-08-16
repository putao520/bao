// @trace TEST-ENG-007-KEYOBJECT [req:REQ-ENG-007] [level:integration]
// KeyObject.export({type, format}) — REAL serialization matrix against
// openssl-generated keys (interop both directions):
//
//   public  : spki (default) / pkcs1(RSA)  × pem(default)/der
//   private : pkcs8 (default) / pkcs1(RSA) / sec1(EC) × pem/der
//   secret  : raw Buffer, non-destructive
//
// Pre-fix state (silent-fake class): export() ignored the options argument,
// returned whatever bytes were stored, and CONSUMED the slot (second export
// → undefined); createSecretKey returned a plain object whose `export` was a
// hex STRING property (not callable, secret leaked as a property).
//
// openssl CLI generates the ground-truth keys and re-parses every export
// (pkey / rsa / pkey -pubin, PEM and DER) — a fake export cannot pass.

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

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
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Serialize concurrent JSEngine init across tests in this binary (same
/// pattern as conformance_common).
fn engine_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn make_ctx() -> JsContext {
    let _guard = engine_lock().lock().unwrap_or_else(|e| e.into_inner());
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bao-ko-export-{}-{}", tag, std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn openssl_ok(args: &[&str]) -> bool {
    let out = Command::new("openssl").args(args).output();
    match out {
        Ok(o) => {
            if !o.status.success() {
                eprintln!(
                    "openssl {:?} failed: {}",
                    args,
                    String::from_utf8_lossy(&o.stderr)
                );
            }
            o.status.success()
        }
        Err(e) => {
            eprintln!("openssl spawn failed: {}", e);
            false
        }
    }
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex digit"))
        .collect()
}

/// JS probe bundle shared by the RSA and EC tests: exercise the whole
/// export matrix in JS and stash exported artifacts on globalThis for the
/// Rust side to re-verify with openssl.
const KO_PROBE: &str = r#"
(function() {
  var results = [];
  function check(name, cond) { results.push(name + ':' + (cond ? 'PASS' : 'FAIL')); }
  function hex(b) { var s = ''; for (var i = 0; i < b.length; i++) s += ('0' + b[i].toString(16)).slice(-2); return s; }
  var fs = require('fs');
  var crypto = require('crypto');
  var g = globalThis.__ko = {};

  var privPem = fs.readFileSync(globalThis.__koPrivPem, 'utf8');
  var priv = crypto.createPrivateKey(privPem);
  check('priv-type', priv.type === 'private');
  check('priv-symmetric-false', priv.symmetric === false);
  check('priv-kind', priv.asymmetricKeyType === globalThis.__koKind);

  // default export = pkcs8 PEM ("PRIVATE KEY")
  var pkcs8Pem = priv.export();
  check('default-is-pkcs8-pem', typeof pkcs8Pem === 'string' && pkcs8Pem.indexOf('-----BEGIN PRIVATE KEY-----') === 0 && pkcs8Pem.indexOf('-----END PRIVATE KEY-----') > 0);
  check('explicit-same', priv.export({ type: 'pkcs8', format: 'pem' }) === pkcs8Pem);

  // export is NON-destructive (pre-fix: second call returned undefined)
  check('export-not-consumed', priv.export() === pkcs8Pem);

  // pkcs8 DER + re-import roundtrip (byte-stable)
  var pkcs8DerHex = hex(priv.export({ type: 'pkcs8', format: 'der' }));
  check('pkcs8-der-nonempty', pkcs8DerHex.length > 100);
  var rePriv = crypto.createPrivateKey({ key: Buffer.from(pkcs8DerHex, 'hex'), format: 'der', type: 'pkcs8' });
  check('der-reimport-stable', hex(rePriv.export({ type: 'pkcs8', format: 'der' })) === pkcs8DerHex);
  check('pem-der-cross-consistent', rePriv.export() === pkcs8Pem);

  // type-specific traditional forms
  if (globalThis.__koKind === 'rsa') {
    var pkcs1Pem = priv.export({ type: 'pkcs1', format: 'pem' });
    check('pkcs1-pem', typeof pkcs1Pem === 'string' && pkcs1Pem.indexOf('-----BEGIN RSA PRIVATE KEY-----') === 0);
    var pkcs1DerHex = hex(priv.export({ type: 'pkcs1', format: 'der' }));
    var rePriv1 = crypto.createPrivateKey({ key: Buffer.from(pkcs1DerHex, 'hex'), format: 'der', type: 'pkcs1' });
    check('pkcs1-der-roundtrip', hex(rePriv1.export({ type: 'pkcs1', format: 'der' })) === pkcs1DerHex);
    g.pkcs1Pem = pkcs1Pem;
    g.pkcs1DerHex = pkcs1DerHex;
  } else {
    var sec1Pem = priv.export({ type: 'sec1', format: 'pem' });
    check('sec1-pem', typeof sec1Pem === 'string' && sec1Pem.indexOf('-----BEGIN EC PRIVATE KEY-----') === 0);
    var sec1DerHex = hex(priv.export({ type: 'sec1', format: 'der' }));
    var rePrivS = crypto.createPrivateKey({ key: Buffer.from(sec1DerHex, 'hex'), format: 'der', type: 'sec1' });
    check('sec1-der-roundtrip', hex(rePrivS.export({ type: 'sec1', format: 'der' })) === sec1DerHex);
    g.sec1Pem = sec1Pem;
    g.sec1DerHex = sec1DerHex;
    var threwPkcs1 = false;
    try { priv.export({ type: 'pkcs1', format: 'pem' }); } catch (e) { threwPkcs1 = true; }
    check('ec-pkcs1-throws', threwPkcs1);
  }

  // public derivation + spki forms
  var pub = crypto.createPublicKey(priv);
  check('pub-type', pub.type === 'public');
  check('pub-kind', pub.asymmetricKeyType === globalThis.__koKind);
  var spkiPem = pub.export({ type: 'spki', format: 'pem' });
  check('spki-pem', typeof spkiPem === 'string' && spkiPem.indexOf('-----BEGIN PUBLIC KEY-----') === 0);
  check('pub-default-spki', pub.export() === spkiPem);
  var spkiDerHex = hex(pub.export({ type: 'spki', format: 'der' }));
  var rePub = crypto.createPublicKey({ key: Buffer.from(spkiDerHex, 'hex'), format: 'der', type: 'spki' });
  check('spki-der-roundtrip', hex(rePub.export({ type: 'spki', format: 'der' })) === spkiDerHex);
  g.spkiPem = spkiPem;
  g.spkiDerHex = spkiDerHex;

  // public from the ORIGINAL openssl public PEM string (common Node shape)
  var pubFromPem = crypto.createPublicKey(fs.readFileSync(globalThis.__koPubPem, 'utf8'));
  check('pub-from-pem-string', pubFromPem.type === 'public' && pubFromPem.export() === spkiPem);

  // sign/verify through KeyObjects (canonical storage feeding the real
  // sign/verify surface — key-kind-aware family selection)
  var data = Buffer.from('keyobject-signverify-payload');
  var sig = crypto.sign(globalThis.__koSignAlgo, data, priv);
  check('sig-nonempty', sig && sig.length > 0);
  check('verify-ok', crypto.verify(globalThis.__koSignAlgo, data, pub, sig) === true);
  check('verify-tamper', crypto.verify(globalThis.__koSignAlgo, Buffer.from('keyobject-signverify-payloaX'), pub, sig) === false);

  // fail-closed: a public key cannot become a private KeyObject
  var threwPrivFromPub = false;
  try { crypto.createPrivateKey(spkiPem); } catch (e) { threwPrivFromPub = true; }
  check('priv-from-pub-throws', threwPrivFromPub);

  g.pkcs8Pem = pkcs8Pem;
  g.pkcs8DerHex = pkcs8DerHex;
  return results.join('|');
})()
"#;

/// Rust-side openssl re-verification of the JS-side exported artifacts.
fn openssl_reverify(ctx: &mut JsContext, dir: &std::path::Path, orig_pem: &std::path::Path, kind: &str) {
    let pkcs8_pem = eval_string(ctx, "globalThis.__ko.pkcs8Pem");
    let pkcs8_der_hex = eval_string(ctx, "globalThis.__ko.pkcs8DerHex");
    let spki_pem = eval_string(ctx, "globalThis.__ko.spkiPem");
    let spki_der_hex = eval_string(ctx, "globalThis.__ko.spkiDerHex");
    let (trad_pem, trad_der_hex, trad_name) = if kind == "rsa" {
        (
            eval_string(ctx, "globalThis.__ko.pkcs1Pem"),
            eval_string(ctx, "globalThis.__ko.pkcs1DerHex"),
            "pkcs1",
        )
    } else {
        (
            eval_string(ctx, "globalThis.__ko.sec1Pem"),
            eval_string(ctx, "globalThis.__ko.sec1DerHex"),
            "sec1",
        )
    };
    assert!(!pkcs8_pem.starts_with("ERROR"), "artifact read failed: {}", pkcs8_pem);

    let p = dir.join("pkcs8.pem");
    std::fs::write(&p, pkcs8_pem.as_bytes()).unwrap();
    assert!(
        openssl_ok(&["pkey", "-in", p.to_str().unwrap(), "-noout"]),
        "openssl cannot parse exported pkcs8 PEM ({})",
        kind
    );

    let der_bytes = hex_to_bytes(&pkcs8_der_hex);
    let p = dir.join("pkcs8.der");
    std::fs::write(&p, &der_bytes).unwrap();
    assert!(
        openssl_ok(&["pkey", "-inform", "DER", "-in", p.to_str().unwrap(), "-noout"]),
        "openssl cannot parse exported pkcs8 DER ({})",
        kind
    );

    let p = dir.join("spki.pem");
    std::fs::write(&p, spki_pem.as_bytes()).unwrap();
    assert!(
        openssl_ok(&["pkey", "-pubin", "-in", p.to_str().unwrap(), "-noout"]),
        "openssl cannot parse exported spki PEM ({})",
        kind
    );

    let p = dir.join("spki.der");
    std::fs::write(&p, hex_to_bytes(&spki_der_hex)).unwrap();
    assert!(
        openssl_ok(&[
            "pkey",
            "-pubin",
            "-inform",
            "DER",
            "-in",
            p.to_str().unwrap(),
            "-noout"
        ]),
        "openssl cannot parse exported spki DER ({})",
        kind
    );

    let p = dir.join("trad.pem");
    std::fs::write(&p, trad_pem.as_bytes()).unwrap();
    let trad_pem_ok = if kind == "rsa" {
        openssl_ok(&["rsa", "-in", p.to_str().unwrap(), "-noout"])
    } else {
        openssl_ok(&["ec", "-in", p.to_str().unwrap(), "-noout"])
    };
    assert!(
        trad_pem_ok,
        "openssl cannot parse exported {} PEM ({})",
        trad_name, kind
    );

    let p = dir.join("trad.der");
    std::fs::write(&p, hex_to_bytes(&trad_der_hex)).unwrap();
    let trad_der_ok = if kind == "rsa" {
        openssl_ok(&["rsa", "-inform", "DER", "-in", p.to_str().unwrap(), "-noout"])
    } else {
        openssl_ok(&["ec", "-inform", "DER", "-in", p.to_str().unwrap(), "-noout"])
    };
    assert!(
        trad_der_ok,
        "openssl cannot parse exported {} DER ({})",
        trad_name, kind
    );

    // Strongest interop: openssl's own PKCS#8 DER encoding of the SAME key
    // must be BYTE-IDENTICAL to our pkcs8 DER export (DER is deterministic
    // for the same key material; any divergence means we re-encoded
    // something). NOTE: `openssl pkey -outform DER` writes the TRADITIONAL
    // (PKCS#1/SEC1) DER for RSA — pkcs8 -topk8 -nocrypt is the command that
    // produces the PKCS#8 DER form to compare against.
    let norm = dir.join("openssl_norm.der");
    assert!(
        openssl_ok(&[
            "pkcs8",
            "-topk8",
            "-nocrypt",
            "-in",
            orig_pem.to_str().unwrap(),
            "-outform",
            "DER",
            "-out",
            norm.to_str().unwrap()
        ]),
        "openssl pkcs8 DER normalization failed ({})",
        kind
    );
    let openssl_der = std::fs::read(&norm).unwrap();
    assert_eq!(
        der_bytes, openssl_der,
        "pkcs8 DER export differs from openssl's DER of the same key ({})",
        kind
    );
}

fn run_keyobject_matrix(
    ctx: &mut JsContext,
    priv_pem: &std::path::Path,
    pub_pem: &std::path::Path,
    kind: &str,
    sign_algo: &str,
) {
    let boot = eval_string(
        ctx,
        &format!(
            r#"globalThis.__koPrivPem = {:?}; globalThis.__koPubPem = {:?}; globalThis.__koKind = {:?}; globalThis.__koSignAlgo = {:?}; 'ok'"#,
            priv_pem.to_str().unwrap(),
            pub_pem.to_str().unwrap(),
            kind,
            sign_algo
        ),
    );
    assert_eq!(boot, "ok", "probe boot failed");
    let out = eval_string(ctx, KO_PROBE);
    assert!(
        out.contains(":PASS"),
        "JS probe produced no checks — top-level error: {}",
        out
    );
    let failures: Vec<&str> = out.split('|').filter(|s| !s.contains(":PASS")).collect();
    assert!(
        failures.is_empty(),
        "KeyObject matrix ({}): failing checks:\n  {}\nraw: {}",
        kind,
        failures.join("\n  "),
        out
    );
}

#[test]
fn test_rsa_keyobject_export_matrix_openssl_interop() {
    let dir = tmpdir("rsa");
    let key = dir.join("rsa_key.pem");
    let pub_key = dir.join("rsa_pub.pem");
    assert!(
        openssl_ok(&["genrsa", "-out", key.to_str().unwrap(), "2048"]),
        "openssl genrsa failed"
    );
    assert!(
        openssl_ok(&[
            "rsa",
            "-in",
            key.to_str().unwrap(),
            "-pubout",
            "-out",
            pub_key.to_str().unwrap()
        ]),
        "openssl rsa -pubout failed"
    );

    let mut ctx = make_ctx();
    run_keyobject_matrix(&mut ctx, &key, &pub_key, "rsa", "SHA256");
    openssl_reverify(&mut ctx, &dir, &key, "rsa");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_ec_keyobject_export_matrix_openssl_interop() {
    let dir = tmpdir("ec");
    let key = dir.join("ec_key.pem");
    let pub_key = dir.join("ec_pub.pem");
    assert!(
        openssl_ok(&[
            "ecparam",
            "-name",
            "prime256v1",
            "-genkey",
            "-noout",
            "-out",
            key.to_str().unwrap()
        ]),
        "openssl ecparam failed"
    );
    assert!(
        openssl_ok(&[
            "ec",
            "-in",
            key.to_str().unwrap(),
            "-pubout",
            "-out",
            pub_key.to_str().unwrap()
        ]),
        "openssl ec -pubout failed"
    );

    let mut ctx = make_ctx();
    run_keyobject_matrix(&mut ctx, &key, &pub_key, "ec", "SHA256");
    openssl_reverify(&mut ctx, &dir, &key, "ec");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_secret_keyobject_real_shape() {
    let mut ctx = make_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
(function() {
  var results = [];
  function check(name, cond) { results.push(name + ':' + (cond ? 'PASS' : 'FAIL')); }
  var crypto = require('crypto');
  var k = crypto.createSecretKey(Buffer.from('raw-secret-bytes'));
  check('type', k.type === 'secret');
  check('symmetric', k.symmetric === true);
  check('export-fn', typeof k.export === 'function');
  var e1 = k.export();
  var e2 = k.export();
  check('export-buffer', !!e1 && typeof e1 === 'object' && e1.length === 16);
  check('export-twice', !!e2 && e2.length === 16);   // non-destructive
  var s = ''; for (var i = 0; i < e1.length; i++) s += String.fromCharCode(e1[i]);
  check('export-value', s === 'raw-secret-bytes');
  var kh = crypto.createSecretKey('0011aabb', 'hex');
  var eh = kh.export();
  var hexs = ''; for (var i = 0; i < eh.length; i++) hexs += ('0' + eh[i].toString(16)).slice(-2);
  check('hex-encoding', hexs === '0011aabb');
  var threw = false;
  try { k.export({ format: 'pem' }); } catch (e) { threw = true; }
  check('secret-pem-throws', threw);
  var threwEmpty = false;
  try { crypto.createSecretKey(Buffer.alloc(0)); } catch (e) { threwEmpty = true; }
  check('empty-throws', threwEmpty);
  return results.join('|');
})()
"#,
    );
    let failures: Vec<&str> = out.split('|').filter(|s| !s.contains(":PASS")).collect();
    assert!(
        failures.is_empty(),
        "secret KeyObject failing checks:\n  {}\nraw: {}",
        failures.join("\n  "),
        out
    );
}

#[test]
fn test_invalid_key_shapes_fail_closed() {
    // Garbage key material must throw, never produce a KeyObject.
    let mut ctx = make_ctx();
    let res = eval_string(
        &mut ctx,
        "crypto.createPrivateKey(Buffer.from('this-is-not-a-key')) && 'no-throw'",
    );
    assert!(
        res.starts_with("ERROR:"),
        "garbage key material must fail closed, got: {}",
        res
    );

    // Encrypted PEM import: explicit error (passphrase unsupported), never a
    // silent empty/stripped key.
    let dir = tmpdir("enc");
    let enc_key = dir.join("enc.pem");
    let enc_gen = Command::new("openssl")
        .args([
            "genrsa",
            "-aes128",
            "-passout",
            "pass:testpw",
            "-out",
            enc_key.to_str().unwrap(),
            "2048",
        ])
        .output()
        .expect("openssl genrsa encrypted");
    assert!(enc_gen.status.success(), "openssl encrypted genrsa failed");
    let enc_pem = std::fs::read_to_string(&enc_key).unwrap();
    let boot = eval_string(&mut ctx, &format!("globalThis.__enc = {:?}; 'ok'", enc_pem));
    assert_eq!(boot, "ok");
    let res = eval_string(
        &mut ctx,
        "crypto.createPrivateKey(globalThis.__enc) && 'no-throw'",
    );
    assert!(
        res.starts_with("ERROR:"),
        "encrypted key import must fail closed, got: {}",
        res
    );
    let _ = std::fs::remove_dir_all(&dir);
}
