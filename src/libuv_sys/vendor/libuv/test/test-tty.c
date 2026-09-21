/* Copyright Joyent, Inc. and other Node contributors. All rights reserved.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to
 * deal in the Software without restriction, including without limitation the
 * rights to use, copy, modify, merge, publish, distribute, sublicense, and/or
 * sell copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
 * FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
 * IN THE SOFTWARE.
 */

#include "uv.h"
#include "task.h"

#ifdef _WIN32
# include <io.h>
# include <windows.h>
#else /*  Unix */
# include <fcntl.h>
# include <unistd.h>
# if defined(__linux__) && !defined(__ANDROID__)
#  include <pty.h>
# elif defined(__OpenBSD__) || defined(__NetBSD__) || defined(__APPLE__)
#  include <util.h>
# elif defined(__FreeBSD__) || defined(__DragonFly__)
#  include <libutil.h>
# endif
#endif

#include <string.h>
#include <errno.h>


TEST_IMPL(tty) {
  int r, width, height;
  int ttyin_fd, ttyout_fd;
  uv_tty_t tty_in, tty_out;
  uv_loop_t* loop = uv_default_loop();

  /* Make sure we have an FD that refers to a tty */
#ifdef _WIN32
  HANDLE handle;
  handle = CreateFileA("conin$",
                       GENERIC_READ | GENERIC_WRITE,
                       FILE_SHARE_READ | FILE_SHARE_WRITE,
                       NULL,
                       OPEN_EXISTING,
                       FILE_ATTRIBUTE_NORMAL,
                       NULL);
  ASSERT_PTR_NE(handle, INVALID_HANDLE_VALUE);
  ttyin_fd = _open_osfhandle((intptr_t) handle, 0);

  handle = CreateFileA("conout$",
                       GENERIC_READ | GENERIC_WRITE,
                       FILE_SHARE_READ | FILE_SHARE_WRITE,
                       NULL,
                       OPEN_EXISTING,
                       FILE_ATTRIBUTE_NORMAL,
                       NULL);
  ASSERT_PTR_NE(handle, INVALID_HANDLE_VALUE);
  ttyout_fd = _open_osfhandle((intptr_t) handle, 0);

#else /* unix */
  ttyin_fd = open("/dev/tty", O_RDONLY, 0);
  if (ttyin_fd < 0) {
    fprintf(stderr, "Cannot open /dev/tty as read-only: %s\n", strerror(errno));
    fflush(stderr);
    return TEST_SKIP;
  }

  ttyout_fd = open("/dev/tty", O_WRONLY, 0);
  if (ttyout_fd < 0) {
    fprintf(stderr, "Cannot open /dev/tty as write-only: %s\n", strerror(errno));
    fflush(stderr);
    return TEST_SKIP;
  }
#endif

  ASSERT_GE(ttyin_fd, 0);
  ASSERT_GE(ttyout_fd, 0);

  ASSERT_EQ(UV_UNKNOWN_HANDLE, uv_guess_handle(-1));

  ASSERT_EQ(UV_TTY, uv_guess_handle(ttyin_fd));
  ASSERT_EQ(UV_TTY, uv_guess_handle(ttyout_fd));

  r = uv_tty_init(loop, &tty_in, ttyin_fd, 1);  /* Readable. */
  ASSERT_OK(r);
  ASSERT(uv_is_readable((uv_stream_t*) &tty_in));
  ASSERT(!uv_is_writable((uv_stream_t*) &tty_in));

  r = uv_tty_init(loop, &tty_out, ttyout_fd, 0);  /* Writable. */
  ASSERT_OK(r);
  ASSERT(!uv_is_readable((uv_stream_t*) &tty_out));
  ASSERT(uv_is_writable((uv_stream_t*) &tty_out));

  r = uv_tty_get_winsize(&tty_out, &width, &height);
  ASSERT_OK(r);

  printf("width=%d height=%d\n", width, height);

  if (width == 0 && height == 0) {
   /* Some environments such as containers or Jenkins behave like this
    * sometimes */
    MAKE_VALGRIND_HAPPY(loop);
    return TEST_SKIP;
  }

  ASSERT_GT(width, 0);
  ASSERT_GT(height, 0);

  /* Turn on raw mode. */
  r = uv_tty_set_mode(&tty_in, UV_TTY_MODE_RAW);
  ASSERT_OK(r);

  /* Turn off raw mode. */
  r = uv_tty_set_mode(&tty_in, UV_TTY_MODE_NORMAL);
  ASSERT_OK(r);

  /* Calling uv_tty_reset_mode() repeatedly should not clobber errno. */
  errno = 0;
  ASSERT_OK(uv_tty_reset_mode());
  ASSERT_OK(uv_tty_reset_mode());
  ASSERT_OK(uv_tty_reset_mode());
  ASSERT_OK(errno);

  /* TODO check the actual mode! */

  uv_close((uv_handle_t*) &tty_in, NULL);
  uv_close((uv_handle_t*) &tty_out, NULL);

  uv_run(loop, UV_RUN_DEFAULT);

  MAKE_VALGRIND_HAPPY(uv_default_loop());
  return 0;
}


