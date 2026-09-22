// Build script for bun_libarchive.
//
// unix: system libarchive (`-larchive`, Debian/Ubuntu libarchive-dev face —
// the historical supply).
//
// windows: compile the vendored upstream libarchive (csrc/libarchive,
// oven-sh/bun pin ded82291ab41d5e355831b96b0e1ff49e24d8939 = libarchive
// 3.8.7) with clang-cl per the upstream recipe (oven-sh/bun
// scripts/build/deps/libarchive.ts):
//   sources   = SOURCES + SOURCES_WIN (archive_windows.c family)
//   includes  = libarchive + the zlib ABI face (HAVE_ZLIB_H → gzip filter;
//               symbol supply = libz-rs-sys in the final link, same channel
//               as lsquic_sys)
//   defines   = HAVE_CONFIG_H / LIBARCHIVE_STATIC / _CRT_SECURE_NO_DEPRECATE
//               (no __LIBARCHIVE_ENABLE_VISIBILITY — ELF-only attribute,
//               clang-cl rejects it)
//   cflags    = -Wno-incompatible-pointer-types-discards-qualifiers (upstream
//               bun posture for vendored C)
//   config.h  = the upstream configH(cfg) WINDOWS template, generated into
//               OUT_DIR by this script (cmake's feature pass replaced by the
//               pinned answers — see BAO-WINDOWS.md).
//
// zlib note: the zlib.h/zconf.h ABI face in csrc/zlib is a byte-identical
// mirror of libz-rs-sys 0.6.8's include face (same bytes as lsquic_sys's and
// bun_uws_sys's copies) so the compile-time API face and the final-link
// symbol supplier cannot skew.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Dual-mode (stable ⇄ nightly): nightly-only cfg-gated paths need the
    // channel probe (same template as bun_alloc). Detected from the compiler
    // version string; declared via check-cfg so `unexpected_cfgs` accepts it.
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let is_nightly = std::process::Command::new(&rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|version| version.contains("nightly"))
        .unwrap_or(false);
    if is_nightly {
        println!("cargo:rustc-cfg=bao_nightly");
    }
    println!("cargo:rustc-check-cfg=cfg(bao_nightly)");

    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env != "msvc" {
        // unix: system libarchive (existing supply).
        println!("cargo:rustc-link-lib=archive");
        return;
    }

    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = crate_dir.join("csrc").join("libarchive");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // config.h — the upstream configH(WINDOWS) template, generated into
    // OUT_DIR (archive_platform.h includes it first).
    fs::write(out_dir.join("config.h"), CONFIG_H).unwrap();

    let mut build = cc::Build::new();
    build
        .include(&src)
        .include(&crate_dir.join("csrc").join("zlib"))
        .include(&out_dir)
        .files(SOURCES.iter().map(|f| src.join(format!("{f}.c"))))
        .files(SOURCES_WIN.iter().map(|f| src.join(format!("{f}.c"))))
        .define("HAVE_CONFIG_H", Some("1"))
        .define("LIBARCHIVE_STATIC", Some("1"))
        .define("_CRT_SECURE_NO_DEPRECATE", Some("1"))
        .flag("-Wno-incompatible-pointer-types-discards-qualifiers");

    build.compile("archive");
}

