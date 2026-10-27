// E20 fs-domain .mjs sample 2/5 — writeFileSync/readFileSync roundtrip.
import { require, check, done } from '../common/index.mjs';

const fs = require('node:fs');
const here = import.meta.url.replace(/^file:\/\//, '').replace(/\/[^/]*$/, '');
const target = here + '/../data/e20_roundtrip.txt';

fs.writeFileSync(target, 'e20-roundtrip');
const back = fs.readFileSync(target, 'utf8');
check('fs_write_roundtrip', back === 'e20-roundtrip', JSON.stringify(back));

done('fs_sample_2_write');
