// Sync only cargoc's files in the two existing owner-controlled channels.
// Uses gh's authenticated session locally, or GH_TOKEN in GitHub Actions.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

export function manifestVersion(text, kind) {
  const version = kind === 'scoop' ? JSON.parse(text).version : /^\s*version "([^"]+)"$/m.exec(text)?.[1];
  if (!/^\d+\.\d+\.\d+$/.test(version ?? '')) throw new Error('Invalid stable channel version');
  return version;
}
export function mayUpdate(previous, next) {
  const a = previous.split('.').map(BigInt), b = next.split('.').map(BigInt);
  for (let i = 0; i < 3; i++) {
    if (a[i] > b[i]) throw new Error(`Refusing to downgrade ${previous} to ${next}`);
    if (a[i] < b[i]) return;
  }
}
function api(endpoint, body) {
  const args = ['api', endpoint];
  if (body) args.push('--method', 'PUT', '--input', '-');
  const output = spawnSync('gh', args, { encoding: 'utf8', input: body ? JSON.stringify(body) : undefined, timeout: 30_000 });
  if (!body && output.status !== 0 && /HTTP 404\b/.test(output.stderr ?? '')) return null;
  if (output.status !== 0) throw new Error(`Channel API failed: ${endpoint}: ${output.stderr ?? output.error}`);
  return JSON.parse(output.stdout);
}
export async function syncChannels(directory, version, request = api, dryRun = false) {
  const files = [
    { repo: 'bigfish1913/homebrew-tap', path: 'Formula/cargoc.rb', local: 'homebrew/cargoc.rb', kind: 'homebrew' },
    { repo: 'bigfish1913/scoop-bucket', path: 'bucket/cargoc.json', local: 'scoop/cargoc.json', kind: 'scoop' },
  ];
  for (const file of files) {
    const text = readFileSync(join(directory, file.local), 'utf8');
    if (manifestVersion(text, file.kind) !== version) throw new Error(`Local manifest does not match ${version}`);
    if (dryRun) { console.log(`${file.repo}/${file.path}: ${version}`); continue; }
    const endpoint = `repos/${file.repo}/contents/${file.path}`;
    const current = await request(endpoint);
    const body = { message: `cargoc ${version}`, content: Buffer.from(text).toString('base64') };
    if (current) {
      const previous = Buffer.from(current.content, 'base64').toString('utf8');
      if (previous === text) continue;
      mayUpdate(manifestVersion(previous, file.kind), version);
      body.sha = current.sha;
    }
    await request(endpoint, body);
    console.log(`Updated ${file.repo}/${file.path} to ${version}`);
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [directory, version, flag] = process.argv.slice(2);
  syncChannels(directory, version, api, flag === '--dry-run').catch(error => { console.error(error.message); process.exitCode = 1; });
}