// The upstream SOURCES / SOURCES_WIN lists
// (oven-sh/bun scripts/build/deps/libarchive.ts), verbatim.
const SOURCES: &[&str] = &[
  "archive_acl", "archive_check_magic", "archive_cmdline", "archive_cryptor",
  "archive_digest", "archive_entry", "archive_entry_copy_stat",
  "archive_entry_link_resolver", "archive_entry_sparse", "archive_entry_stat",
  "archive_entry_strmode", "archive_entry_xattr", "archive_hmac", "archive_match",
  "archive_options", "archive_pack_dev", "archive_parse_date", "archive_pathmatch",
  "archive_ppmd8", "archive_ppmd7", "archive_random", "archive_rb", "archive_read",
  "archive_read_add_passphrase", "archive_read_append_filter",
  "archive_read_data_into_fd", "archive_read_disk_entry_from_file",
  "archive_read_disk_posix", "archive_read_disk_set_standard_lookup",
  "archive_read_extract", "archive_read_extract2", "archive_read_open_fd",
  "archive_read_open_file", "archive_read_open_filename", "archive_read_open_memory",
  "archive_read_set_format", "archive_read_set_options",
  "archive_read_support_filter_all", "archive_read_support_filter_by_code",
  "archive_read_support_filter_bzip2", "archive_read_support_filter_compress",
  "archive_read_support_filter_gzip", "archive_read_support_filter_grzip",
  "archive_read_support_filter_lrzip", "archive_read_support_filter_lz4",
  "archive_read_support_filter_lzop", "archive_read_support_filter_none",
  "archive_read_support_filter_program", "archive_read_support_filter_rpm",
  "archive_read_support_filter_uu", "archive_read_support_filter_xz",
  "archive_read_support_filter_zstd", "archive_read_support_format_7zip",
  "archive_read_support_format_all", "archive_read_support_format_ar",
  "archive_read_support_format_by_code", "archive_read_support_format_cab",
  "archive_read_support_format_cpio", "archive_read_support_format_empty",
  "archive_read_support_format_iso9660", "archive_read_support_format_lha",
  "archive_read_support_format_mtree", "archive_read_support_format_rar",
  "archive_read_support_format_rar5", "archive_read_support_format_raw",
  "archive_read_support_format_tar", "archive_read_support_format_warc",
  "archive_read_support_format_xar", "archive_read_support_format_zip",
  "archive_string", "archive_string_sprintf", "archive_time", "archive_util",
  "archive_version_details", "archive_virtual", "archive_write",
  "archive_write_disk_posix", "archive_write_disk_set_standard_lookup",
  "archive_write_open_fd", "archive_write_open_file", "archive_write_open_filename",
  "archive_write_open_memory", "archive_write_add_filter",
  "archive_write_add_filter_b64encode", "archive_write_add_filter_by_name",
  "archive_write_add_filter_bzip2", "archive_write_add_filter_compress",
  "archive_write_add_filter_grzip", "archive_write_add_filter_gzip",
  "archive_write_add_filter_lrzip", "archive_write_add_filter_lz4",
  "archive_write_add_filter_lzop", "archive_write_add_filter_none",
  "archive_write_add_filter_program", "archive_write_add_filter_uuencode",
  "archive_write_add_filter_xz", "archive_write_add_filter_zstd",
  "archive_write_set_format", "archive_write_set_format_7zip",
  "archive_write_set_format_ar", "archive_write_set_format_by_name",
  "archive_write_set_format_cpio", "archive_write_set_format_cpio_binary",
  "archive_write_set_format_cpio_newc", "archive_write_set_format_cpio_odc",
  "archive_write_set_format_filter_by_ext", "archive_write_set_format_gnutar",
  "archive_write_set_format_iso9660", "archive_write_set_format_mtree",
  "archive_write_set_format_pax", "archive_write_set_format_raw",
  "archive_write_set_format_shar", "archive_write_set_format_ustar",
  "archive_write_set_format_v7tar", "archive_write_set_format_warc",
  "archive_write_set_format_xar", "archive_write_set_format_zip",
  "archive_write_set_options", "archive_write_set_passphrase",
  "filter_fork_posix", "xxhash",
  "archive_blake2sp_ref", "archive_blake2s_ref",
];

const SOURCES_WIN: &[&str] = &[
  "archive_entry_copy_bhfi",
  "archive_read_disk_windows",
  "archive_windows",
  "archive_write_disk_windows",
  "filter_fork_windows",
];

const CONFIG_H: &str = r#"/* Generated by build.rs — upstream template: oven-sh/bun scripts/build/deps/libarchive.ts configH(WINDOWS) @ 4af1842c8c */
#define __LIBARCHIVE_CONFIG_H_INCLUDED 1
#define NTDDI_VERSION 0x0A000000
#define _WIN32_WINNT 0x0A00
#define WINVER 0x0A00

#define SIZEOF_SHORT 2
#define SIZEOF_INT 4
#define SIZEOF_LONG 4
#define SIZEOF_LONG_LONG 8
#define SIZEOF_UNSIGNED_SHORT 2
#define SIZEOF_UNSIGNED 4
#define SIZEOF_UNSIGNED_LONG 4
#define SIZEOF_UNSIGNED_LONG_LONG 8
#define SIZEOF_WCHAR_T 2
#define ICONV_CONST

