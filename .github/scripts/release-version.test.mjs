import assert from 'node:assert/strict';
import test from 'node:test';
import { ensureTag, releasePlan } from './release-version.mjs';

const main = { version: '0.1.1', eventName: 'push', refName: 'main', refType: 'branch' };
const commit = 'a'.repeat(40);

test('main publishes the package version, not an invented or automatically incremented version', () => {
  assert.deepEqual(releasePlan(main), { version: '0.1.1', tag: 'v0.1.1', prerelease: false, build: true, publish: true });
  assert.equal(releasePlan({ ...main, version: '0.2.0-rc.1' }).prerelease, true);
});

test('an existing public release is never rebuilt or overwritten; drafts fail closed', () => {
  const plan = releasePlan({ ...main, release: { tag_name: 'v0.1.1', draft: false } });
  assert.equal(plan.build, false);
  assert.equal(plan.publish, false);
  assert.throws(() => releasePlan({ ...main, release: { tag_name: 'v0.1.1', draft: true } }), /draft/);
});

test('manual runs build only; tag pushes must exactly match Cargo.toml', () => {
  assert.equal(releasePlan({ ...main, eventName: 'workflow_dispatch' }).publish, false);
  assert.equal(releasePlan({ ...main, refType: 'tag', refName: 'v0.1.1' }).publish, true);
  for (const input of [
    { ...main, refName: 'other' }, { ...main, refType: 'tag', refName: 'v0.1.2' },
    { ...main, eventName: 'pull_request' }, { ...main, version: '../escape' },
    { ...main, version: '0.1.1\npublish=true' },
  ]) assert.throws(() => releasePlan(input));
});

test('missing tags are created only on the tested source commit', async () => {
  const calls = [];
  await ensureTag(async (...args) => { calls.push(args); return null; }, 'v0.1.1', commit);
  assert.deepEqual(calls, [
    ['git/ref/tags/v0.1.1'],
    ['git/refs', { method: 'POST', body: { ref: 'refs/tags/v0.1.1', sha: commit } }],
  ]);
});

test('matching lightweight and annotated tags are reusable, never moved', async () => {
  await ensureTag(async () => ({ object: { type: 'commit', sha: commit } }), 'v0.1.1', commit);
  const calls = [];
  await ensureTag(async path => {
    calls.push(path);
    return { object: path.startsWith('git/ref/')
      ? { type: 'tag', sha: 'b'.repeat(40) } : { type: 'commit', sha: commit } };
  }, 'v0.1.1', commit);
  assert.equal(calls.length, 2);
  await assert.rejects(ensureTag(async () => ({ object: { type: 'commit', sha: 'c'.repeat(40) } }), 'v0.1.1', commit), /another commit/);
  await assert.rejects(ensureTag(async () => { throw new Error('HTTP 403'); }, 'v0.1.1', commit), /403/);
});
