// SSL no-op stubs for uSockets when compiled without BoringSSL/OpenSSL.
// When ssl_ctx == NULL (plain TCP), these functions are never meaningfully
// called — the callers in context.c / socket.c guard with `if (s->ssl)` or
// `if (ssl_ctx)`.  We provide minimal stubs so the linker is satisfied.
//
// When BoringSSL is linked (Wave 74-TLS), replace this file with
// packages/bun-usockets/src/crypto/openssl.c.

#include "internal/internal.h"
#include "libusockets.h"
#include <stddef.h>

void us_internal_init_loop_ssl_data(struct us_loop_t *loop) { (void)loop; }
void us_internal_free_loop_ssl_data(struct us_loop_t *loop) { (void)loop; }

void us_internal_ssl_ctx_up_ref(struct ssl_ctx_st *p) { (void)p; }
void us_internal_ssl_ctx_unref(struct ssl_ctx_st *p) { (void)p; }

void us_internal_ssl_attach(struct us_socket_t *s, struct ssl_ctx_st *ssl_ctx,
                            int is_client, const char *sni,
                            struct us_listen_socket_t *listener) {
    (void)s; (void)ssl_ctx; (void)is_client; (void)sni; (void)listener;
}

void us_internal_ssl_detach(struct us_socket_t *s) { (void)s; }

struct us_socket_t *us_internal_ssl_close(struct us_socket_t *s, int code, void *reason) {
    (void)s; (void)code; (void)reason; return NULL;
}

struct us_socket_t *us_internal_ssl_on_open(struct us_socket_t *s, int is_client,
                                            char *ip, int ip_length) {
    (void)s; (void)is_client; (void)ip; (void)ip_length; return NULL;
}

struct us_socket_t *us_internal_ssl_on_close(struct us_socket_t *s, int code, void *reason) {
    (void)s; (void)code; (void)reason; return NULL;
}

struct us_socket_t *us_internal_ssl_on_end(struct us_socket_t *s) { (void)s; return NULL; }
struct us_socket_t *us_internal_ssl_on_writable(struct us_socket_t *s) { (void)s; return NULL; }

struct us_socket_t *us_internal_ssl_on_data(struct us_socket_t *s, char *data, int length) {
    (void)s; (void)data; (void)length; return NULL;
}

int us_internal_ssl_is_low_prio(struct us_socket_t *s) { (void)s; return 0; }
int us_internal_ssl_is_shut_down(struct us_socket_t *s) { (void)s; return 0; }
int us_internal_ssl_is_handshake_finished(struct us_socket_t *s) { (void)s; return 0; }
int us_internal_ssl_handshake_callback_has_fired(struct us_socket_t *s) { (void)s; return 0; }

int us_internal_ssl_write(struct us_socket_t *s, const char *data, int length) {
    (void)s; (void)data; (void)length; return 0;
}

void us_internal_ssl_shutdown(struct us_socket_t *s) { (void)s; }
void us_internal_ssl_handshake_abort(struct us_socket_t *s) { (void)s; }

void us_internal_listen_socket_ssl_free(struct us_listen_socket_t *ls) { (void)ls; }

void *us_internal_ssl_get_native_handle(struct us_socket_t *s) {
    (void)s; return NULL;
}

struct us_socket_t *us_socket_adopt_tls(struct us_socket_t *s,
                                         struct us_socket_group_t *group,
                                         unsigned char kind,
                                         struct ssl_ctx_st *ssl_ctx,
                                         const char *sni,
                                         int old_ext_size,
                                         int ext_size) {
    (void)s; (void)group; (void)kind; (void)ssl_ctx; (void)sni;
    (void)old_ext_size; (void)ext_size; return NULL;
}

struct us_bun_verify_error_t us_internal_ssl_verify_error(struct us_socket_t *s) {
    (void)s;
    struct us_bun_verify_error_t err = {0, NULL, NULL};
    return err;
}

void *us_internal_ssl_sni_userdata(struct us_socket_t *s) {
    (void)s; return NULL;
}

void us_socket_start_tls_handshake(struct us_socket_t *s) { (void)s; }

struct us_bun_verify_error_t us_socket_verify_error(struct us_socket_t *s) {
    (void)s;
    struct us_bun_verify_error_t err = {0, NULL, NULL};
    return err;
}

// SNI server-name stubs — libuwsockets.cpp references these unconditionally
// (uWS::TemplatedApp::listen / HttpContext::onData). In plain TCP mode (no
// BoringSSL) they are never meaningfully called: uWS guards with `if (ssl)`.
// Provide no-op stubs so the linker is satisfied; real implementations live
// in crypto/openssl.c (compiled when BAO_UWS_WITH_TLS is set).

int us_listen_socket_add_server_name(struct us_listen_socket_t *ls,
                                     const char *hostname_pattern,
                                     struct ssl_ctx_st *ssl_ctx,
                                     void *user) {
    (void)ls; (void)hostname_pattern; (void)ssl_ctx; (void)user; return 0;
}

void us_listen_socket_on_server_name(struct us_listen_socket_t *ls,
                                     void (*cb)(struct us_listen_socket_t *,
                                                const char *hostname)) {
    (void)ls; (void)cb;
}

void *us_socket_server_name_userdata(us_socket_r s) {
    (void)s; return NULL;
}
