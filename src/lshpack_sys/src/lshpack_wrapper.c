/* Thin C wrapper bridging ls-hpack to the lshpack_wrapper_* ABI used by bun_http.
 *
 * Ported from Bun's src/jsc/bindings/c-bindings.cpp (lshpack section).
 * Uses C (not C++) to avoid requiring a C++ compiler for this crate.
 */

#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <stdbool.h>
#include "lshpack.h"
#include "lsxpack_header.h"

/* ── Types ────────────────────────────────────────────────────────────── */

typedef void *(*lshpack_alloc_fn)(size_t);
typedef void (*lshpack_free_fn)(void *);

typedef struct {
    struct lshpack_enc enc;
    struct lshpack_dec dec;
    lshpack_free_fn free_fn;
} lshpack_wrapper;

/* Must match bun_http::lshpack::lshpack_header (#[repr(C)]) */
typedef struct {
    const uint8_t *name;
    size_t name_len;
    const uint8_t *value;
    size_t value_len;
    bool never_index;
    uint16_t hpack_index;
} lshpack_header_t;

#define LSHPACK_MAX_HEADER_SIZE 65536

/* Thread-local shared buffer — reused across encode/decode calls. */
static __thread char *shared_header_buffer_ = NULL;

static char *get_shared_buffer(void)
{
    if (!shared_header_buffer_) {
        shared_header_buffer_ = (char *)malloc(LSHPACK_MAX_HEADER_SIZE);
    }
    return shared_header_buffer_;
}

/* ── Exported functions ───────────────────────────────────────────────── */

/* Returns lshpack_wrapper* which Rust sees as *mut HPACK.
 * The Rust HPACK struct is { self_: *mut c_void } — a single-pointer
 * newtype that is never dereferenced in Rust; it just carries the address. */

void *lshpack_wrapper_init(lshpack_alloc_fn alloc,
                           lshpack_free_fn free_fn,
                           size_t max_capacity)
{
    lshpack_wrapper *w = (lshpack_wrapper *)alloc(sizeof(lshpack_wrapper));
    if (!w)
        return NULL;
    memset(w, 0, sizeof(*w));
    w->free_fn = free_fn;
    if (lshpack_enc_init(&w->enc) != 0) {
        free_fn(w);
        return NULL;
    }
    lshpack_dec_init(&w->dec);
    lshpack_enc_set_max_capacity(&w->enc, (unsigned)max_capacity);
    lshpack_dec_set_max_capacity(&w->dec, (unsigned)max_capacity);
    return w;
}

void lshpack_wrapper_deinit(void *self)
{
    lshpack_wrapper *w = (lshpack_wrapper *)self;
    lshpack_dec_cleanup(&w->dec);
    lshpack_enc_cleanup(&w->enc);
    w->free_fn(w);
}

void lshpack_wrapper_enc_set_max_capacity(void *self, unsigned max_capacity)
{
    lshpack_wrapper *w = (lshpack_wrapper *)self;
    lshpack_enc_set_max_capacity(&w->enc, max_capacity);
}

size_t lshpack_wrapper_decode(void *self,
                              const unsigned char *src,
                              size_t src_len,
                              lshpack_header_t *output)
{
    lshpack_wrapper *w = (lshpack_wrapper *)self;

    lsxpack_header_t hdr;
    memset(&hdr, 0, sizeof(hdr));
    lsxpack_header_prepare_decode(&hdr, get_shared_buffer(), 0,
                                  LSHPACK_MAX_HEADER_SIZE);

    const unsigned char *s = src;
    int rc = lshpack_dec_decode(&w->dec, &s, src + src_len, &hdr);
    if (rc != 0)
        return 0;

    output->name = (const uint8_t *)lsxpack_header_get_name(&hdr);
    output->name_len = hdr.name_len;
    output->value = (const uint8_t *)lsxpack_header_get_value(&hdr);
    output->value_len = hdr.val_len;
    output->never_index = (hdr.flags & LSXPACK_NEVER_INDEX) != 0;

    if (hdr.hpack_index != LSHPACK_HDR_UNKNOWN &&
        hdr.hpack_index <= LSHPACK_HDR_WWW_AUTHENTICATE) {
        output->hpack_index = (uint16_t)(hdr.hpack_index - 1);
    } else {
        output->hpack_index = 255;
    }

    return (size_t)(s - src);
}

size_t lshpack_wrapper_encode(void *self,
                              const unsigned char *name,
                              size_t name_len,
                              const unsigned char *value,
                              size_t value_len,
                              int never_index,
                              unsigned char *buffer,
                              size_t buffer_len,
                              size_t buffer_offset)
{
    lshpack_wrapper *w = (lshpack_wrapper *)self;

    if (name_len + value_len > LSHPACK_MAX_HEADER_SIZE)
        return 0;

    char *shared = get_shared_buffer();
    lsxpack_header_t hdr;
    memset(&hdr, 0, sizeof(hdr));
    memcpy(shared, name, name_len);
    memcpy(shared + name_len, value, value_len);
    lsxpack_header_set_offset2(&hdr, shared, 0, name_len, name_len, value_len);
    if (never_index)
        hdr.indexed_type = 2;

    unsigned char *start = buffer + buffer_offset;
    unsigned char *end = buffer + buffer_len;
    unsigned char *ptr = lshpack_enc_encode(&w->enc, start, end, &hdr);
    if (!ptr)
        return 0;
    return (size_t)(ptr - start);
}
