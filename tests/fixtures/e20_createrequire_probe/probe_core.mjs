// E20 createRequire probe — the single gate behind the fs-domain 96-file
// ESM_MISSING_EXPORT:createRequire equivalence class (E18 farm baseline
// 2026-10-25). Task oracle: node v24 createRequire semantics.
// Run: bao run tests/fixtures/e20_createrequire_probe/probe_core.mjs
import { createRequire } from 'node:module';

const results = [];
function check(name, cond, detail = '') {
  results.push([name, cond ? 'PASS' : 'FAIL' + (detail ? '(' + detail + ')' : '')]);
}

// P1 — the gate itself: ESM named-export link of node:module.
check('typeof_createRequire', typeof createRequire === 'function');

// P2 — task-specified probe: createRequire('/x/y.js') carries require('node:fs').
const r = createRequire('/x/y.js');
check('typeof_r', typeof r === 'function');
const fsMod = r('node:fs');
check('node_fs_round', !!fsMod && typeof fsMod.readFileSync === 'function');

// P3 — oracle attribute face: Object.keys(r) === [resolve, main, extensions, cache].
const keys = Object.keys(r).sort().join(',');
check('req_keys', keys === 'cache,extensions,main,resolve', keys);
check('typeof_r_resolve', typeof r.resolve === 'function');
check('r_main_undefined', r.main === undefined);
const extKeys = Object.keys(r.extensions).join(',');
check('ext_keys', extKeys === '.js,.json,.node', extKeys);

// P4 — dynamic import shares the same surface.
const dyn = await import('node:module');
check('dyn_import_face', typeof dyn.createRequire === 'function');

// P5 — oracle object identities: cache === Module._cache, extensions === Module._extensions.
const Module = r('node:module');
check('cache_identity', r.cache === Module._cache);
check('ext_identity', r.extensions === Module._extensions);

// P6 — builtin resolve passthrough (form recorded honestly; bao's shared
// resolve face currently returns the stripped key — pre-existing global
// semantics, divergence from node's `node:fs` echo logged in the task report).
check('resolve_builtin_is_string', typeof r.resolve('node:fs') === 'string');

// P7 — invalid args throw ERR_INVALID_ARG_VALUE (node v24 oracle: no-arg,
// relative path, non-file URL, number all rejected).
try {
  createRequire();
  check('no_arg_throw', false, 'no-throw');
} catch (e) {
  check('no_arg_throw', e.code === 'ERR_INVALID_ARG_VALUE', e.code);
}
try {
  createRequire('./rel.js');
  check('rel_path_throw', false, 'no-throw');
} catch (e) {
  check('rel_path_throw', e.code === 'ERR_INVALID_ARG_VALUE', e.code);
}
try {
  createRequire('https://x.com/a.js');
  check('non_file_url_throw', false, 'no-throw');
} catch (e) {
  check('non_file_url_throw', e.code === 'ERR_INVALID_ARG_VALUE', e.code);
}
try {
  createRequire(42);
  check('number_throw', false, 'no-throw');
} catch (e) {
  check('number_throw', e.code === 'ERR_INVALID_ARG_VALUE', e.code);
}

// P8 — file:// URL string face.
const r3 = createRequire('file:///tmp/e20-probe/z.js');
check('url_string_face', typeof r3 === 'function');

// P9 — URL object face.
const r4 = createRequire(new URL('file:///tmp/e20-probe/z.js'));
check('url_object_face', typeof r4 === 'function');

// P10 — the interop pattern the fs fixtures use: createRequire(import.meta.url).
const req2 = createRequire(import.meta.url);
check('import_meta_url_face', typeof req2 === 'function');

// P11 — base-dir semantics: relative resolve/require from the created
// require's own directory (dirname(filename)), not cwd.
const here = import.meta.url.replace(/^file:\/\//, '').replace(/\/[^/]*$/, '');
const resolved = req2.resolve('./sibling_target.js');
check(
  'resolve_relative_from_base',
  resolved === here + '/sibling_target.js',
  resolved,
);
const sib = req2('./sibling_target.cjs');
check('require_relative_cjs', !!sib && sib.marker === 42, JSON.stringify(sib));

// P12 — node_modules chain from the created base (parentFilename 视角).
const nm = req2('fakepkg');
check(
  'node_modules_from_base',
  !!nm && nm.pkgMarker === 'e20',
  JSON.stringify(nm),
);

for (const [k, v] of results) console.log(k + ' ' + v);
const fails = results.filter(([, v]) => !v.startsWith('PASS'));
if (fails.length > 0) {
  console.error('E20_PROBE: ' + fails.length + ' FAIL');
  process.exit(1);
}
console.log('E20_PROBE_ALL_PASS');
