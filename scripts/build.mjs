// Copyright  Alexandre Díaz <dev@redneboa.es>

import {execSync} from 'child_process';

execSync('rollup -c');
//execSync('postcss ./web/scss/components/ --dir static/');