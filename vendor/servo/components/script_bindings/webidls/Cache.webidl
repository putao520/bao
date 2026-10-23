/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://w3c.github.io/ServiceWorker/#cache
[Pref="dom_serviceworker_enabled", SecureContext, Exposed=(Window,Worker)]
interface Cache {
  [NewObject] Promise<Response?> match(RequestInfo request, optional CacheQueryOptions options = {});
  [NewObject] Promise<sequence<Response>> matchAll(optional RequestInfo request, optional CacheQueryOptions options = {});
  [NewObject] Promise<undefined> add(RequestInfo request);
  [NewObject] Promise<undefined> addAll(sequence<RequestInfo> requests);
  [NewObject] Promise<undefined> put(RequestInfo request, Response response);
  [NewObject] Promise<boolean> delete(RequestInfo request, optional CacheQueryOptions options = {});
  // Workaround until FrozenArray get implemented.
  [NewObject] Promise<sequence<Request>> keys(optional RequestInfo request, optional CacheQueryOptions options = {});
};

dictionary CacheQueryOptions {
  boolean ignoreSearch = false;
  boolean ignoreMethod = false;
  boolean ignoreVary = false;
};