#ifdef _WIN32
static void tty_raw_alloc(uv_handle_t* handle, size_t size, uv_buf_t* buf) {
  buf->base = malloc(size);
  buf->len = size;
}

static void tty_raw_read(uv_stream_t* tty_in, ssize_t nread, const uv_buf_t* buf) {
  if (nread > 0) {
    ASSERT_EQ(1, nread );
    ASSERT_EQ(buf->base[0], ' ');
    uv_close((uv_handle_t*) tty_in, NULL);
  } else {
    ASSERT_OK(nread);
  }
}

TEST_IMPL(tty_raw) {
  int r;
  int ttyin_fd;
  uv_tty_t tty_in;
  uv_loop_t* loop = uv_default_loop();
  HANDLE handle;
  INPUT_RECORD record;
  DWORD written;

  /* Make sure we have an FD that refers to a tty */
  handle = CreateFileA("conin$",
                       GENERIC_READ | GENERIC_WRITE,
                       FILE_SHARE_READ | FILE_SHARE_WRITE,
                       NULL,
                       OPEN_EXISTING,
                       FILE_ATTRIBUTE_NORMAL,
                       NULL);
  ASSERT_PTR_NE(handle, INVALID_HANDLE_VALUE);
  ttyin_fd = _open_osfhandle((intptr_t) handle, 0);
  ASSERT_GE(ttyin_fd, 0);
  ASSERT_EQ(UV_TTY, uv_guess_handle(ttyin_fd));

  r = uv_tty_init(loop, &tty_in, ttyin_fd, 1);  /* Readable. */
  ASSERT_OK(r);
  ASSERT(uv_is_readable((uv_stream_t*) &tty_in));
  ASSERT(!uv_is_writable((uv_stream_t*) &tty_in));

  r = uv_read_start((uv_stream_t*)&tty_in, tty_raw_alloc, tty_raw_read);
  ASSERT_OK(r);

  /* Give uv_tty_line_read_thread time to block on ReadConsoleW */
  Sleep(100);

  /* Turn on raw mode. */
  r = uv_tty_set_mode(&tty_in, UV_TTY_MODE_RAW);
  ASSERT_OK(r);

  /* Write ' ' that should be read in raw mode */
  record.EventType = KEY_EVENT;
  record.Event.KeyEvent.bKeyDown = TRUE;
  record.Event.KeyEvent.wRepeatCount = 1;
  record.Event.KeyEvent.wVirtualKeyCode = VK_SPACE;
  record.Event.KeyEvent.wVirtualScanCode = MapVirtualKeyW(VK_SPACE, MAPVK_VK_TO_VSC);
  record.Event.KeyEvent.uChar.UnicodeChar = L' ';
  record.Event.KeyEvent.dwControlKeyState = 0;
  WriteConsoleInputW(handle, &record, 1, &written);

  uv_run(loop, UV_RUN_DEFAULT);

  MAKE_VALGRIND_HAPPY(loop);
  return 0;
}

