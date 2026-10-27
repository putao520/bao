// E20 fs-domain .mjs sample 1/5 — readFileSync through the created require
// (E18 equivalence-class shape: createRequire gate + fs API).
import { require, check, done } from '../common/index.mjs';

const fs = require('node:fs');
const here = import.meta.url.replace(/^file:\/\//, '').replace(/\/[^/]*$/, '');

const content = fs.readFileSync(here + '/../data/hello.txt', 'utf8');
check('fs_read_content', content.trim() === 'e20-hello', JSON.stringify(content));

done('fs_sample_1_read');
