# cargoc

A Rust CLI that wraps Cargo with a centralized, owned intermediate-build cache and disk-pressure cleanup. Final build artifacts stay at Cargo's original target directory: the workspace's `target/` by default, or the location selected by your existing Cargo configuration, environment, or `--target-dir`.

[简体中文](README.zh-CN.md)

## Requirements and installation

- Rust **1.88+** to compile cargoc (MSRV).
- The Cargo executable wrapped for managed builds must be **1.91+**, with stable `build.build-dir` support. An older Cargo may compile cargoc but cannot run its managed builds.
- Windows, Linux, or macOS. This is a native Rust application, not a Python script.

From this source checkout:

```sh
cargo install --path . --locked
cargoc init
cargoc build --release
```

`--locked` requires the checkout's committed `Cargo.lock`. No crates.io publication or release downloads are assumed here.

To choose a dedicated root, use this **instead of** the default `init` above:

```sh
cargoc init --cache-dir D:/BuildCache/cargoc
```

`init` refuses to overwrite an existing cargoc configuration; use `cargoc config` to update it. Do not point it at an existing nonempty cache such as `D:/BuildCache/cargo`: cargoc refuses to adopt nonempty, unowned directories.

## Guided setup and optional default Cargo integration

The default configuration is `%APPDATA%\\cargoc\\config.toml` on Windows and `$XDG_CONFIG_HOME/cargoc/config.toml` on Linux/macOS (falling back to `~/.config/cargoc/config.toml`). Use `CARGOC_CONFIG` or the global `--cargoc-config PATH` option to select another file.

For a first-time guided setup, run:

```powershell
cargoc setup
```

It creates the config and dedicated cache, then asks whether cargoc should become the default `cargo` command. It never overwrites Rustup's real `cargo.exe`: it creates a user-level shim and records the real Cargo path in the cargoc config.

For a one-line non-interactive setup:

```powershell
cargoc setup --enable --cache-dir D:/BuildCache/cargoc --max-cache-gib 80 --min-free-gib 20
```

Inspect, enable, or restore the integration:

```powershell
cargoc setup --status
cargoc setup --disable
```

On Windows, reopen the terminal after enabling so the user-level PATH change takes effect. `--disable` removes only cargoc's shim and PATH entry; it does not delete or rename Rustup's Cargo. On Linux/macOS, cargoc does not edit shell startup files; put `~/.cargoc/bin` before the Rustup bin directory using the command printed by setup.

If you answer `N`, only the cargoc config is initialized and the normal `cargo` remains unchanged; use `cargoc build` explicitly when desired.

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
```

`build`, `check`, `test`, `run`, and other non-reserved commands forward their arguments to Cargo. `--manifest-path` is supported. Use `cargoc exec +stable build` to select a Rustup toolchain explicitly.

| cargoc command | Behavior |
| --- | --- |
| `init [settings]` | Create a user configuration and an owned cache root. |
| `config [settings]` | Show configuration, or update supplied settings. |
| `status [--json]` | Report managed cache sizes, workspace records, and free space on the cache volume. JSON also includes configuration. |
| `clean [--dry-run]` | Clear **all idle managed intermediate caches**, not just old/over-budget ones. Dry run reports proposed removals without deleting cache files. |
| `exec <cargo arguments...>` | Forward a command whose name collides with cargoc's own commands, or start with Cargo options/toolchain selection. |

**`cargoc clean` is not `cargo clean`.** It leaves final Cargo outputs alone. In contrast:

```sh
cargoc exec clean
cargoc exec init new-project
```

`cargoc exec clean` invokes **native Cargo clean**, which can delete final outputs from Cargo's selected target directory as well as the selected intermediates. Use it deliberately. `exec` escapes command-name collisions; it does not generally disable management.

Some commands, including Cargo `fmt`, `new`, `init`, `install`, `metadata`, `fetch`, and `update`, are direct passthroughs and print a notice that they do not use the managed cache. A cargoc configuration is still required. Managed commands reject `--build-dir`, Cargo `--config` overrides of `build.build-dir`, and Cargo `-C`; change directories or use `--manifest-path` instead.

## Configuration

cargoc writes its **own** TOML configuration, never your global Cargo configuration. Command-line `--cargoc-config PATH` takes precedence over `CARGOC_CONFIG`; otherwise the default path is:

- Windows: `%APPDATA%/cargoc/config.toml`.
- Linux/macOS: `$XDG_CONFIG_HOME/cargoc/config.toml`, or `~/.config/cargoc/config.toml`.
- If `%APPDATA%` is unset, Windows also falls back to `~/.config/cargoc/config.toml`.

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

Only commands actually launched through cargoc are managed. Bare `cargo` and IDEs that invoke an absolute Cargo executable bypass it. Configure integrations explicitly; installing cargoc does not transparently replace Cargo.

Automatic cleanup and Cargo exit events are written to `events.jsonl` beside the cargoc configuration. The log is rotated after approximately 2 MiB. Cleanup/logging failures are reported as warnings and do not replace Cargo's exit code; unsafe candidates are never force-deleted. Size reports use logical bytes, so physical disk space reclaimed can differ (for example, with hard links).

The dedicated cache root is disposable storage: do not put source files, manual notes, or final `target-dir` outputs inside it. Symlinks/reparse points in cache paths are refused. This is a trusted-user maintenance tool, not protection against another process maliciously replacing filesystem entries during cleanup.

## Development

CI runs on Windows, Ubuntu, and macOS with stable Rust and the `rustfmt`/`clippy` components:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The workflow sets `CARGO_TARGET_DIR` and `CARGO_BUILD_BUILD_DIR` to separate runner-local temporary directories. It runs Cargo directly, not through cargoc, and does not change global configuration. A separate Rust 1.88 job checks all targets against the declared MSRV.

## License

[MIT](LICENSE). Copyright 2026 cargoc contributors.