TEST_IMPL(tty_empty_write) {
  int r;
  int ttyout_fd;
  uv_tty_t tty_out;
  char dummy[1];
  uv_buf_t bufs[1];
  uv_loop_t* loop;

  /* Make sure we have an FD that refers to a tty */
  HANDLE handle;

  loop = uv_default_loop();

  handle = CreateFileA("conout$",
                       GENERIC_READ | GENERIC_WRITE,
                       FILE_SHARE_READ | FILE_SHARE_WRITE,
                       NULL,
                       OPEN_EXISTING,
                       FILE_ATTRIBUTE_NORMAL,
                       NULL);
  ASSERT_PTR_NE(handle, INVALID_HANDLE_VALUE);
  ttyout_fd = _open_osfhandle((intptr_t) handle, 0);

  ASSERT_GE(ttyout_fd, 0);

  ASSERT_EQ(UV_TTY, uv_guess_handle(ttyout_fd));

  r = uv_tty_init(loop, &tty_out, ttyout_fd, 0);  /* Writable. */
  ASSERT_OK(r);
  ASSERT(!uv_is_readable((uv_stream_t*) &tty_out));
  ASSERT(uv_is_writable((uv_stream_t*) &tty_out));

  bufs[0].len = 0;
  bufs[0].base = &dummy[0];

  r = uv_try_write((uv_stream_t*) &tty_out, bufs, 1);
  ASSERT_OK(r);

  uv_close((uv_handle_t*) &tty_out, NULL);

  uv_run(loop, UV_RUN_DEFAULT);

  MAKE_VALGRIND_HAPPY(loop);
  return 0;
}

TEST_IMPL(tty_large_write) {
  int r;
  int ttyout_fd;
  uv_tty_t tty_out;
  char dummy[10000];
  uv_buf_t bufs[1];
  uv_loop_t* loop;

  /* Make sure we have an FD that refers to a tty */
  HANDLE handle;

  loop = uv_default_loop();

  handle = CreateFileA("conout$",
                       GENERIC_READ | GENERIC_WRITE,
                       FILE_SHARE_READ | FILE_SHARE_WRITE,
                       NULL,
                       OPEN_EXISTING,
                       FILE_ATTRIBUTE_NORMAL,
                       NULL);
  ASSERT_PTR_NE(handle, INVALID_HANDLE_VALUE);
  ttyout_fd = _open_osfhandle((intptr_t) handle, 0);

  ASSERT_GE(ttyout_fd, 0);

  ASSERT_EQ(UV_TTY, uv_guess_handle(ttyout_fd));

  r = uv_tty_init(loop, &tty_out, ttyout_fd, 0);  /* Writable. */
  ASSERT_OK(r);

  memset(dummy, '.', sizeof(dummy) - 1);
  dummy[sizeof(dummy) - 1] = '\n';

  bufs[0] = uv_buf_init(dummy, sizeof(dummy));

  r = uv_try_write((uv_stream_t*) &tty_out, bufs, 1);
  ASSERT_EQ(10000, r);

  uv_close((uv_handle_t*) &tty_out, NULL);

  uv_run(loop, UV_RUN_DEFAULT);

  MAKE_VALGRIND_HAPPY(loop);
  return 0;
}

TEST_IMPL(tty_raw_cancel) {
  int r;
  int ttyin_fd;
  uv_tty_t tty_in;
  HANDLE handle;

  /* Make sure we have an FD that refers to a tty */
  handle = CreateFileA("conin$",
                       GENERIC_READ | GENERIC_WRITE,
                       FILE_SHARE_READ | FILE_SHARE_WRITE,
                       NULL,
                       OPEN_EXISTING,
                       FILE_ATTRIBUTE_NORMAL,
                       NULL);
  ASSERT_PTR_NE(handle, INVALID_HANDLE_VALUE);
  ttyin_fd = _open_osfhandle((intptr_t) handle, 0);
  ASSERT_GE(ttyin_fd, 0);
  ASSERT_EQ(UV_TTY, uv_guess_handle(ttyin_fd));

  r = uv_tty_init(uv_default_loop(), &tty_in, ttyin_fd, 1);  /* Readable. */
  ASSERT_OK(r);
  r = uv_tty_set_mode(&tty_in, UV_TTY_MODE_RAW);
  ASSERT_OK(r);
  r = uv_read_start((uv_stream_t*)&tty_in, tty_raw_alloc, tty_raw_read);
  ASSERT_OK(r);

  r = uv_read_stop((uv_stream_t*) &tty_in);
  ASSERT_OK(r);

  MAKE_VALGRIND_HAPPY(uv_default_loop());
  return 0;
}

/* Sized exactly to the allocation the test hands out, so a regression to
 * the old one-byte NUL overflow lands out of bounds for ASAN to catch. */
static char exact_fill_mem[6];
static int exact_fill_reads;

