// E20 reconstruction of the E18 fs-corpus common harness. E18 farm baseline
// (2026-10-25): `common/index.mjs:63 import { createRequire } from
// 'node:module'` was the single equivalence-class gate — 96/96 fs-domain
// .mjs fixtures died at ESM link time on this one missing named export.
// This harness reproduces the shape: ESM harness importing createRequire,
// building a CJS require from import.meta.url, handing it to the samples.
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);

const results = [];

export function check(name, cond, detail = '') {
  const verdict = cond ? 'PASS' : 'FAIL' + (detail ? '(' + detail + ')' : '');
  results.push([name, verdict]);
  return cond;
}

export function done(label) {
  for (const [k, v] of results) console.log(label + '.' + k + ' ' + v);
  const fails = results.filter(([, v]) => !v.startsWith('PASS'));
  if (fails.length > 0) {
    console.error(label + ': ' + fails.length + ' FAIL');
    process.exit(1);
  }
  console.log(label + ' ALL_PASS');
}

export { require };
