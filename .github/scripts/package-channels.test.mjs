import assert from 'node:assert/strict';
import test from 'node:test';
import { channelManifests } from './package-channels.mjs';
import { archiveName } from './package-release.mjs';
const targets = ['x86_64-apple-darwin', 'aarch64-apple-darwin', 'x86_64-unknown-linux-gnu',
  'aarch64-unknown-linux-gnu', 'x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc'];
const sums = targets.map((target, index) => `${String(index + 1).repeat(64)}  ${archiveName(target, '0.1.1')}`).join('\n');
test('both channels use actual checksums, correct architectures and archive root', () => {
  const { formula, scoop } = channelManifests('0.1.1', sums);
  for (const target of targets.slice(0, 4)) assert.ok(formula.includes(archiveName(target, '0.1.1')));
  assert.equal(scoop.architecture['64bit'].hash, '5'.repeat(64));
  assert.equal(scoop.architecture.arm64.hash, '6'.repeat(64));
  assert.equal(scoop.architecture.arm64.extract_dir, 'cargoc-v0.1.1-aarch64-pc-windows-msvc');
  assert.equal(scoop.bin, 'cargoc.exe');
  assert.ok(scoop.autoupdate.architecture.arm64.extract_dir.includes('$version'));
});
test('missing, unsafe, duplicate or prerelease inputs fail closed', () => {
  for (const input of ['', sums.split('\n').slice(1).join('\n'), `${sums}\n${sums.split('\n')[0]}`, sums.replace('1'.repeat(64), 'bad')]) {
    assert.throws(() => channelManifests('0.1.1', input));
  }
  for (const version of ['../escape', '0.1.1-rc.1', '0.1.1\n']) assert.throws(() => channelManifests(version, sums));
});

test('channel synchronization refuses downgrades', async () => {
  const { mayUpdate, manifestVersion } = await import('./sync-channels.mjs');
  assert.equal(manifestVersion('  version "0.1.1"\n', 'homebrew'), '0.1.1');
  assert.equal(manifestVersion('{"version":"0.1.1"}', 'scoop'), '0.1.1');
  assert.doesNotThrow(() => mayUpdate('0.1.1', '0.1.2'));
  assert.throws(() => mayUpdate('0.2.0', '0.1.2'), /downgrade/);
});
