use crate::sha::ffi;

/// Derive key material using scrypt (RFC 7914).
///
/// `n` must be a power of 2 >= 2. `max_mem` limits memory usage in bytes
/// (default 32 MiB = `32 * 1024 * 1024`).
///
/// Returns `true` on success, `false` on BoringSSL error.
pub fn derive(
    password: &[u8],
    salt: &[u8],
    n: u64,
    r: u64,
    p: u64,
    max_mem: usize,
    output: &mut [u8],
) -> bool {
    // SAFETY: password/salt/output are valid slices; n/r/p/max_mem are passed through.
    let rc = unsafe {
        ffi::EVP_PBE_scrypt(
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            n,
            r,
            p,
            max_mem,
            output.as_mut_ptr(),
            output.len(),
        )
    };
    rc != 0
}

/// Validate scrypt parameters without deriving.
///
/// Returns `true` if parameters are valid.
pub fn validate_params(n: u64, r: u64, p: u64, max_mem: usize) -> bool {
    unsafe {
        ffi::EVP_PBE_validate_scrypt_params(
            core::ptr::null(), 0,
            core::ptr::null(), 0,
            n, r, p, max_mem,
            core::ptr::null_mut(), 0,
        ) != 0
    }
}