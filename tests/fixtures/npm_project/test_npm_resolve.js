// Test that local module resolution works (node_modules or relative)
const path = require('path');
const fs = require('fs');

// Test require.resolve exists
if (typeof require.resolve === 'function') {
  console.log('require_resolve_ok=true');
}

// Test relative import
const { add } = require('./utils.js');
console.log('add_result=' + add(2, 3));
console.log('NPM_RESOLVE_PASSED');
