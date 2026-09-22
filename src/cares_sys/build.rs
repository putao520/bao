// Build script for bun_cares_sys.
//
// unix: system libcares (`-lcares`, the historical supply).
//
// windows: compile the vendored upstream c-ares (csrc, oven-sh/bun pin
// 3ac47ee46edd8ea40370222f91613fc16c434853) with clang-cl per the upstream
// recipe (oven-sh/bun scripts/build/deps/cares.ts):
//   sources  = the full src/lib list (windows_port + the win32 event backend
//              included; the unix event backends self-collapse under the
//              pinned ares_config.h)
//   includes = include / src/lib / src/lib/include + OUT_DIR (the generated
//              ares_config.h / ares_build.h)
//   defines  = HAVE_CONFIG_H / CARES_BUILDING_LIBRARY / CARES_STATICLIB /
//              WIN32_LEAN_AND_MEAN / _CRT_SECURE_NO_DEPRECATE /
//              _CRT_NONSTDC_NO_DEPRECATE
//   cflags   = -D_WIN32_WINNT=0x0602 (hex LITERAL via flag — sdkddkver.h
//              token-pastes `ver##0000`, a decimal define would mangle
//              NTDDI_VERSION)
// The ares_config.h / ares_build.h are the upstream configH(WINDOWS) /
// buildH(WINDOWS) templates, generated into OUT_DIR by this script (cmake's
// ~130 try_compile probes replaced by pinned answers).

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env != "msvc" {
        // unix: system libcares (existing supply).
        println!("cargo:rustc-link-lib=cares");
        return;
    }

    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = crate_dir.join("csrc").join("src-lib");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("ares_config.h"), ARES_CONFIG_H).unwrap();
    fs::write(out_dir.join("ares_build.h"), ARES_BUILD_H).unwrap();

    let mut build = cc::Build::new();
    build
        .include(crate_dir.join("csrc").join("include"))
        .include(&src)
        .include(src.join("include"))
        .include(&out_dir)
        .files(SOURCES.iter().map(|f| src.join(format!("{f}.c"))))
        .define("HAVE_CONFIG_H", Some("1"))
        .define("CARES_BUILDING_LIBRARY", None)
        .define("CARES_STATICLIB", None)
        .define("WIN32_LEAN_AND_MEAN", None)
        .define("_CRT_SECURE_NO_DEPRECATE", None)
        .define("_CRT_NONSTDC_NO_DEPRECATE", None)
        .flag("-D_WIN32_WINNT=0x0602");

    build.compile("cares");
}

const SOURCES: &[&str] = &[
  "ares_addrinfo2hostent", "ares_addrinfo_localhost", "ares_android",
  "ares_cancel", "ares_close_sockets", "ares_conn", "ares_cookie", "ares_data",
  "ares_destroy", "ares_free_hostent", "ares_free_string", "ares_freeaddrinfo",
  "ares_getaddrinfo", "ares_getenv", "ares_gethostbyaddr", "ares_gethostbyname",
  "ares_getnameinfo", "ares_hosts_file", "ares_init", "ares_library_init",
  "ares_metrics", "ares_options", "ares_parse_into_addrinfo", "ares_process",
  "ares_qcache", "ares_query", "ares_search", "ares_send",
  "ares_set_socket_functions", "ares_socket", "ares_sortaddrinfo",
  "ares_strerror", "ares_sysconfig", "ares_sysconfig_files", "ares_sysconfig_mac",
  "ares_sysconfig_win", "ares_timeout", "ares_update_servers", "ares_version",
  "inet_net_pton", "inet_ntop", "windows_port",
  "dsa/ares_array", "dsa/ares_htable", "dsa/ares_htable_asvp",
  "dsa/ares_htable_dict", "dsa/ares_htable_strvp", "dsa/ares_htable_szvp",
  "dsa/ares_htable_vpstr", "dsa/ares_htable_vpvp", "dsa/ares_llist",
  "dsa/ares_slist",
  "event/ares_event_configchg", "event/ares_event_epoll",
  "event/ares_event_kqueue", "event/ares_event_poll", "event/ares_event_select",
  "event/ares_event_thread", "event/ares_event_wake_pipe", "event/ares_event_win32",
  "legacy/ares_create_query", "legacy/ares_expand_name", "legacy/ares_expand_string",
  "legacy/ares_fds", "legacy/ares_getsock", "legacy/ares_parse_a_reply",
  "legacy/ares_parse_aaaa_reply", "legacy/ares_parse_caa_reply",
  "legacy/ares_parse_mx_reply", "legacy/ares_parse_naptr_reply",
  "legacy/ares_parse_ns_reply", "legacy/ares_parse_ptr_reply",
  "legacy/ares_parse_soa_reply", "legacy/ares_parse_srv_reply",
  "legacy/ares_parse_txt_reply", "legacy/ares_parse_uri_reply",
  "record/ares_dns_mapping", "record/ares_dns_multistring", "record/ares_dns_name",
  "record/ares_dns_parse", "record/ares_dns_record", "record/ares_dns_write",
  "str/ares_buf", "str/ares_str", "str/ares_strsplit",
  "util/ares_iface_ips", "util/ares_threads", "util/ares_timeval",
  "util/ares_math", "util/ares_rand", "util/ares_uri",
];

