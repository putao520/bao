// E20 fs-domain .mjs sample 4/5 — readdirSync listing.
import { require, check, done } from '../common/index.mjs';

const fs = require('node:fs');
const here = import.meta.url.replace(/^file:\/\//, '').replace(/\/[^/]*$/, '');

const entries = fs.readdirSync(here + '/../data');
check('readdir_has_hello', entries.includes('hello.txt'), entries.join(','));

done('fs_sample_4_dir');
