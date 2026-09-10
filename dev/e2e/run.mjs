// One command for the whole E2E smoke suite: `node dev/e2e/run.mjs`.
//
// Does setup (pinned jellyfin-web + the generated clip) and then runs every
// scenario in dev/e2e/scenarios serially — the app is single-instance per
// profile and each run owns an mpv window, so nothing here may overlap.
//
// Flags:
//   --only <substr>   run only scenario files whose name contains <substr>
//   --setup-only      prepare the cache and exit
//   --list            print the scenario files and exit
// Environment:
//   E2E_AUDIO=1       let mpv open a real audio device (silent otherwise)

import fs from 'node:fs';
import path from 'node:path';
import { spawn } from 'node:child_process';

import { E2E_DIR, requireBuild } from './lib/paths.mjs';
import { prepare } from './setup.mjs';

const SCENARIOS = path.join(E2E_DIR, 'scenarios');

function scenarioFiles(filter) {
    return fs
        .readdirSync(SCENARIOS)
        .filter((f) => f.endsWith('.test.mjs'))
        .filter((f) => !filter || f.includes(filter))
        .sort()
        .map((f) => path.join(SCENARIOS, f));
}

function arg(name) {
    const i = process.argv.indexOf(name);
    return i === -1 ? undefined : process.argv[i + 1];
}

async function main() {
    const only = arg('--only');
    const files = scenarioFiles(only);

    if (process.argv.includes('--list')) {
        for (const f of files) process.stdout.write(`${path.basename(f)}\n`);
        return 0;
    }
    if (!files.length) {
        process.stderr.write(`no scenarios matched ${only ?? '(all)'}\n`);
        return 1;
    }

    requireBuild();
    const ready = await prepare();
    process.stdout.write(
        `[e2e] jellyfin-web ${ready.jellyfinWebVersion}, clip ${path.basename(ready.clip)}, ` +
            `${files.length} scenario file(s), audio ${process.env.E2E_AUDIO === '1' ? 'ON' : 'muted'}\n`,
    );
    if (process.argv.includes('--setup-only')) return 0;

    const started = Date.now();
    // --test-concurrency=1: only one app instance may exist at a time.
    const child = spawn(process.execPath, ['--test', '--test-concurrency=1', ...files], {
        stdio: 'inherit',
        cwd: path.dirname(path.dirname(E2E_DIR)),
    });
    const code = await new Promise((resolve) => child.on('exit', resolve));
    process.stdout.write(`[e2e] finished in ${((Date.now() - started) / 1000).toFixed(1)} s\n`);
    return code ?? 1;
}

main().then(
    (code) => process.exit(code),
    (e) => {
        process.stderr.write(`[e2e] ${e.stack ?? e.message}\n`);
        process.exit(1);
    },
);
