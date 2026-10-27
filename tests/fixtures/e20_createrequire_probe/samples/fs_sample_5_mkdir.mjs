// E20 fs-domain .mjs sample 5/5 — mkdirSync/existsSync/rmdirSync recursive.
import { require, check, done } from '../common/index.mjs';

const fs = require('node:fs');
const here = import.meta.url.replace(/^file:\/\//, '').replace(/\/[^/]*$/, '');
const dir = here + '/../data/e20_tmp_nested/deeper';

fs.mkdirSync(dir, { recursive: true });
check('mkdir_exists', fs.existsSync(dir) === true);
fs.rmdirSync(here + '/../data/e20_tmp_nested', { recursive: true });
check('rmdir_gone', fs.existsSync(here + '/../data/e20_tmp_nested') === false);

done('fs_sample_5_mkdir');
