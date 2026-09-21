/* Copyright libuv project contributors. All rights reserved.
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
#include "internal.h"
#include "winapi.h"

static void uv__register_system_resume_callback(void);

static uv_once_t uv__detect_system_wakeup_guard = UV_ONCE_INIT;

/* Registering the power callback loads powrprof/umpdc and connects an ALPC
 * port to the power service. It only matters once a loop actually blocks, so
 * uv__poll arms it before its first wait instead of at process startup. */
void uv__detect_system_wakeup_ensure(void) {
  /* Try registering system power event callback. This is the cleanest
   * method, but it will only work on Win8 and above.
   */
  uv_once(&uv__detect_system_wakeup_guard, uv__register_system_resume_callback);
}

static ULONG CALLBACK uv__system_resume_callback(PVOID Context,
                                                 ULONG Type,
                                                 PVOID Setting) {
  if (Type == PBT_APMRESUMESUSPEND || Type == PBT_APMRESUMEAUTOMATIC)
    uv__wake_all_loops();

  return 0;
}

static void uv__register_system_resume_callback(void) {
  _DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS recipient;
  _HPOWERNOTIFY registration_handle;

  if (pPowerRegisterSuspendResumeNotification == NULL) {
    HMODULE powrprof_module = LoadLibraryExW(L"powrprof.dll", NULL, LOAD_LIBRARY_SEARCH_SYSTEM32);
    if (powrprof_module != NULL)
      pPowerRegisterSuspendResumeNotification =
          (sPowerRegisterSuspendResumeNotification) (void (*)(void))
              GetProcAddress(powrprof_module,
                             "PowerRegisterSuspendResumeNotification");
    if (pPowerRegisterSuspendResumeNotification == NULL)
      return;
  }

  recipient.Callback = uv__system_resume_callback;
  recipient.Context = NULL;
  (*pPowerRegisterSuspendResumeNotification)(DEVICE_NOTIFY_CALLBACK,
                                             &recipient,
                                             &registration_handle);
}
