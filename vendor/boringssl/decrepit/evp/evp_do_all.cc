// Copyright 2016 The BoringSSL Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#include <openssl/evp.h>

#include <openssl/cipher.h>
#include <openssl/digest.h>


void EVP_CIPHER_do_all_sorted(void (*callback)(const EVP_CIPHER *cipher,
                                               const char *name,
                                               const char *unused, void *arg),
                              void *arg) {
  // Return only lowercase names to match Node.js behavior.
  // Cipher lookups are case-insensitive, so uppercase names are not needed.
  callback(EVP_aes_128_cbc(), "aes-128-cbc", nullptr, arg);
  callback(EVP_aes_128_cfb128(), "aes-128-cfb", nullptr, arg);
  callback(EVP_aes_128_ctr(), "aes-128-ctr", nullptr, arg);
  callback(EVP_aes_128_ecb(), "aes-128-ecb", nullptr, arg);
  callback(EVP_aes_128_gcm(), "aes-128-gcm", nullptr, arg);
  callback(EVP_aes_128_ofb(), "aes-128-ofb", nullptr, arg);
  callback(EVP_aes_192_cbc(), "aes-192-cbc", nullptr, arg);
  callback(EVP_aes_192_ctr(), "aes-192-ctr", nullptr, arg);
  callback(EVP_aes_192_ecb(), "aes-192-ecb", nullptr, arg);
  callback(EVP_aes_192_gcm(), "aes-192-gcm", nullptr, arg);
  callback(EVP_aes_192_ofb(), "aes-192-ofb", nullptr, arg);
  callback(EVP_aes_256_cbc(), "aes-256-cbc", nullptr, arg);
  callback(EVP_aes_256_cfb128(), "aes-256-cfb", nullptr, arg);
  callback(EVP_aes_256_ctr(), "aes-256-ctr", nullptr, arg);
  callback(EVP_aes_256_ecb(), "aes-256-ecb", nullptr, arg);
  callback(EVP_aes_256_gcm(), "aes-256-gcm", nullptr, arg);
  callback(EVP_aes_256_ofb(), "aes-256-ofb", nullptr, arg);
  callback(EVP_bf_cbc(), "bf-cbc", nullptr, arg);
  callback(EVP_bf_cfb(), "bf-cfb", nullptr, arg);
  callback(EVP_bf_ecb(), "bf-ecb", nullptr, arg);
  callback(EVP_des_cbc(), "des-cbc", nullptr, arg);
  callback(EVP_des_ecb(), "des-ecb", nullptr, arg);
  callback(EVP_des_ede(), "des-ede", nullptr, arg);
  callback(EVP_des_ede_cbc(), "des-ede-cbc", nullptr, arg);
  callback(EVP_des_ede3(), "des-ede3", nullptr, arg);
  callback(EVP_des_ede3_cbc(), "des-ede3-cbc", nullptr, arg);
  callback(EVP_rc2_cbc(), "rc2-cbc", nullptr, arg);
  callback(EVP_rc4(), "rc4", nullptr, arg);
}

void EVP_MD_do_all_sorted(void (*callback)(const EVP_MD *md,
                                           const char *name, const char *unused,
                                           void *arg),
                          void *arg) {
  // Return only lowercase names to match Node.js behavior.
  // Digest lookups are case-insensitive, so uppercase names are not needed.
  callback(EVP_md4(), "md4", nullptr, arg);
  callback(EVP_md5(), "md5", nullptr, arg);
  callback(EVP_ripemd160(), "ripemd160", nullptr, arg);
  callback(EVP_sha1(), "sha1", nullptr, arg);
  callback(EVP_sha224(), "sha224", nullptr, arg);
  callback(EVP_sha256(), "sha256", nullptr, arg);
  callback(EVP_sha384(), "sha384", nullptr, arg);
  callback(EVP_sha512(), "sha512", nullptr, arg);
  callback(EVP_sha512_224(), "sha512-224", nullptr, arg);
  callback(EVP_sha512_256(), "sha512-256", nullptr, arg);
  callback(EVP_sha3_224(), "sha3-224", nullptr, arg);
  callback(EVP_sha3_256(), "sha3-256", nullptr, arg);
  callback(EVP_sha3_384(), "sha3-384", nullptr, arg);
  callback(EVP_sha3_512(), "sha3-512", nullptr, arg);
}

void EVP_MD_do_all(void (*callback)(const EVP_MD *md, const char *name,
                                    const char *unused, void *arg),
                   void *arg) {
  EVP_MD_do_all_sorted(callback, arg);
}

void EVP_MD_do_all_provided(
    OSSL_LIB_CTX *libctx, void (*callback)(EVP_MD *md, void *arg), void *arg) {
  callback(const_cast<EVP_MD *>(EVP_md4()), arg);
  callback(const_cast<EVP_MD *>(EVP_md5()), arg);
  callback(const_cast<EVP_MD *>(EVP_sha1()), arg);
  callback(const_cast<EVP_MD *>(EVP_sha224()), arg);
  callback(const_cast<EVP_MD *>(EVP_sha256()), arg);
  callback(const_cast<EVP_MD *>(EVP_sha384()), arg);
  callback(const_cast<EVP_MD *>(EVP_sha512()), arg);
  callback(const_cast<EVP_MD *>(EVP_sha512_256()), arg);
}
