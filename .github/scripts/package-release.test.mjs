import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { basename, join } from 'node:path';
import { tmpdir } from 'node:os';
import { archiveName, packageRelease } from './package-release.mjs';

test('all six targets have unique, versioned archive names', () => {
  const targets = ['x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc',
    'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu',
    'x86_64-apple-darwin', 'aarch64-apple-darwin'];
  const names = targets.map(target => archiveName(target, '1.2.3'));
  assert.equal(new Set(names).size, 6);
  for (let index = 0; index < targets.length; index++) {
    assert.equal(names[index], `cargoc-v1.2.3-${targets[index]}.${index < 2 ? 'zip' : 'tar.gz'}`);
  }
  assert.match(archiveName(targets[0], '1.2.3-beta.1+build'), /v1\.2\.3-beta\.1\+build-/);
});

test('unsafe versions and unsupported targets are rejected', () => {
  for (const version of ['../escape', '1.2.3\n', 'v1.2.3', '', '1.2.3/path']) {
    assert.throws(() => archiveName('x86_64-pc-windows-msvc', version));
  }
  for (const target of ['../escape', 'toString', 'x86_64-unknown-linux-musl']) {
    assert.throws(() => archiveName(target, '1.2.3'));
  }
});

test('native archive extracts, runs and matches its checksum', context => {
  const binary = process.env.CARGOC_TEST_BINARY;
  const target = process.env.TARGET;
  if (!binary || !target) { context.skip('Set CARGOC_TEST_BINARY and TARGET for the native packaging test'); return; }
  const version = execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim().split(' ')[1];
  const temporary = mkdtempSync(join(tmpdir(), 'cargoc-release-test-'));
  try {
    const output = join(temporary, 'output');
    const archive = packageRelease(binary, target, version, output);
    const digest = createHash('sha256').update(readFileSync(archive)).digest('hex');
    assert.equal(readFileSync(`${archive}.sha256`, 'utf8'), `${digest}  ${basename(archive)}\n`);
    assert.equal(readdirSync(output).length, 2, 'Only the archive and checksum should be uploaded');
    assert.throws(() => packageRelease(binary, target, version, output), /replace existing/);
    assert.throws(() => packageRelease(binary, target, '999.0.0', output), /version mismatch/);
    const expanded = join(temporary, 'expanded');
    mkdirSync(expanded);
    if (process.platform === 'win32') {
      execFileSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command',
        '$ErrorActionPreference="Stop"; Expand-Archive -LiteralPath $env:CARGOC_ARCHIVE_PATH -DestinationPath $env:CARGOC_EXPAND_DIR'], {
        env: { ...process.env, CARGOC_ARCHIVE_PATH: archive, CARGOC_EXPAND_DIR: expanded },
      });
    } else {
      execFileSync('tar', ['-xzf', archive, '-C', expanded]);
    }
    const root = join(expanded, `cargoc-v${version}-${target}`);
    for (const file of ['README.md', 'README.zh-CN.md', 'LICENSE']) {
      assert.ok(readFileSync(join(root, file)).length > 0);
    }
    const unpacked = join(root, process.platform === 'win32' ? 'cargoc.exe' : 'cargoc');
    assert.equal(execFileSync(unpacked, ['--version'], { encoding: 'utf8' }).trim(), `cargoc ${version}`);
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
});
