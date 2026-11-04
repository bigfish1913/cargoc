// Dependency-free packaging helper, shared by all native GitHub release jobs.
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

const platforms = {
  'x86_64-pc-windows-msvc': 'win32',
  'aarch64-pc-windows-msvc': 'win32',
  'x86_64-unknown-linux-gnu': 'linux',
  'aarch64-unknown-linux-gnu': 'linux',
  'x86_64-apple-darwin': 'darwin',
  'aarch64-apple-darwin': 'darwin',
};

export function archiveName(target, version) {
  if (!Object.hasOwn(platforms, target)) throw new Error(`Unsupported target: ${target}`);
  if (typeof version !== 'string' || version.trim() !== version
      || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error(`Unsafe or invalid version: ${version}`);
  }
  return `cargoc-v${version}-${target}.${platforms[target] === 'win32' ? 'zip' : 'tar.gz'}`;
}

export function packageRelease(binary, target, version, output) {
  const name = archiveName(target, version);
  if (platforms[target] !== process.platform) throw new Error('Packaging requires a native runner');
  const source = resolve(binary);
  const executable = process.platform === 'win32' ? 'cargoc.exe' : 'cargoc';
  if (basename(source) !== executable) throw new Error(`Expected ${executable}`);
  const actual = execFileSync(source, ['--version'], { encoding: 'utf8', timeout: 30_000 }).trim();
  if (actual !== `cargoc ${version}`) throw new Error(`Binary version mismatch: ${actual}`);
  mkdirSync(output, { recursive: true });
  const archive = resolve(output, name);
  if (existsSync(archive) || existsSync(`${archive}.sha256`)) throw new Error('Refusing to replace existing release files');
  const temporary = mkdtempSync(join(tmpdir(), 'cargoc-package-'));
  try {
    const directory = `cargoc-v${version}-${target}`;
    const stage = join(temporary, directory);
    mkdirSync(stage);
    copyFileSync(source, join(stage, executable));
    if (process.platform !== 'win32') chmodSync(join(stage, executable), 0o755);
    const project = fileURLToPath(new URL('../../', import.meta.url));
    for (const file of ['README.md', 'README.zh-CN.md', 'LICENSE']) {
      copyFileSync(join(project, file), join(stage, file));
    }
    if (process.platform === 'win32') {
      execFileSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command',
        '$ErrorActionPreference="Stop"; Compress-Archive -LiteralPath $env:CARGOC_PACKAGE_DIR -DestinationPath $env:CARGOC_ARCHIVE_PATH -CompressionLevel Optimal'], {
        stdio: 'inherit',
        env: { ...process.env, CARGOC_PACKAGE_DIR: stage, CARGOC_ARCHIVE_PATH: archive },
      });
    } else {
      execFileSync('tar', ['-czf', archive, '-C', temporary, directory], { stdio: 'inherit' });
    }
    const digest = createHash('sha256').update(readFileSync(archive)).digest('hex');
    writeFileSync(`${archive}.sha256`, `${digest}  ${name}\n`, { flag: 'wx' });
    return archive;
  } catch (error) {
    rmSync(archive, { force: true });
    rmSync(`${archive}.sha256`, { force: true });
    throw error;
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  const args = process.argv.slice(2);
  if (args.length !== 4) throw new Error('Usage: package-release.mjs BINARY TARGET VERSION OUTPUT');
  console.log(`Created ${packageRelease(...args)}`);
}
