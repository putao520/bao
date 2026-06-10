use core::ffi::c_uint;

use crate::sha::evp::Algorithm;
use crate::sha::ffi;

/// Derive key material using PBKDF2-HMAC.
///
/// Returns `true` on success, `false` on BoringSSL error.
/// The `output` buffer is filled with `output.len()` derived bytes.
pub fn derive(
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    algorithm: Algorithm,
    output: &mut [u8],
) -> bool {
    let Some(digest) = algorithm.md() else {
        return false;
    };
    // SAFETY: password/salt/output are valid slices; digest is a static EVP_MD singleton.
    let rc = unsafe {
        ffi::PKCS5_PBKDF2_HMAC(
            if password.is_empty() { core::ptr::null() } else { password.as_ptr() },
            password.len(),
            salt.as_ptr(),
            salt.len(),
            iterations as c_uint,
            digest,
            output.len(),
            output.as_mut_ptr(),
        )
    };
    rc > 0
}