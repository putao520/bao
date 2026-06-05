// Test CJS require resolution
const path = require('path');
const assert = require('assert');

// Test built-in module
console.log('path_sep=' + path.sep);

// Test assert works
assert.strictEqual(1 + 1, 2);
console.log('assert_ok=true');

// Test process
console.log('process_arch=' + process.arch);
console.log('process_platform=' + process.platform);

console.log('CJS_PASSED');