// The upstream configH(WINDOWS) template (oven-sh/bun scripts/build/deps/cares.ts @ 4af1842c8c).
const ARES_CONFIG_H: &str = r#"#define HAVE_ASSERT_H 1
#define HAVE_ERRNO_H 1
#define HAVE_FCNTL_H 1
#define HAVE_INTTYPES_H 1
#define HAVE_LIMITS_H 1
#define HAVE_MEMORY_H 1
#define HAVE_SIGNAL_H 1
#define HAVE_STDBOOL_H 1
#define HAVE_STDINT_H 1
#define HAVE_STDLIB_H 1
#define HAVE_STRING_H 1
#define HAVE_TIME_H 1
#define HAVE_SYS_STAT_H 1
#define HAVE_SYS_TYPES_H 1
#define HAVE_AF_INET6 1
#define HAVE_PF_INET6 1
#define HAVE_LONGLONG 1
#define HAVE_CONNECT 1
#define HAVE_FCNTL 1
#define HAVE_FREEADDRINFO 1
#define HAVE_GETADDRINFO 1
#define HAVE_GETENV 1
#define HAVE_GETHOSTNAME 1
#define HAVE_GETNAMEINFO 1
#define HAVE_RECV 1
#define HAVE_RECVFROM 1
#define HAVE_SEND 1
#define HAVE_SENDTO 1
#define HAVE_SETSOCKOPT 1
#define HAVE_SOCKET 1
#define HAVE_STRDUP 1
#define HAVE_STRNLEN 1
#define HAVE_STAT 1
#define HAVE_STRUCT_ADDRINFO 1
#define HAVE_STRUCT_IN6_ADDR 1
#define HAVE_STRUCT_SOCKADDR_IN6 1
#define HAVE_STRUCT_SOCKADDR_STORAGE 1
#define HAVE_STRUCT_TIMEVAL 1
#define HAVE_STRUCT_SOCKADDR_IN6_SIN6_SCOPE_ID 1
#define CARES_THREADS 1

#define HAVE_IPHLPAPI_H 1
#define HAVE_MSWSOCK_H 1
#define HAVE_NETIOAPI_H 1
#define HAVE_WINDOWS_H 1
#define HAVE_WINSOCK2_H 1
#define HAVE_WS2IPDEF_H 1
#define HAVE_WS2TCPIP_H 1
#define HAVE_IO_H 1
#define HAVE_CLOSESOCKET 1
#define HAVE_CONVERTINTERFACEINDEXTOLUID 1
#define HAVE_CONVERTINTERFACELUIDTONAMEA 1
#define HAVE_GETBESTROUTE2 1
#define HAVE_IF_INDEXTONAME 1
#define HAVE_IF_NAMETOINDEX 1
#define HAVE_INET_NTOP 1
#define HAVE_INET_PTON 1
#define HAVE_IOCTLSOCKET 1
#define HAVE_IOCTLSOCKET_FIONBIO 1
#define HAVE_NOTIFYIPINTERFACECHANGE 1
#define HAVE_REGISTERWAITFORSINGLEOBJECT 1
#define HAVE__STRDUP 1

#define GETHOSTNAME_TYPE_ARG2 int
#define RECVFROM_TYPE_ARG1 SOCKET
#define RECVFROM_TYPE_ARG2 char *
#define RECVFROM_TYPE_ARG2_IS_VOID 0
#define RECVFROM_TYPE_ARG3 int
#define RECVFROM_TYPE_ARG4 int
#define RECVFROM_TYPE_ARG5 struct sockaddr *
#define RECVFROM_TYPE_ARG5_IS_VOID 0
#define RECVFROM_TYPE_ARG6 int *
#define RECVFROM_TYPE_ARG6_IS_VOID 0
#define RECVFROM_TYPE_RETV int
#define RECV_TYPE_ARG1 SOCKET
#define RECV_TYPE_ARG2 char *
#define RECV_TYPE_ARG3 int
#define RECV_TYPE_ARG4 int
#define RECV_TYPE_RETV int
#define SEND_TYPE_ARG1 SOCKET
#define SEND_TYPE_ARG2 const char *
#define SEND_TYPE_ARG3 int
#define SEND_TYPE_ARG4 int
#define SEND_TYPE_RETV int
"#;

// The upstream buildH(WINDOWS) template.
const ARES_BUILD_H: &str = r#"#ifndef __CARES_BUILD_H
#define __CARES_BUILD_H
#define CARES_TYPEOF_ARES_SOCKLEN_T int
#define CARES_TYPEOF_ARES_SSIZE_T __int64
#define CARES_HAVE_WINDOWS_H
#define CARES_HAVE_WS2TCPIP_H
#define CARES_HAVE_WINSOCK2_H
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#endif
"#;