#define LIBARCHIVE_VERSION_NUMBER "3008007"
#define LIBARCHIVE_VERSION_STRING "3.8.7"
#define BSDTAR_VERSION_STRING "3.8.7"
#define BSDCPIO_VERSION_STRING "3.8.7"
#define BSDCAT_VERSION_STRING "3.8.7"
#define BSDUNZIP_VERSION_STRING "3.8.7"
#define VERSION "3.8.7"

#define HAVE_INT16_T 1
#define HAVE_INT32_T 1
#define HAVE_INT64_T 1
#define HAVE_INTMAX_T 1
#define HAVE_UINT8_T 1
#define HAVE_UINT16_T 1
#define HAVE_UINT32_T 1
#define HAVE_UINT64_T 1
#define HAVE_UINTMAX_T 1
#define HAVE_DECL_INT32_MAX 1
#define HAVE_DECL_INT32_MIN 1
#define HAVE_DECL_INT64_MAX 1
#define HAVE_DECL_INT64_MIN 1
#define HAVE_DECL_INTMAX_MAX 1
#define HAVE_DECL_INTMAX_MIN 1
#define HAVE_DECL_SIZE_MAX 1
#define HAVE_DECL_UINT32_MAX 1
#define HAVE_DECL_UINT64_MAX 1
#define HAVE_DECL_UINTMAX_MAX 1
#define HAVE_CTYPE_H 1
#define HAVE_ERRNO_H 1
#define HAVE_FCNTL_H 1
#define HAVE_LIMITS_H 1
#define HAVE_LOCALE_H 1
#define HAVE_SIGNAL_H 1
#define HAVE_STDARG_H 1
#define HAVE_STDINT_H 1
#define HAVE_STDIO_H 1
#define HAVE_STDLIB_H 1
#define HAVE_STRING_H 1
#define HAVE_TIME_H 1
#define HAVE_WCHAR_H 1
#define HAVE_WCTYPE_H 1
#define HAVE_SYS_STAT_H 1
#define HAVE_SYS_TYPES_H 1
#define HAVE_INTTYPES_H 1
#define HAVE_EILSEQ 1
#define HAVE_WCHAR_T 1
#define HAVE_FSTAT 1
#define HAVE_GETPID 1
#define HAVE_MEMMOVE 1
#define HAVE_MEMORY_H 1
#define HAVE_MKDIR 1
#define HAVE_SETLOCALE 1
#define HAVE_STRCHR 1
#define HAVE_STRDUP 1
#define HAVE_STRERROR 1
#define HAVE_STRFTIME 1
#define HAVE_STRNLEN 1
#define HAVE_STRRCHR 1
#define HAVE_TZSET 1
#define HAVE_VPRINTF 1
#define HAVE_WCRTOMB 1
#define HAVE_WCSCMP 1
#define HAVE_WCSCPY 1
#define HAVE_WCSLEN 1
#define HAVE_WCTOMB 1
#define HAVE_WMEMCMP 1
#define HAVE_WMEMCPY 1
#define HAVE_WMEMMOVE 1
#define HAVE_MBRTOWC 1
#define HAVE_ZLIB_H 1

#define HAVE_IO_H 1
#define HAVE_DIRECT_H 1
#define HAVE_PROCESS_H 1
#define HAVE_SYS_UTIME_H 1
#define HAVE_WINDOWS_H 1
#define HAVE_WINCRYPT_H 1
#define HAVE__CTIME64_S 1
#define HAVE__FSEEKI64 1
#define HAVE__GET_TIMEZONE 1
#define HAVE__GMTIME64_S 1
#define HAVE__LOCALTIME64_S 1
#define HAVE__MKGMTIME64 1
#define HAVE_STRNCPY_S 1
#define HAVE_WCSCPY_S 1
#define HAVE_WCSNCPY_S 1

/* POSIX type fallbacks — UCRT's <sys/types.h> doesn't define these.
   Values match cmake's WIN32 branch (CMakeLists.txt CHECK_TYPE_SIZE block). */
#define gid_t short
#define uid_t short
#define id_t short
#define mode_t unsigned short
#define pid_t int
#define ssize_t int64_t
"#;
