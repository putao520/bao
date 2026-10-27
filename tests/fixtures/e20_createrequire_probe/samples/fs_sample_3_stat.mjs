// E20 fs-domain .mjs sample 3/5 — statSync file/dir discrimination.
import { require, check, done } from '../common/index.mjs';

const fs = require('node:fs');
const here = import.meta.url.replace(/^file:\/\//, '').replace(/\/[^/]*$/, '');

const st = fs.statSync(here + '/../data/hello.txt');
check('stat_is_file', st.isFile() === true);
const sd = fs.statSync(here + '/../data');
check('stat_is_dir', sd.isDirectory() === true);

done('fs_sample_3_stat');
