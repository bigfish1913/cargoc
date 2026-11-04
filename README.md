# cargoc

A Rust CLI that wraps Cargo with a centralized, owned intermediate-build cache and disk-pressure cleanup. Final build artifacts stay at Cargo's original target directory: the workspace's `target/` by default, or the location selected by your existing Cargo configuration, environment, or `--target-dir`.

[简体中文](README.zh-CN.md) · [GitHub](https://github.com/bigfish1913/cargoc) · [crates.io](https://crates.io/crates/cargoc)

## Requirements and installation

- Rust **1.88+** to compile cargoc (MSRV).
- The Cargo executable wrapped for managed builds must be **1.91+**, with stable `build.build-dir` support. An older Cargo may compile cargoc but cannot run its managed builds.
- Windows, Linux, or macOS. This is a native Rust application, not a Python script.

### Prebuilt installer (no Rust compiler needed)

Linux/macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/bigfish1913/cargoc/main/scripts/install.sh | sh
# Preview a pinned install without downloading it:
# sh scripts/install.sh --version 0.1.3 --dry-run
```

Windows PowerShell 5.1+ (x64 and ARM64):

```powershell
& ([scriptblock]::Create((Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/bigfish1913/cargoc/main/scripts/install.ps1).Content))
# With a downloaded script: .\scripts\install.ps1 -Version 0.1.3 -DryRun
```

Review the downloaded script before running it if you do not want to execute remote code directly. Both installers require the release's SHA-256 checksum, verify the binary version, and refuse to overwrite an existing installation. They install only `cargoc`, never Rustup's `cargo`, and do not edit PATH/configuration or enable the shim. Defaults: `~/.local/bin` on Unix, `%LOCALAPPDATA%/Programs/cargoc` on Windows. Use `--dir` / `-Dir` to choose another directory; follow the printed PATH instructions.

### Package managers

```sh
# macOS Intel/Apple Silicon and Linux x64/ARM64 (glibc 2.35+)
brew tap bigfish1913/tap
brew install cargoc

# Windows x64/ARM64
scoop bucket add bigfish1913 https://github.com/bigfish1913/scoop-bucket
scoop install cargoc
```

The [Homebrew tap](https://github.com/bigfish1913/homebrew-tap) and [Scoop bucket](https://github.com/bigfish1913/scoop-bucket) use hashes from published release assets. These are owner-maintained channels, not Homebrew core or Microsoft's winget. See [channel maintenance](packaging/README.md).

#### Use cargoc as the default Cargo after installation

Installing with Homebrew or Scoop installs `cargoc` only; it does **not** automatically enable Cargo integration or replace the real Cargo executable. Cargo must already be installed. To make bare `cargo` commands use cargoc:

**macOS / Homebrew** (also applies to Homebrew on Linux):

```sh
cargoc setup --enable
export PATH="$HOME/.cargoc/bin:$PATH"
command -v cargo
cargo --version
cargoc setup --status
```

The `export` affects only the current shell. For persistence, add it to your shell startup file (for example, `~/.zshrc` for macOS's default interactive zsh). Setup does not edit that file. `command -v cargo` should select `~/.cargoc/bin/cargo`; that directory must precede the real Cargo directory on PATH.

**Windows / Scoop**, in PowerShell:

```powershell
cargoc setup --enable
# Reopen the terminal before running the following checks:
Get-Command cargo
cargo --version
cargoc setup --status
```

Setup prepends `%USERPROFILE%\.cargoc\bin` to the **user-level** PATH and creates a native `cargo.exe` proxy alongside compatible `cargo.cmd` and `cargo` shell scripts. With the default PATHEXT, `Get-Command cargo` should select that directory's `cargo.exe`; tools launching `cargo.exe` through PATH also use the proxy. It forwards OS arguments individually, preserves exit codes, and uses the cargoc configuration selected during setup. Restart existing terminals and IDEs to inherit PATH changes. A machine-level Cargo PATH entry may still take precedence.

In either case, `cargo --version` should show both Cargo and cargoc versions, and setup status should report `shim_installed=true active_in_current_path=true`. On Windows, also check `Get-Command cargo.exe` and `cargo.exe --version`. Upgrade an existing two-script installation by running `cargoc setup --enable` with the new version. The native proxy has an integrity-checked ownership trailer and refuses to overwrite/delete unknown or corrupt files; this is not a publisher signature.

Integration remains PATH-based: IDEs/scripts using the real Cargo's absolute path or `rustup run <toolchain> cargo ...` still bypass it and need their launch command adjusted. Run `cargoc setup --disable` to remove integration; it leaves the original Cargo untouched.

Upgrade with `brew update && brew upgrade bigfish1913/tap/cargoc` or `scoop update && scoop update cargoc`, not `cargoc self-update`. If integration was enabled, run `cargoc setup --enable` again after upgrading or moving the installation, then verify the status. The generated shim stores an absolute cargoc path, which may change across package-manager upgrades, especially with Homebrew's versioned directories.

### crates.io and source builds

Install the published release from [crates.io](https://crates.io/crates/cargoc):

```sh
cargo install cargoc --locked
cargoc init
cargoc build --release
```

For the latest source version, clone the repository and install locally:

```sh
git clone https://github.com/bigfish1913/cargoc.git
cd cargoc
cargo install --path . --locked
```

`--locked` uses the committed `Cargo.lock` for reproducible dependency versions.

> **Version note:** **0.1.3** adds a native Windows `cargo.exe` proxy, managed `cargo install` caches, and read-only external-cache auditing. It retains 0.1.2's installer/package-manager channels and source-aware update notices, plus 0.1.1's Cargo-home configuration, migration, dual-version output, and self-update. After upgrading, run `cargoc setup --enable` again to create the native proxy. crates.io remains a separate publication: its **0.1.0** release uses the legacy configuration locations listed in the migration section. Use a completed GitHub Release or install from source for these features.

To choose a dedicated root, use this **instead of** the default `init` above:

```sh
cargoc init --cache-dir D:/BuildCache/cargoc
```

`init` refuses to overwrite an existing cargoc configuration; use `cargoc config` to update it. Do not point it at an existing nonempty cache such as `D:/BuildCache/cargo`: cargoc refuses to adopt nonempty, unowned directories.

## Guided setup and optional default Cargo integration

The default configuration is `$CARGO_HOME/cargoc.toml`, or `~/.cargo/cargoc.toml` if `CARGO_HOME` is unset. It sits beside Cargo's own `config.toml` without changing it. Use `CARGOC_CONFIG` or the global `--cargoc-config PATH` option to select another file.

For a first-time guided setup, run:

```powershell
cargoc setup
```

It creates the config and dedicated cache, then asks whether cargoc should become the default `cargo` command. It never overwrites Rustup's real `cargo.exe`: it creates a user-level shim and records the real Cargo path in the cargoc config.

For a one-line non-interactive setup:

```powershell
cargoc setup --enable --cache-dir D:/BuildCache/cargoc --max-cache-gib 100 --min-free-gib 20
```

Enable, inspect, or restore the integration:

```powershell
cargoc setup --enable
cargoc setup --status
cargoc setup --disable
```

The example above sets a **100 GiB** budget and an **80 GiB** pressure-cleanup target with the default `low_water_ratio = 0.8`. The built-in budget remains **80 GiB** unless you change it.

On Windows, reopen the terminal after enabling so the user-level PATH change takes effect. `--disable` removes only cargoc's shim and PATH entry; it does not delete or rename Rustup's Cargo. On Linux/macOS, cargoc does not edit shell startup files; put `~/.cargoc/bin` before the Rustup bin directory using the command printed by setup.

If you answer `N`, only the cargoc config is initialized and the normal `cargo` remains unchanged; use `cargoc build` explicitly when desired.

### Verify which Cargo is being used

After enabling integration and refreshing PATH, `cargo --version` shows both versions (example):

```text
cargo 1.99.1 (...)
cargoc 0.1.3
```

`cargo -V`, `cargo -vV`, `cargo version`, and `cargo +stable --version` also show both versions. Verbose Cargo metadata and Cargo's failure exit codes are preserved. `cargoc --version` still shows only cargoc's own version.

Setup honors an explicit `--cargo PATH` first, then an existing configured Cargo; otherwise it searches PATH in directory order, excluding cargoc's shims. It validates Cargo's identity before changing the integration and preserves Rustup's `cargo` invocation path rather than launching its resolved `rustup` target. Unowned shim files are never overwritten or deleted.

`cargoc setup --status` distinguishes registration from installed shims and whether this process's PATH actually selects the shim. Shell aliases/functions and absolute executable paths can still bypass it; check `Get-Command cargo` in PowerShell or `command -v cargo` in a POSIX shell if needed. A Windows machine-level PATH entry may take precedence over user PATH; installing the shim alone is not proof that every shell selects it.

## Everyday commands

```sh
cargoc build
cargoc check --workspace
cargoc test --locked
cargoc run --release -- application-argument
cargoc clippy --all-targets -- -D warnings
cargoc status
cargoc status --json
cargoc clean --dry-run
cargoc clean
cargoc self-update --check
cargoc self-update
cargoc self-update --rollback
```

`build`, `check`, `test`, `run`, and other non-reserved commands forward their arguments to Cargo. `--manifest-path` is supported. Use `cargoc exec +stable build` to select a Rustup toolchain explicitly.

| cargoc command | Behavior |
| --- | --- |
| `setup [settings] [--enable\|--disable\|--status]` | Configure cargoc and optionally enable, restore, or inspect default Cargo integration. |
| `init [settings]` | Create a user configuration and an owned cache root. |
| `config [settings]` | Show configuration, or update supplied settings. |
| `status [--json]` | Report managed workspace/install caches and free space on the cache volume. JSON also includes configuration. |
| `audit --path PATH` | Read-only JSON inventory of external caches: logical bytes, file counts, and existing Cargo lock observations. Never adopts, moves, or deletes files. |
| `clean [--dry-run]` | Clear **all idle managed intermediate caches**, not just old/over-budget ones. Dry run reports proposed removals without deleting cache files. |
| `self-update [--check\|--rollback\|--force]` | Check or install the latest native GitHub Release, or restore the newest local rollback backup. |
| `exec <cargo arguments...>` | Forward a command whose name collides with cargoc's own commands, or start with Cargo options/toolchain selection. |

**`cargoc clean` is not `cargo clean`.** It leaves final Cargo outputs alone. In contrast:

```sh
cargoc exec clean
cargoc exec init new-project
```

`cargoc exec clean` invokes **native Cargo clean**, which can delete final outputs from Cargo's selected target directory as well as the selected intermediates. Use it deliberately. `exec` escapes command-name collisions; it does not generally disable management.

Some commands, including Cargo `fmt`, `new`, `init`, `metadata`, `fetch`, and `update`, are direct passthroughs and print a notice that they do not use the managed cache. A cargoc configuration is still required. Managed commands reject `--build-dir`, Cargo `--config` overrides of `build.build-dir`, and Cargo `-C`; change directories or use `--manifest-path` instead.

Registry, git, and `--path` installations through `cargoc install` / `cargoc exec install` now use managed intermediate caches without requiring a workspace in the current directory. `install --list`, `install --help`, and `help install` remain passthroughs. Installs share the build lifecycle's leases, budget, disk monitoring, and cleanup. Identity includes the Cargo version, explicit toolchain, and complete arguments (local source paths normalized); only digests are stored, not URL credentials in state or cargoc logs. Identical concurrent installs are refused; different arguments may select different caches. Cargo's own output may still include source URLs.

Installed programs remain in Cargo's selected install root, and `--target-dir` keeps its native meaning; neither is collected. Never point sources, the install root, or final target-dir into the managed root through global Cargo configuration. cargoc checks explicit paths, relevant environment variables, and explicit `--config` output paths, but does not fully evaluate every implicit Cargo configuration layer.

## Updating cargoc

### Update notices

Interactive managed build commands check for updates in a separate, short-lived process; the foreground command never waits for HTTP. A successful check is cached for 24 hours under `~/.cargoc/updates/<source>/check.json`; failures are silent and retried no more than hourly. A previously fetched version is usable for up to seven days without extending its freshness after failures. A fast first command may finish before the check; the notice then appears on a later invocation.

Notices appear only on stderr after a successful build and never install anything. They are disabled for CI, redirected/non-terminal stderr, help/version, machine-readable output, `--offline`, `--frozen`, and `CARGO_NET_OFFLINE=true`. Set `CARGOC_DISABLE_UPDATE_CHECK=1` to disable them entirely. Cargo-registry installs check crates.io; other channels check stable GitHub Releases, so an unpublished GitHub version is not advertised as a crates.io update.

The suggested command follows the detected installation:

| Installation | Update command |
| --- | --- |
| Prebuilt installer / standalone binary / local source build | `cargoc self-update` |
| crates.io (`cargo install`) | `cargo install cargoc --locked` |
| Homebrew | `brew update && brew upgrade bigfish1913/tap/cargoc` |
| Scoop | `scoop update && scoop update cargoc` |

Detection is best-effort, using the executable path and Cargo's registry-install record. Direct `self-update`/rollback refuses detected package-manager installations, even with `--force`; use the corresponding package manager. `self-update --check` reports the source and appropriate command without replacing files.

### Standalone binary updates

`cargoc self-update` checks the official GitHub Releases API, selects the archive for the current OS and CPU, downloads it over HTTPS, verifies the release-provided SHA-256 file, extracts only the expected binary without unpacking other paths, and verifies its version before replacement:

```sh
cargoc self-update --check  # check only; no file changes
cargoc self-update          # download and replace cargoc
cargoc self-update --rollback
```

The updater replaces only the running `cargoc` executable. It never changes Rustup's `cargo`, Cargo's `config.toml`, `~/.cargo/cargoc.toml`, the integration shim, or the managed cache, and does not require a cache configuration. Before replacement it keeps a rollback copy under `~/.cargoc/rollback/`, isolated by installation path. `--rollback` verifies the saved copy's SHA-256 and version before restoring it; a corrupt backup is refused. Concurrent updates to the same installation are rejected. Use `--force` only when deliberately reinstalling the same or an older release. Downloads are capped at 128 MiB, extracted binaries at 64 MiB, and each HTTP request has a 60-second deadline. Unsupported platforms/ABIs are rejected (in particular, GNU releases must not replace musl builds).

Only published stable releases are selected. SHA-256 detects corrupted downloads; it is **not an independent publisher signature**. The updater trusts this project's GitHub account, release assets and HTTPS. No compatible Release exists until the release workflow has successfully published one.

Installation of updates remains explicit: background version discovery only prints a notice. Automatic installation for **standalone binaries only** is opt-in through your OS scheduler, not an implicit background service. No scheduled task is installed for you. Review the release policy, then use either example below to check and install daily; the executable is left unchanged when already current.

Windows PowerShell (creates a task for the current user, without administrator privileges; that user must be logged in):

```powershell
$action = New-ScheduledTaskAction -Execute (Get-Command cargoc -CommandType Application).Source -Argument 'self-update'
$trigger = New-ScheduledTaskTrigger -Daily -At '10:00'
Register-ScheduledTask -TaskName 'cargoc-self-update' -Action $action -Trigger $trigger -Description 'Update cargoc from its stable GitHub Release'
# Disable automatic updates:
# Unregister-ScheduledTask -TaskName 'cargoc-self-update' -Confirm:$false
```

Linux/macOS: add the following line using `crontab -e` (adjust the absolute executable path if installed elsewhere):

```cron
0 10 * * * "$HOME/.cargo/bin/cargoc" self-update >> "$HOME/.cargoc-self-update.log" 2>&1
```

Remove that line to disable automatic updates. Do not run the updater as administrator/root merely to update a user-owned installation. Keep a recoverable copy when updating an installation managed by another package manager.

## Configuration

cargoc writes its **own** TOML configuration, never your global Cargo configuration. Command-line `--cargoc-config PATH` takes precedence over `CARGOC_CONFIG`; otherwise the default path is:

- All platforms: `$CARGO_HOME/cargoc.toml` when `CARGO_HOME` is set.
- Windows default: `%USERPROFILE%/.cargo/cargoc.toml`.
- Linux/macOS default: `~/.cargo/cargoc.toml`.

Cargo's `config.toml` remains separate and is never edited. If the new default is missing, cargoc copies validated legacy settings from `%APPDATA%/cargoc/config.toml` (Windows) or `$XDG_CONFIG_HOME/cargoc/config.toml` / `~/.config/cargoc/config.toml` (Linux/macOS), retaining the old file as a recovery copy. Existing new configs are never overwritten. Explicit `--cargoc-config` / `CARGOC_CONFIG` selections do not migrate. If default Cargo integration was enabled with the old path, run `cargoc setup --enable` once to refresh its pinned config path; cache directories and thresholds remain unchanged.

```sh
cargoc --cargoc-config ./cargoc-local.toml init --cache-dir D:/BuildCache/cargoc
cargoc --cargoc-config ./cargoc-local.toml build
cargoc config
cargoc config --max-cache-gib 80 --low-water-ratio 0.8
cargoc config --auto-clean false
cargoc config --auto-clean true --abort-free-gib 1
```

Keep configuration outside the disposable cache root. All settings below are accepted by both `init` and `config`; TOML keys use underscores instead of hyphens.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--cache-dir PATH` | Platform cache directory + `cargoc/cache` | Dedicated managed root; CLI paths are made absolute. |
| `--max-cache-gib N` | `80` | Soft budget for managed intermediate bytes, not all disk usage. Must be positive. |
| `--low-water-ratio N` | `0.8` | Pressure cleanup target fraction; strictly between 0 and 1. Default target: **64 GiB**. |
| `--min-free-gib N` | `20` | Desired free space on the cache volume; also checked before starting a managed Cargo invocation. |
| `--abort-free-gib N` | `1` | Emergency threshold: stop only this invocation's Cargo process tree. `0` disables this stop. |
| `--max-age-days N` | `7` | Expire idle caches by last cargoc use, **not filesystem mtime**. `0` disables age expiry. |
| `--poll-seconds N` | `5` | Automatic cleanup interval; between `0.1` and `86400` seconds. Emergency free-space checks run more frequently (up to once per loop, approximately 250 ms by default). |
| `--auto-clean true\|false` | `true` | Enable automatic cleanup before, during, and after managed commands. Disabling it does not disable startup free-space checks or the emergency stop. |
| `--cargo PATH` | `cargo` | Real Cargo executable, not a shell command. Do not point it back to cargoc. |

The default cache base is `%LOCALAPPDATA%` on Windows and `$XDG_CACHE_HOME` on Linux/macOS; if unset, it falls back to `~/.cache`. Numeric values must be finite and nonnegative, with the stricter limits listed above. When `min_free_gib` is nonzero, `abort_free_gib` cannot exceed it.

## Ownership and cleanup safety

- Each workspace gets `<cache-dir>/workspaces/<SHA256>` based on its resolved absolute workspace path (case and verbatim-prefix aliases normalized on Windows). A root ownership marker (`.cargoc-root.json`) and validated workspace records identify managed data.
- Automatic cleanup removes oldest-last-used idle caches when they expire or when the managed budget/free-space thresholds trigger pressure. Under pressure it aims for the low-water target and desired free space, as far as safe candidates permit.
- Workspace leases, a collector lock, and Cargo native locks protect active caches. Concurrent cargoc builds of the same workspace are refused rather than sharing a writable managed cache. Locked candidates are skipped.
- Manual `clean` considers all idle managed caches, even with `auto_clean = false`; it does not clean unknown contents. Some lock files/directories may remain after cleanup.
- Changing `cache_dir` does **not** migrate or delete the old root. Existing non-cargoc caches remain untouched and are not counted toward the managed budget. Never add ownership markers by hand to adopt an old cache.
- The wrapper supplies `build.build-dir` for each managed invocation. It does not change Cargo's original target-dir configuration or move existing outputs. Old caches must be handled separately by their owner.
- Emergency shutdown targets this invocation's Windows Job Object or Unix process group only, not unrelated Cargo/rustc processes. This is process ownership, not a sandbox for arbitrary build scripts.

**This is a soft cache budget, not a hard disk quota or a guarantee against a full disk.** The current build can exceed the budget, active caches are protected, and space checks are periodic. Final outputs, registries, other caches, and unrelated programs can consume space outside this budget. A cache on a different volume cannot protect the final-output volume. A managed build is refused if startup free space remains below the configured minimum/emergency threshold after any enabled cleanup.

Only commands actually launched through cargoc are managed. Installing cargoc alone does not change the normal `cargo` command. After enabling the shim and putting it first on PATH, bare `cargo build`, `cargo test`, and similar commands run through cargoc. IDEs or scripts that invoke the real Cargo executable by absolute path still bypass it.

Automatic cleanup and Cargo exit events are written to `events.jsonl` beside the cargoc configuration. The log is rotated after approximately 2 MiB. Cleanup/logging failures are reported as warnings and do not replace Cargo's exit code; unsafe candidates are never force-deleted. Size reports use logical bytes, so physical disk space reclaimed can differ (for example, with hard links).

The dedicated cache root is disposable storage: do not put source files, manual notes, or final `target-dir` outputs inside it. Symlinks/reparse points in cache paths are refused. This is a trusted-user maintenance tool, not protection against another process maliciously replacing filesystem entries during cleanup.

### Handling external legacy caches

A global Cargo setting such as `build.build-dir = "D:/BuildCache/cargo/{workspace-path-hash}"` still sends bypassed invocations to that directory, independently of `D:/BuildCache/cargoc`. Fix the launch commands first, then inventory the legacy data:

```sh
cargoc audit --path D:/BuildCache/cargo
cargoc status --json
cargoc clean --dry-run
```

`audit` requires no initialized configuration and does not migrate legacy settings. It refuses symlinks/reparse points, overlap with the managed root, and directories containing cargoc ownership markers. `busy` indicates observed contention; `unavailable` means activity could not be assessed. Even `available` or missing locks **do not authorize deletion**: writers may resume after inspection, and final outputs may be present.

The original owner must handle legacy data separately: stop related IDEs, builds and background tasks, verify the original build-dir/target-dir, retain needed final artifacts and recovery copies, then explicitly decide on removal. Never move old directories into cargoc's root, add ownership markers, or point native Cargo's global build-dir into cargoc's `workspaces`: these bypass leases and risk deletion during a build. `audit` intentionally has no automatic deletion option, and `cargoc clean` leaves external caches alone.

## Prebuilt releases

Pushes to `main` automatically publish a new package version through `.github/workflows/release.yml`, using native GitHub-hosted runners for:

| Target | Archive |
| --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` (`.zip`) |
| Windows ARM64 | `aarch64-pc-windows-msvc` (`.zip`) |
| Linux x64 | `x86_64-unknown-linux-gnu` (`.tar.gz`) |
| Linux ARM64 | `aarch64-unknown-linux-gnu` (`.tar.gz`) |
| macOS Intel | `x86_64-apple-darwin` (`.tar.gz`) |
| macOS Apple Silicon | `aarch64-apple-darwin` (`.tar.gz`) |

After a successful release build, archives are available from [GitHub Releases](https://github.com/bigfish1913/cargoc/releases), for example `cargoc-v0.1.3-x86_64-pc-windows-msvc.zip`. Each archive contains the `cargoc` executable, both README files, and the MIT license. Each has a `.sha256` sidecar; the Release also includes a combined `SHA256SUMS` file.

Download the archive for your architecture, verify its checksum, extract it, and put **only `cargoc` / `cargoc.exe`** in a directory on PATH. Never overwrite Rustup's `cargo` executable. A Rust compiler is not needed to install the prebuilt program, but managed builds still require Cargo 1.91+. Linux archives use glibc, built on Ubuntu 22.04 (glibc 2.35); Alpine/musl binaries are not provided. Windows/macOS binaries are not Authenticode/Developer ID signed or notarized.

For maintainers, update `Cargo.toml` **and `Cargo.lock`** to a new version (for example, `0.1.3`), then commit and push the reviewed changes to `main`:

```sh
git add Cargo.toml Cargo.lock # also stage the reviewed source/docs changes
git commit -m "Release 0.1.3"
git push origin main
```

Stable releases also attach generated `cargoc.rb` and `cargoc.json` manifests. The optional channel-sync job needs a narrowly scoped `CHANNELS_TOKEN` secret with Contents write access to the two channel repositories; without it, use the documented local sync command. Never store a token in the repository.

No manual tag is needed. After formatting, Clippy, MSRV, all six native test/build jobs, packaged-binary smoke tests and checksum verification succeed, the workflow creates `v{version}` on the **exact tested commit**, uploads assets to a draft, then publishes it. It does not increment package versions or commit changes back to `main`. A push with an already-published version skips publication; bump the version to release again. Matching `v*` tag pushes remain supported and must exactly match the package version.

Versions such as `0.1.3-rc.1` become prereleases. Existing releases/assets and tags pointing to another commit are never overwritten. A failed upload may leave a draft that must be inspected and removed before retrying. Tag/Release creation by `GITHUB_TOKEN` does not start another release run. This does not publish to crates.io.

For a build-only run, use **Actions → Release → Run workflow** (`workflow_dispatch`): it uploads Actions artifacts retained for 14 days, without creating a tag or publishing a Release. GitHub-hosted runner labels can change; update the matrix if necessary.

## Development

CI runs on Windows, Ubuntu, and macOS with stable Rust and the `rustfmt`/`clippy` components:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The workflow sets `CARGO_TARGET_DIR` and `CARGO_BUILD_BUILD_DIR` to separate runner-local temporary directories. It runs Cargo directly, not through cargoc, and does not change global configuration. A separate Rust 1.88 job checks all targets against the declared MSRV.

## Contributors

Contributions, bug reports, and documentation improvements are welcome. See the [contributor list](https://github.com/bigfish1913/cargoc/graphs/contributors) for everyone who has contributed to this repository.

[![cargoc contributors](https://contrib.rocks/image?repo=bigfish1913/cargoc)](https://github.com/bigfish1913/cargoc/graphs/contributors)

Contributor avatars are generated by [contrib.rocks](https://contrib.rocks).

## Star history

[![Star history for cargoc](https://api.star-history.com/svg?repos=bigfish1913/cargoc&type=Date)](https://www.star-history.com/#bigfish1913/cargoc&Date)

[View the interactive Star history](https://www.star-history.com/#bigfish1913/cargoc&Date) · [Star this repository](https://github.com/bigfish1913/cargoc/stargazers)

Charts and avatars use third-party services and may be temporarily unavailable because of API rate limits. The links above remain available if images cannot load.

## License

[MIT](LICENSE). Copyright 2026 cargoc contributors.

