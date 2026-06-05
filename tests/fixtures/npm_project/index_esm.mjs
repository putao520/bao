// Test ESM import resolution
import path from 'path';
import assert from 'assert';

console.log('esm_path_sep=' + path.sep);
assert.strictEqual(2 + 2, 4);
console.log('esm_assert_ok=true');
console.log('ESM_PASSED');
