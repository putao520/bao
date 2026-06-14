use crate::CryptoError;
use bun_boringssl_sys as bssl;
use std::ffi::CStr;
use std::ptr;

pub struct X509Certificate {
    der_bytes: Vec<u8>,
    x509: *mut bssl::X509,
}

impl X509Certificate {
    pub fn from_pem(pem: &str) -> Result<X509Certificate, CryptoError> {
        unsafe {
            let bio = bssl::BIO_new_mem_buf(
                pem.as_ptr() as *const _,
                pem.len() as _,
            );
            if bio.is_null() {
                return Err(CryptoError::InvalidCertificate("BIO_new_mem_buf failed".into()));
            }
            let x509 = bssl::PEM_read_bio_X509(bio, ptr::null_mut(), None, ptr::null_mut());
            bssl::BIO_free(bio);
            if x509.is_null() {
                return Err(CryptoError::InvalidCertificate("PEM_read_bio_X509 failed".into()));
            }
            let der_bytes = encode_der(x509);
            Ok(X509Certificate { der_bytes, x509 })
        }
    }

    pub fn from_der(der: &[u8]) -> Result<X509Certificate, CryptoError> {
        unsafe {
            let mut inp = der.as_ptr();
            let x509 = bssl::d2i_X509(ptr::null_mut(), &mut inp, der.len() as _);
            if x509.is_null() {
                return Err(CryptoError::InvalidCertificate("d2i_X509 failed".into()));
            }
            Ok(X509Certificate {
                der_bytes: der.to_vec(),
                x509,
            })
        }
    }

    pub fn subject(&self) -> String {
        unsafe { x509_name_to_string(bssl::X509_get_subject_name(self.x509)) }
    }

    pub fn issuer(&self) -> String {
        unsafe { x509_name_to_string(bssl::X509_get_issuer_name(self.x509)) }
    }

    pub fn fingerprint_sha256(&self) -> String {
        evp_digest_fingerprint(&self.der_bytes, bssl::EVP_sha256())
    }

    pub fn fingerprint_sha1(&self) -> String {
        evp_digest_fingerprint(&self.der_bytes, bssl::EVP_sha1())
    }

    pub fn valid_from(&self) -> String {
        unsafe { asn1_time_to_string(bssl::X509_get_notBefore(self.x509)) }
    }

    pub fn valid_to(&self) -> String {
        unsafe { asn1_time_to_string(bssl::X509_get_notAfter(self.x509)) }
    }

    pub fn serial_number(&self) -> String {
        unsafe {
            let serial = bssl::X509_get_serialNumber(self.x509);
            if serial.is_null() {
                return String::new();
            }
            let bn = bssl::ASN1_INTEGER_to_BN(serial, ptr::null_mut());
            if bn.is_null() {
                return String::new();
            }
            let hex_ptr = bssl::BN_bn2hex(bn);
            bssl::BN_free(bn);
            if hex_ptr.is_null() {
                return String::new();
            }
            let s = CStr::from_ptr(hex_ptr).to_string_lossy().to_uppercase();
            bssl::OPENSSL_free(hex_ptr as *mut _);
            s
        }
    }
}

impl Drop for X509Certificate {
    fn drop(&mut self) {
        unsafe { bssl::X509_free(self.x509) };
    }
}

fn x509_name_to_string(name: *mut bssl::X509_NAME) -> String {
    if name.is_null() {
        return String::new();
    }
    let mut buf = [0i8; 512];
    let ret = unsafe { bssl::X509_NAME_oneline(name, buf.as_mut_ptr(), buf.len() as _) };
    if ret.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

fn asn1_time_to_string(time: *mut bssl::ASN1_TIME) -> String {
    if time.is_null() {
        return String::new();
    }
    // ASN1_TIME is an ASN1_STRING internally; use its data directly.
    let s = time as *const bssl::asn1_string_st;
    let (len, data) = unsafe { ((*s).length, (*s).data) };
    if len <= 0 || data.is_null() {
        return String::new();
    }
    let slice = unsafe { std::slice::from_raw_parts(data, len as usize) };
    String::from_utf8_lossy(slice).into_owned()
}

fn encode_der(x509: *mut bssl::X509) -> Vec<u8> {
    let len = unsafe { bssl::i2d_X509(x509, ptr::null_mut()) };
    if len <= 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; len as usize];
    let mut outp = buf.as_mut_ptr();
    unsafe { bssl::i2d_X509(x509, &mut outp) };
    buf
}

fn evp_digest_fingerprint(data: &[u8], md: *const bssl::EVP_MD) -> String {
    let mut out = [0u8; bssl::EVP_MAX_MD_SIZE as usize];
    let mut out_len: std::ffi::c_uint = 0;
    let rc = unsafe {
        bssl::EVP_Digest(
            data.as_ptr() as *const _,
            data.len(),
            out.as_mut_ptr(),
            &mut out_len,
            md,
            ptr::null_mut(),
        )
    };
    if rc != 1 {
        return String::new();
    }
    hex_colon(&out[..out_len as usize])
}

fn hex_colon(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(":")
}
