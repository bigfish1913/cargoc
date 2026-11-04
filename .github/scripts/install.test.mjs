import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, existsSync, rmSync, chmodSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, basename } from 'node:path';
import { fileURLToPath } from 'node:url';
import { packageRelease } from './package-release.mjs';
const project = fileURLToPath(new URL('../../', import.meta.url));

test('native installer is checksum-required, extraction-safe and refuses overwrites (offline)', context => {
  const binary = process.env.CARGOC_TEST_BINARY;
  const target = process.env.TARGET;
  if (!binary || !target) { context.skip('Set CARGOC_TEST_BINARY and TARGET'); return; }
  const version = execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim().split(' ')[1];
  const temporary = mkdtempSync(join(tmpdir(), 'cargoc-install-test-'));
  try {
    const archive = packageRelease(binary, target, version, join(temporary, 'dist'));
    const destination = join(temporary, 'install with spaces');
    const env = { ...process.env, CARGOC_FIXTURE_ARCHIVE: archive, CARGOC_FIXTURE_CHECKSUM: `${archive}.sha256`, CARGOC_FIXTURE_VERSION: version, CARGOC_FIXTURE_DESTINATION: destination, CARGOC_DISABLE_UPDATE_CHECK: '1' };
    let run;
    if (process.platform === 'win32') {
      const harness = join(temporary, 'harness.ps1');
      writeFileSync(harness, `$ErrorActionPreference='Stop'\nfunction Invoke-WebRequest { param($Uri,$OutFile,[switch]$UseBasicParsing,$TimeoutSec); if($Uri.EndsWith('.sha256')){Copy-Item -LiteralPath $env:CARGOC_FIXTURE_CHECKSUM -Destination $OutFile}else{Copy-Item -LiteralPath $env:CARGOC_FIXTURE_ARCHIVE -Destination $OutFile} }\n& '${join(project, 'scripts/install.ps1').replaceAll("'", "''")}' -Version $env:CARGOC_FIXTURE_VERSION -Dir $env:CARGOC_FIXTURE_DESTINATION\n`);
      run = extra => spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', harness], { env: { ...env, ...extra }, encoding: 'utf8', timeout: 30_000 });
    } else {
      const mocks = join(temporary, 'mocks'); mkdirSync(mocks);
      const curl = join(mocks, 'curl');
      writeFileSync(curl, '#!/bin/sh\nfor url do :; done\ncase "$url" in *.sha256) cat "$CARGOC_FIXTURE_CHECKSUM" ;; *) cat "$CARGOC_FIXTURE_ARCHIVE" ;; esac\n');
      chmodSync(curl, 0o755);
      run = extra => spawnSync('sh', [join(project, 'scripts/install.sh'), '--version', version, '--dir', extra?.CARGOC_FIXTURE_DESTINATION ?? destination], { env: { ...env, PATH: `${mocks}:${process.env.PATH}`, ...extra }, encoding: 'utf8', timeout: 30_000 });
    }
    const installed = run();
    assert.equal(installed.status, 0, `${installed.stdout}\n${installed.stderr}`);
    const executable = join(destination, process.platform === 'win32' ? 'cargoc.exe' : 'cargoc');
    assert.equal(execFileSync(executable, ['--version'], { encoding: 'utf8' }).trim(), `cargoc ${version}`);
    assert.notEqual(run().status, 0, 'existing files must never be overwritten');
    const bad = join(temporary, 'bad.sha256');
    writeFileSync(bad, `${'0'.repeat(64)}  ${basename(archive)}\n`);
    const rejected = run({ CARGOC_FIXTURE_CHECKSUM: bad, CARGOC_FIXTURE_DESTINATION: join(temporary, 'rejected') });
    assert.notEqual(rejected.status, 0);
    assert.ok(!existsSync(join(temporary, 'rejected', basename(executable))));
  } finally { rmSync(temporary, { recursive: true, force: true }); }
});