static void exact_fill_alloc(uv_handle_t* handle,
                             size_t suggested_size,
                             uv_buf_t* buf) {
  /* A 6-byte buffer: exactly two 3-byte UTF-8 characters, the shape that
   * used to overflow (one NUL past the end) and then, after the first fix,
   * delivered a torn character with nread == 6. */
  *buf = uv_buf_init(exact_fill_mem, 6);
}

static void exact_fill_read(uv_stream_t* stream,
                            ssize_t nread,
                            const uv_buf_t* buf) {
  if (nread == 0)
    return;
  ASSERT_GT(nread, 0);
  /* The NUL terminator must stay inside the allocation, so a single read
   * can never fill it completely. */
  ASSERT_LE(nread, 5);
  /* The payload must be complete UTF-8 sequences: no embedded NUL, no
   * character torn at the end of the buffer. */
  {
    ssize_t n = 0;
    while (n < nread) {
      unsigned char c = (unsigned char) buf->base[n];
      int seq;
      ASSERT_NE(0, c);
      if (c < 0x80)
        seq = 1;
      else if ((c >> 5) == 0x6)
        seq = 2;
      else if ((c >> 4) == 0xE)
        seq = 3;
      else if ((c >> 3) == 0x1E)
        seq = 4;
      else
        seq = 0;  /* stray continuation byte = torn character */
      ASSERT_GT(seq, 0);
      ASSERT_LE(n + seq, nread);
      n += seq;
    }
  }
  exact_fill_reads++;
  if (exact_fill_reads == 2)
    uv_read_stop(stream);
}

TEST_IMPL(tty_line_read_exact_fill) {
  int r;
  int ttyin_fd;
  uv_tty_t tty_in;
  uv_loop_t* loop = uv_default_loop();
  HANDLE handle;
  INPUT_RECORD records[3];
  DWORD written;
  int i;

  handle = CreateFileA("conin$",
                       GENERIC_READ | GENERIC_WRITE,
                       FILE_SHARE_READ | FILE_SHARE_WRITE,
                       NULL,
                       OPEN_EXISTING,
                       FILE_ATTRIBUTE_NORMAL,
                       NULL);
  ASSERT_PTR_NE(handle, INVALID_HANDLE_VALUE);
  FlushConsoleInputBuffer(handle);
  ttyin_fd = _open_osfhandle((intptr_t) handle, 0);
  ASSERT_GE(ttyin_fd, 0);
  ASSERT_EQ(UV_TTY, uv_guess_handle(ttyin_fd));

  r = uv_tty_init(loop, &tty_in, ttyin_fd, 1);  /* Readable, line mode. */
  ASSERT_OK(r);

  /* Two copies of U+4E2D (3 UTF-8 bytes each) and ENTER. */
  for (i = 0; i < 3; i++) {
    memset(&records[i], 0, sizeof(records[i]));
    records[i].EventType = KEY_EVENT;
    records[i].Event.KeyEvent.bKeyDown = TRUE;
    records[i].Event.KeyEvent.wRepeatCount = 1;
    records[i].Event.KeyEvent.uChar.UnicodeChar = (WCHAR) ((i < 2) ? 0x4E2D : 0x000D);
  }
  records[2].Event.KeyEvent.wVirtualKeyCode = VK_RETURN;
  records[2].Event.KeyEvent.wVirtualScanCode =
      MapVirtualKeyW(VK_RETURN, MAPVK_VK_TO_VSC);
  ASSERT(WriteConsoleInputW(handle, records, 3, &written));
  ASSERT_EQ(3, written);

  exact_fill_reads = 0;
  r = uv_read_start((uv_stream_t*) &tty_in, exact_fill_alloc, exact_fill_read);
  ASSERT_OK(r);

  uv_run(loop, UV_RUN_DEFAULT);
  ASSERT_GE(exact_fill_reads, 2);

  uv_close((uv_handle_t*) &tty_in, NULL);
  uv_run(loop, UV_RUN_DEFAULT);

  MAKE_VALGRIND_HAPPY(uv_default_loop());
  return 0;
}


static char enobufs_mem[3];
static int enobufs_reads;

static void enobufs_alloc(uv_handle_t* handle,
                          size_t suggested_size,
                          uv_buf_t* buf) {
  /* Too small to hold even one converted 3-byte UTF-8 character. */
  *buf = uv_buf_init(enobufs_mem, 3);
}

