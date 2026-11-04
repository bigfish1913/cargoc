# cargoc

A Rust CLI that wraps Cargo with a centralized, owned intermediate-build cache and disk-pressure cleanup. Final build artifacts stay at Cargo's original target directory: the workspace's `target/` by default, or the location selected by your existing Cargo configuration, environment, or `--target-dir`.

[简体中文](README.zh-CN.md) · [GitHub](https://github.com/bigfish1913/cargoc) · [crates.io](https://crates.io/crates/cargoc)

## Requirements and installation

- Rust **1.88+** to compile cargoc (MSRV).
- The Cargo executable wrapped for managed builds must be **1.91+**, with stable `build.build-dir` support. An older Cargo may compile cargoc but cannot run its managed builds.
- Windows, Linux, or macOS. This is a native Rust application, not a Python script.

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

> **Version note:** The **0.1.1** source/GitHub release line adds Cargo-home configuration (`~/.cargo/cargoc.toml`), legacy-config migration, dual-version output, improved Windows shims, and self-update. crates.io remains a separate publication: its **0.1.0** release uses the legacy configuration locations listed in the migration section. Use a completed GitHub Release or install from source for these features.

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
cargoc 0.1.1
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
| `status [--json]` | Report managed cache sizes, workspace records, and free space on the cache volume. JSON also includes configuration. |
| `clean [--dry-run]` | Clear **all idle managed intermediate caches**, not just old/over-budget ones. Dry run reports proposed removals without deleting cache files. |
| `self-update [--check\|--rollback\|--force]` | Check or install the latest native GitHub Release, or restore the newest local rollback backup. |
| `exec <cargo arguments...>` | Forward a command whose name collides with cargoc's own commands, or start with Cargo options/toolchain selection. |

**`cargoc clean` is not `cargo clean`.** It leaves final Cargo outputs alone. In contrast:

```sh
cargoc exec clean
cargoc exec init new-project
```

`cargoc exec clean` invokes **native Cargo clean**, which can delete final outputs from Cargo's selected target directory as well as the selected intermediates. Use it deliberately. `exec` escapes command-name collisions; it does not generally disable management.

Some commands, including Cargo `fmt`, `new`, `init`, `install`, `metadata`, `fetch`, and `update`, are direct passthroughs and print a notice that they do not use the managed cache. A cargoc configuration is still required. Managed commands reject `--build-dir`, Cargo `--config` overrides of `build.build-dir`, and Cargo `-C`; change directories or use `--manifest-path` instead.

## Updating cargoc

`cargoc self-update` checks the official GitHub Releases API, selects the archive for the current OS and CPU, downloads it over HTTPS, verifies the release-provided SHA-256 file, extracts only the expected binary without unpacking other paths, and verifies its version before replacement:

```sh
cargoc self-update --check  # check only; no file changes
cargoc self-update          # download and replace cargoc
cargoc self-update --rollback
```

The updater replaces only the running `cargoc` executable. It never changes Rustup's `cargo`, Cargo's `config.toml`, `~/.cargo/cargoc.toml`, the integration shim, or the managed cache, and does not require a cache configuration. Before replacement it keeps a rollback copy under `~/.cargoc/rollback/`, isolated by installation path. `--rollback` verifies the saved copy's SHA-256 and version before restoring it; a corrupt backup is refused. Concurrent updates to the same installation are rejected. Use `--force` only when deliberately reinstalling the same or an older release. Downloads are capped at 128 MiB, extracted binaries at 64 MiB, and each HTTP request has a 60-second deadline. Unsupported platforms/ABIs are rejected (in particular, GNU releases must not replace musl builds).

Only published stable releases are selected. SHA-256 detects corrupted downloads; it is **not an independent publisher signature**. The updater trusts this project's GitHub account, release assets and HTTPS. No compatible Release exists until the release workflow has successfully published one.

Updates are explicit by default: normal Cargo/build invocations do not contact GitHub. Automatic updates are opt-in through your OS scheduler, not an implicit background service. No scheduled task is installed for you. Review the release policy, then use either example below to check and install daily; the executable is left unchanged when already current.

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

After a successful release build, archives are available from [GitHub Releases](https://github.com/bigfish1913/cargoc/releases), for example `cargoc-v0.1.1-x86_64-pc-windows-msvc.zip`. Each archive contains the `cargoc` executable, both README files, and the MIT license. Each has a `.sha256` sidecar; the Release also includes a combined `SHA256SUMS` file.

Download the archive for your architecture, verify its checksum, extract it, and put **only `cargoc` / `cargoc.exe`** in a directory on PATH. Never overwrite Rustup's `cargo` executable. A Rust compiler is not needed to install the prebuilt program, but managed builds still require Cargo 1.91+. Linux archives use glibc, built on Ubuntu 22.04 (glibc 2.35); Alpine/musl binaries are not provided. Windows/macOS binaries are not Authenticode/Developer ID signed or notarized.

For maintainers, update `Cargo.toml` **and `Cargo.lock`** to a new version (for example, `0.1.2`), then commit and push the reviewed changes to `main`:

```sh
git add Cargo.toml Cargo.lock # also stage the reviewed source/docs changes
git commit -m "Release 0.1.2"
git push origin main
```

No manual tag is needed. After formatting, Clippy, MSRV, all six native test/build jobs, packaged-binary smoke tests and checksum verification succeed, the workflow creates `v{version}` on the **exact tested commit**, uploads assets to a draft, then publishes it. It does not increment package versions or commit changes back to `main`. A push with an already-published version skips publication; bump the version to release again. Matching `v*` tag pushes remain supported and must exactly match the package version.

Versions such as `0.1.2-rc.1` become prereleases. Existing releases/assets and tags pointing to another commit are never overwritten. A failed upload may leave a draft that must be inspected and removed before retrying. Tag/Release creation by `GITHUB_TOKEN` does not start another release run. This does not publish to crates.io.

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

