# Installation channels / 安装渠道

The channel generators are the source of truth. Release manifests are generated
from the six **verified** archive checksums, never placeholder hashes. The
archive contract is `cargoc-v{version}-{target}/cargoc[.exe]`, not a flat archive.
Homebrew strips the top directory; Scoop uses an architecture-specific
`extract_dir`; cargo-binstall metadata includes that directory in `bin-dir`.

## Channels

| Channel | Scope | Upgrade |
| --- | --- | --- |
| `scripts/install.sh` | Linux/macOS, x64/ARM64, glibc on Linux | `cargoc self-update` |
| `scripts/install.ps1` | Windows x64/ARM64 | `cargoc self-update` |
| `bigfish1913/homebrew-tap`, `Formula/cargoc.rb` | macOS/Linux, both architectures | `brew update && brew upgrade bigfish1913/tap/cargoc` |
| `bigfish1913/scoop-bucket`, `bucket/cargoc.json` | Windows x64/ARM64 | `scoop update && scoop update cargoc` |
| crates.io | Separately published crate; currently 0.1.0 | `cargo install cargoc --locked` |
| cargo-binstall | Metadata configured for the next crates.io publication | Not independently install-tested; not advertised as a live new-version channel yet |

These are owner-maintained package-manager sources, not Homebrew core or winget.
No Microsoft package submission is performed by this repository.

## Release automation

After all six targets pass tests, native installer tests, packaging and checksum
verification, stable GitHub Releases attach `cargoc.rb` and `cargoc.json` alongside
the archives. Prereleases do **not** update stable package-manager channels.

To automatically sync the two external repositories, configure `CHANNELS_TOKEN`
as an Actions secret in **cargoc**, using a fine-grained token scoped only to
`bigfish1913/homebrew-tap` and `bigfish1913/scoop-bucket`, with Contents write
permission. The normal `GITHUB_TOKEN` cannot write another repository. Never
commit credentials, paste them into a script, or broaden the token to unrelated
repositories. Without this secret the release still succeeds, emits a notice,
and leaves the manifests available for local sync.

本仓库默认令牌不能写入另外两个安装源仓库。自动同步需要上述最小权限 Secret；
没有配置时不会假装完成同步，可以用下面的本地命令。发布完成与安装源同步是两个
可区分的状态。

## Manual sync / recovery

Use an authenticated `gh` session with permission for the two owner-controlled
repositories. Download only the manifests from the already-published release:

```sh
VERSION=0.1.3
mkdir -p channels/homebrew channels/scoop
gh release download "v$VERSION" --repo bigfish1913/cargoc --pattern cargoc.rb --dir channels/homebrew
gh release download "v$VERSION" --repo bigfish1913/cargoc --pattern cargoc.json --dir channels/scoop
node .github/scripts/sync-channels.mjs channels "$VERSION" --dry-run
node .github/scripts/sync-channels.mjs channels "$VERSION"
```

The script updates **only** `Formula/cargoc.rb` and `bucket/cargoc.json`, checks
local/remote versions, refuses downgrades, and uses each existing file's GitHub
SHA for optimistic concurrency. Identical content is a no-op. It does not edit
rpi's manifests, create repositories, move release tags or replace assets. A
partial/uncertain network failure should be inspected before retrying; an
identical already-applied update is safe to observe and skip on the next run.

To regenerate for inspection from a verified combined checksum file:

```sh
node .github/scripts/package-channels.mjs 0.1.3 /path/to/SHA256SUMS /tmp/cargoc-channels
```

Output directories must not already contain the output manifest files.

## Checks

- Native release jobs test bootstrap installation with fixture downloads: correct
  version, checksum required, corrupt download rejection, and no overwrites.
- Rust tests cover source detection, stable-version validation, cache freshness,
  failure backoff, offline/CI suppression and refusal to modify package-manager
  installations.
- The manifests' URLs and hashes must be read back after publication. Generating
  a syntactically valid manifest does not prove that Homebrew/Scoop can install
  it; report package-manager install tests separately.
