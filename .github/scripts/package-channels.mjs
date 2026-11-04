// Generate real Homebrew/Scoop manifests from verified release checksums.
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { archiveName } from './package-release.mjs';

const targets = ['x86_64-apple-darwin', 'aarch64-apple-darwin',
  'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu',
  'x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc'];

export function channelManifests(version, sums) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error('Only stable versions enter package-manager channels');
  const checksums = new Map();
  for (const line of sums.trim().split(/\r?\n/)) {
    const match = /^([a-fA-F0-9]{64})\s+\*?([^\s/\\]+)$/.exec(line);
    if (!match || checksums.has(match[2])) throw new Error('Invalid/duplicate SHA256SUMS entry');
    checksums.set(match[2], match[1].toLowerCase());
  }
  const asset = target => {
    const file = archiveName(target, version);
    const hash = checksums.get(file);
    if (!hash) throw new Error(`Missing checksum for ${file}`);
    return { url: `https://github.com/bigfish1913/cargoc/releases/download/v${version}/${file}`, hash };
  };
  for (const target of targets) asset(target);
  const stanza = (target, indent) => {
    const { url, hash } = asset(target);
    return `${indent}url "${url}"\n${indent}sha256 "${hash}"`;
  };
  const formula = `# Generated from the published release's SHA256SUMS; do not substitute placeholder hashes.
class Cargoc < Formula
  desc "Cargo wrapper with managed intermediate caches and safe disk-pressure cleanup"
  homepage "https://github.com/bigfish1913/cargoc"
  version "${version}"
  license "MIT"

  on_macos do
    on_arm do
${stanza('aarch64-apple-darwin', '      ')}
    end
    on_intel do
${stanza('x86_64-apple-darwin', '      ')}
    end
  end
  on_linux do
    on_arm do
${stanza('aarch64-unknown-linux-gnu', '      ')}
    end
    on_intel do
${stanza('x86_64-unknown-linux-gnu', '      ')}
    end
  end

  def install
    bin.install "cargoc"
  end

  test do
    assert_match "cargoc ${version}", shell_output("#{bin}/cargoc --version")
  end
end
`;
  const architecture = {};
  const autoupdateArchitecture = {};
  for (const [key, target] of [['64bit', 'x86_64-pc-windows-msvc'], ['arm64', 'aarch64-pc-windows-msvc']]) {
    architecture[key] = { ...asset(target), extract_dir: `cargoc-v${version}-${target}` };
    autoupdateArchitecture[key] = {
      url: `https://github.com/bigfish1913/cargoc/releases/download/v$version/cargoc-v$version-${target}.zip`,
      extract_dir: `cargoc-v$version-${target}`,
    };
  }
  const scoop = { version, description: 'Cargo wrapper with managed intermediate caches and safe disk-pressure cleanup',
    homepage: 'https://github.com/bigfish1913/cargoc', license: 'MIT', architecture, bin: 'cargoc.exe',
    checkver: { github: 'https://github.com/bigfish1913/cargoc' },
    autoupdate: { architecture: autoupdateArchitecture, hash: { url: '$url.sha256', regex: '^([a-fA-F0-9]{64})' } } };
  return { formula, scoop };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [version, checksumFile, output] = process.argv.slice(2);
  const { formula, scoop } = channelManifests(version, readFileSync(checksumFile, 'utf8'));
  mkdirSync(join(output, 'homebrew'), { recursive: true });
  mkdirSync(join(output, 'scoop'), { recursive: true });
  writeFileSync(join(output, 'homebrew', 'cargoc.rb'), formula, { flag: 'wx' });
  writeFileSync(join(output, 'scoop', 'cargoc.json'), `${JSON.stringify(scoop, null, 2)}\n`, { flag: 'wx' });
}