static void enobufs_read(uv_stream_t* stream,
                         ssize_t nread,
                         const uv_buf_t* buf) {
  if (nread == 0)
    return;
  /* An allocation that cannot fit one converted character must surface
   * UV_ENOBUFS instead of silently consuming input or tearing bytes. */
  ASSERT_EQ(UV_ENOBUFS, nread);
  enobufs_reads++;
  uv_read_stop(stream);
}

TEST_IMPL(tty_line_read_enobufs) {
  int r;
  int ttyin_fd;
  uv_tty_t tty_in;
  uv_loop_t* loop = uv_default_loop();
  HANDLE handle;
  INPUT_RECORD records[2];
  DWORD written;
  int i;

  handle = CreateFileA("conin$",
                       GENERIC_READ | GENERIC_WRITE,
                       FILE_SHARE_READ | FILE_SHARE_WRITE,
                       NULL,
                       OPEN_EXISTING,
                       FILE_ATTRIBUTE_NORMAL,
                       NULL);
  ASSERT_PTR_NE(handle, INVALID_HANDLE_VALUE);
  FlushConsoleInputBuffer(handle);
  ttyin_fd = _open_osfhandle((intptr_t) handle, 0);
  ASSERT_GE(ttyin_fd, 0);
  ASSERT_EQ(UV_TTY, uv_guess_handle(ttyin_fd));

  r = uv_tty_init(loop, &tty_in, ttyin_fd, 1);  /* Readable, line mode. */
  ASSERT_OK(r);

  /* One U+4E2D (3 UTF-8 bytes, over the 2 usable bytes) and ENTER. */
  for (i = 0; i < 2; i++) {
    memset(&records[i], 0, sizeof(records[i]));
    records[i].EventType = KEY_EVENT;
    records[i].Event.KeyEvent.bKeyDown = TRUE;
    records[i].Event.KeyEvent.wRepeatCount = 1;
    records[i].Event.KeyEvent.uChar.UnicodeChar =
        (WCHAR) ((i == 0) ? 0x4E2D : 0x000D);
  }
  records[1].Event.KeyEvent.wVirtualKeyCode = VK_RETURN;
  records[1].Event.KeyEvent.wVirtualScanCode =
      MapVirtualKeyW(VK_RETURN, MAPVK_VK_TO_VSC);
  ASSERT(WriteConsoleInputW(handle, records, 2, &written));
  ASSERT_EQ(2, written);

  enobufs_reads = 0;
  r = uv_read_start((uv_stream_t*) &tty_in, enobufs_alloc, enobufs_read);
  ASSERT_OK(r);

  uv_run(loop, UV_RUN_DEFAULT);
  ASSERT_EQ(1, enobufs_reads);

  uv_close((uv_handle_t*) &tty_in, NULL);
  uv_run(loop, UV_RUN_DEFAULT);

  MAKE_VALGRIND_HAPPY(uv_default_loop());
  return 0;
}
#endif


