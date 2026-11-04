// Release policy and tag creation shared by main/tag-triggered workflows.
import { appendFileSync, readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export function releasePlan({ version, eventName, refName, refType, release = null }) {
  if (typeof version !== 'string' || version.length > 128
      || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error('Invalid package version');
  }
  const tag = `v${version}`;
  const prerelease = version.split('+')[0].includes('-');
  const result = { version, tag, prerelease, build: true, publish: false };
  if (eventName === 'workflow_dispatch') return result;
  if (eventName !== 'push' || !((refType === 'branch' && refName === 'main')
      || (refType === 'tag' && refName === tag))) {
    throw new Error(`Release requires main or the exact package tag ${tag}`);
  }
  if (release !== null) {
    if (release.tag_name !== tag || release.draft !== false) {
      throw new Error(`Inspect the existing release/draft for ${tag} before retrying`);
    }
    return { ...result, build: false };
  }
  return { ...result, publish: true };
}

export async function ensureTag(api, tag, commit) {
  if (!/^v[0-9A-Za-z.+-]+$/.test(tag) || !/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error('Invalid tag or source commit');
  }
  const reference = await api(`git/ref/tags/${encodeURIComponent(tag)}`);
  if (reference === null) {
    await api('git/refs', { method: 'POST', body: { ref: `refs/tags/${tag}`, sha: commit } });
    return;
  }
  let object = reference.object;
  for (let depth = 0; object?.type === 'tag' && depth < 8; depth++) {
    if (!/^[0-9a-f]{40}$/.test(object.sha)) throw new Error('Invalid annotated tag object');
    object = (await api(`git/tags/${object.sha}`))?.object;
  }
  if (object?.type !== 'commit' || object.sha !== commit) {
    throw new Error(`${tag} already points to another commit; bump Cargo.toml instead of moving tags`);
  }
}

function githubApi() {
  const { GH_REPO: repo, GH_TOKEN: token } = process.env;
  if (!/^[\w.-]+\/[\w.-]+$/.test(repo ?? '') || !token) throw new Error('GitHub repository/token missing');
  return async (path, options = {}) => {
    const response = await fetch(`https://api.github.com/repos/${repo}/${path}`, {
      method: options.method ?? 'GET',
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: 'application/vnd.github+json',
        'X-GitHub-Api-Version': '2022-11-28',
        'Content-Type': 'application/json',
      },
      body: options.body ? JSON.stringify(options.body) : undefined,
      signal: AbortSignal.timeout(20_000),
      redirect: 'error',
    });
    if (response.status === 404 && !options.method) return null;
    if (!response.ok) throw new Error(`GitHub API ${options.method ?? 'GET'} ${path}: HTTP ${response.status}`);
    return response.json();
  };
}

async function main() {
  if (process.argv[2] === '--ensure-tag') {
    await ensureTag(githubApi(), process.env.TAG, process.env.SOURCE_SHA);
    return;
  }
  const metadata = JSON.parse(readFileSync(process.argv[2], 'utf8'));
  const version = metadata.packages.find(pkg => pkg.name === 'cargoc')?.version;
  const input = {
    version, eventName: process.env.EVENT_NAME,
    refName: process.env.REF_NAME, refType: process.env.REF_TYPE,
  };
  let plan = releasePlan(input);
  if (plan.publish) {
    const release = await githubApi()(`releases/tags/${encodeURIComponent(plan.tag)}`);
    plan = releasePlan({ ...input, release });
    if (!plan.publish) console.log(`${plan.tag} is already published; bump Cargo.toml for the next release.`);
  }
  for (const [name, value] of Object.entries(plan)) {
    appendFileSync(process.env.GITHUB_OUTPUT, `${name}=${value}\n`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