TEST_IMPL(tty_file) {
#ifndef _WIN32
  uv_loop_t loop;
  uv_tty_t tty;
  uv_tty_t tty_ro;
  uv_tty_t tty_wo;
  int fd;

  ASSERT_OK(uv_loop_init(&loop));

  fd = open("test/fixtures/empty_file", O_RDONLY);
  if (fd != -1) {
    ASSERT_EQ(UV_EINVAL, uv_tty_init(&loop, &tty, fd, 1));
    ASSERT_OK(close(fd));
    /* test EBADF handling */
    ASSERT_EQ(UV_EINVAL, uv_tty_init(&loop, &tty, fd, 1));
  }

/* Bug on AIX where '/dev/random' returns 1 from isatty() */
#ifndef _AIX
  fd = open("/dev/random", O_RDONLY);
  if (fd != -1) {
    ASSERT_EQ(UV_EINVAL, uv_tty_init(&loop, &tty, fd, 1));
    ASSERT_OK(close(fd));
  }
#endif /* _AIX */

  fd = open("/dev/zero", O_RDONLY);
  if (fd != -1) {
    ASSERT_EQ(UV_EINVAL, uv_tty_init(&loop, &tty, fd, 1));
    ASSERT_OK(close(fd));
  }

  fd = open("/dev/tty", O_RDWR);
  if (fd != -1) {
    ASSERT_OK(uv_tty_init(&loop, &tty, fd, 1));
    ASSERT_OK(close(fd)); /* TODO: it's indeterminate who owns fd now */
    ASSERT(uv_is_readable((uv_stream_t*) &tty));
    ASSERT(uv_is_writable((uv_stream_t*) &tty));
    uv_close((uv_handle_t*) &tty, NULL);
    ASSERT(!uv_is_readable((uv_stream_t*) &tty));
    ASSERT(!uv_is_writable((uv_stream_t*) &tty));
  }

  fd = open("/dev/tty", O_RDONLY);
  if (fd != -1) {
    ASSERT_OK(uv_tty_init(&loop, &tty_ro, fd, 1));
    ASSERT_OK(close(fd)); /* TODO: it's indeterminate who owns fd now */
    ASSERT(uv_is_readable((uv_stream_t*) &tty_ro));
    ASSERT(!uv_is_writable((uv_stream_t*) &tty_ro));
    uv_close((uv_handle_t*) &tty_ro, NULL);
    ASSERT(!uv_is_readable((uv_stream_t*) &tty_ro));
    ASSERT(!uv_is_writable((uv_stream_t*) &tty_ro));
  }

  fd = open("/dev/tty", O_WRONLY);
  if (fd != -1) {
    ASSERT_OK(uv_tty_init(&loop, &tty_wo, fd, 0));
    ASSERT_OK(close(fd)); /* TODO: it's indeterminate who owns fd now */
    ASSERT(!uv_is_readable((uv_stream_t*) &tty_wo));
    ASSERT(uv_is_writable((uv_stream_t*) &tty_wo));
    uv_close((uv_handle_t*) &tty_wo, NULL);
    ASSERT(!uv_is_readable((uv_stream_t*) &tty_wo));
    ASSERT(!uv_is_writable((uv_stream_t*) &tty_wo));
  }


  ASSERT_OK(uv_run(&loop, UV_RUN_DEFAULT));

  MAKE_VALGRIND_HAPPY(&loop);
#endif
  return 0;
}

TEST_IMPL(tty_pty) {
/* TODO(gengjiawen): Fix test on QEMU. */
#if defined(__QEMU__)
  RETURN_SKIP("Test does not currently work in QEMU");
#endif
#if defined(__ASAN__)
  RETURN_SKIP("Test does not currently work in ASAN");
#endif

#if defined(__APPLE__)                            || \
    defined(__DragonFly__)                        || \
    defined(__FreeBSD__)                          || \
    (defined(__linux__) && !defined(__ANDROID__)) || \
    defined(__NetBSD__)                           || \
    defined(__OpenBSD__)
  int master_fd, slave_fd, r;
  struct winsize w;
  uv_loop_t loop;
  uv_tty_t master_tty, slave_tty;

  ASSERT_OK(uv_loop_init(&loop));

  r = openpty(&master_fd, &slave_fd, NULL, NULL, &w);
  if (r != 0)
    RETURN_SKIP("No pty available, skipping.");

  ASSERT_OK(uv_tty_init(&loop, &slave_tty, slave_fd, 0));
  ASSERT_OK(uv_tty_init(&loop, &master_tty, master_fd, 0));
  ASSERT(uv_is_readable((uv_stream_t*) &slave_tty));
  ASSERT(uv_is_writable((uv_stream_t*) &slave_tty));
  ASSERT(uv_is_readable((uv_stream_t*) &master_tty));
  ASSERT(uv_is_writable((uv_stream_t*) &master_tty));
  /* Check if the file descriptor was reopened. If it is,
   * UV_HANDLE_BLOCKING_WRITES (value 0x100000) isn't set on flags.
   */
  ASSERT_OK((slave_tty.flags & 0x100000));
  /* The master_fd of a pty should never be reopened.
   */
  ASSERT(master_tty.flags & 0x100000);
  ASSERT_OK(close(slave_fd));
  uv_close((uv_handle_t*) &slave_tty, NULL);
  ASSERT_OK(close(master_fd));
  uv_close((uv_handle_t*) &master_tty, NULL);

  ASSERT_OK(uv_run(&loop, UV_RUN_DEFAULT));

  MAKE_VALGRIND_HAPPY(&loop);
#endif
  return 0;
}
