# cargoc

原生 Rust CLI：为 Cargo 提供集中管理的中间构建缓存及磁盘压力清理。最终产物仍在 Cargo 原本的目标目录：默认为工作区的 `target/`，也可由现有 Cargo 配置、环境变量或 `--target-dir` 指定。

[English](README.md) · [GitHub](https://github.com/bigfish1913/cargoc) · [crates.io](https://crates.io/crates/cargoc)

## 环境要求与安装

- 编译 cargoc 需要 **Rust 1.88+**（MSRV）。
- 受包装的 Cargo 执行受管理构建需要 **1.91+**，以支持稳定的 `build.build-dir`。较旧的 Cargo 可能可以编译 cargoc，但不能执行受管理构建。
- 支持 Windows、Linux、macOS。这是 Rust 程序，不是 Python 脚本。

### 预编译一键安装（不需要 Rust 编译器）

Linux/macOS：

```sh
curl -fsSL https://raw.githubusercontent.com/bigfish1913/cargoc/main/scripts/install.sh | sh
# 下载脚本后，可以指定版本预览，不下载程序：
# sh scripts/install.sh --version 0.1.3 --dry-run
```

Windows PowerShell 5.1+（x64／ARM64）：

```powershell
& ([scriptblock]::Create((Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/bigfish1913/cargoc/main/scripts/install.ps1).Content))
# 下载脚本后：.\scripts\install.ps1 -Version 0.1.3 -DryRun
```

不希望直接执行远程代码时，请先下载并检查脚本。两种安装器都强制校验 Release 的 SHA-256 和程序版本，拒绝覆盖已有安装，只安装 `cargoc`，不替换 Rustup 的 `cargo`，不改动 PATH／配置，也不启用接管。Unix 默认安装到 `~/.local/bin`，Windows 默认安装到 `%LOCALAPPDATA%/Programs/cargoc`；可用 `--dir`／`-Dir` 指定其他目录，再按输出提示添加 PATH。

### 包管理器

```sh
# macOS Intel／Apple Silicon、Linux x64／ARM64（glibc 2.35+）
brew tap bigfish1913/tap
brew install cargoc

# Windows x64／ARM64
scoop bucket add bigfish1913 https://github.com/bigfish1913/scoop-bucket
scoop install cargoc
```

[Homebrew tap](https://github.com/bigfish1913/homebrew-tap) 和 [Scoop bucket](https://github.com/bigfish1913/scoop-bucket) 使用实际发行包的哈希。这是作者维护的安装源，不是 Homebrew core 或微软 winget；维护说明见 [安装渠道](packaging/README.md)。

#### 安装后接管默认 Cargo 命令

Homebrew／Scoop 安装的只是 `cargoc`，**不会自动启用接管，也不会替换真实 Cargo 可执行文件**。请先安装 Cargo，再按下面的步骤让直接输入的 `cargo` 命令经过 cargoc。

**macOS／Homebrew**（Linux 上的 Homebrew 同样适用）：

```sh
cargoc setup --enable
export PATH="$HOME/.cargoc/bin:$PATH"
command -v cargo
cargo --version
cargoc setup --status
```

`export` 只对当前 shell 生效。需要持久生效时，将这一行加入对应的 shell 启动文件，例如 macOS 默认交互式 zsh 的 `~/.zshrc`；setup 不会自动修改它。`command -v cargo` 应指向 `~/.cargoc/bin/cargo`，该目录必须排在真实 Cargo 目录前面。

**Windows／Scoop**，在 PowerShell 中执行：

```powershell
cargoc setup --enable
# 重新打开终端后，再执行下面的检查：
Get-Command cargo
cargo --version
cargoc setup --status
```

Setup 会把 `%USERPROFILE%\.cargoc\bin` 放到**用户级 PATH** 前面，并创建原生 `cargo.exe` 代理及兼容的 `cargo.cmd`、`cargo` shell 脚本。默认 PATHEXT 下，`Get-Command cargo` 应指向该目录中的 `cargo.exe`；直接通过 PATH 启动 `cargo.exe` 的工具也会经过代理。代理逐项转发原始参数、保留退出码，并固定使用 setup 时选择的 cargoc 配置。已有终端和 IDE 需要重启才能继承新 PATH，系统级 PATH 中的 Cargo 项仍可能优先。

两种方式下，`cargo --version` 都应显示 Cargo 与 cargoc 两个版本，接管状态应包含 `shim_installed=true active_in_current_path=true`。Windows 还应检查 `Get-Command cargo.exe` 和 `cargo.exe --version`。已有两脚本接管需要用新版本运行一次 `cargoc setup --enable` 才会补上原生代理。代理带完整性校验，不会覆盖或删除未知／损坏的同名文件；它不是发布者签名。

这是基于 PATH 的接管，不会拦截所有 Cargo 调用：IDE／脚本使用真实 Cargo 的绝对路径或 `rustup run <toolchain> cargo ...` 仍会绕过接管，需要调整这些工具的启动入口。恢复原生入口可执行 `cargoc setup --disable`，不会删除或替换原 Cargo。

升级请使用 `brew update && brew upgrade bigfish1913/tap/cargoc` 或 `scoop update && scoop update cargoc`，不要使用 `cargoc self-update`。若此前已启用接管，升级或移动安装位置后，请再次运行 `cargoc setup --enable` 并检查状态：生成的 shim 保存的是 cargoc 绝对路径，包管理器升级时该路径可能变化，尤其是 Homebrew 的版本目录。

### crates.io 与源码安装

通过 [crates.io](https://crates.io/crates/cargoc) 安装已发布版本：

```sh
cargo install cargoc --locked
cargoc init
cargoc build --release
```

如需最新源码版本，克隆仓库后本地安装：

```sh
git clone https://github.com/bigfish1913/cargoc.git
cd cargoc
cargo install --path . --locked
```

`--locked` 使用已提交的 `Cargo.lock`，固定依赖版本。

> **版本说明：** **0.1.5** 修复健康磁盘且缓存接近上限时的自动清理抖动：保留近期缓存、保护当前工作区，并仅在磁盘压力下进行激进回收；保留 0.1.4 的旧版外部 Cargo `build-dir` 用户选择清理、Cargo 配置备份和原生 `cargo clean` 透传，以及更早版本的 Windows 原生代理、受管理安装缓存和非托管旧缓存只读审计。升级后需再次运行 `cargoc setup --enable` 刷新原生代理。crates.io 是独立发布渠道，其 **0.1.0** 仍使用迁移说明中列出的旧配置位置。需要这些新功能时，请使用已完成的 GitHub Release 或从源码安装。

如需专用缓存根目录，用下面的命令**替代**上面的默认 `init`：

```sh
cargoc init --cache-dir D:/BuildCache/cargoc
```

`init` 不覆盖已有配置；修改请用 `cargoc config`。不要指定已有非空缓存，例如 `D:/BuildCache/cargo`：程序拒绝接管非空且没有所有权标记的目录。

## 一次引导配置与默认 Cargo 接管

配置文件默认位于 `$CARGO_HOME/cargoc.toml`，未设置 `CARGO_HOME` 时为 `~/.cargo/cargoc.toml`。它与 Cargo 自己的 `config.toml` 同目录；除非用户明确选择移除检测到的外部动态 `build-dir`，否则 setup 不修改 Cargo 配置。也可以通过 `CARGOC_CONFIG` 或全局参数 `--cargoc-config PATH` 指定。

推荐首次使用直接运行：

```powershell
cargoc setup
```

它会创建配置和专用缓存目录，然后询问是否让 `cargoc` 接管默认的 `cargo` 命令。接管不会覆盖 Rustup 的真实 `cargo.exe`，而是在用户目录创建 shim，并把真实 Cargo 路径保存到配置中。

不需要交互时：

```powershell
cargoc setup --enable --cache-dir D:/BuildCache/cargoc --max-cache-gib 100 --min-free-gib 20
```

启用、检查状态或恢复原生 Cargo：

```powershell
cargoc setup --enable
cargoc setup --status
cargoc setup --disable
```

如果 setup 检测到类似下面的全局 Cargo 配置：

```toml
build-dir = "D:/BuildCache/cargo/{workspace-path-hash}"
```

交互式 setup 会询问是否移除。移除前会备份原 Cargo 配置。检测到该配置时，非交互式 setup 需要明确选择（`--yes` 为兼容性保留）；可显式选择：

```powershell
cargoc setup --enable --legacy-build-dir keep
cargoc setup --enable --legacy-build-dir remove
```

只有位于 cargoc 管理缓存之外的绝对动态构建目录才会被列为可移除对象；其他 Cargo 配置和无动态占位符的静态 `build-dir` 不会修改。

上例将预算设为 **100 GiB**，使用默认 `low_water_ratio = 0.8` 时，压力清理目标为 **80 GiB**。程序内置的默认预算仍为 **80 GiB**，不会自动改成 100 GiB。

Windows 启用后需要重新打开终端，让用户级 PATH 生效。`--disable` 只移除 cargoc 自己的 shim 和 PATH 项，不删除、不改名 Rustup 的 Cargo。Linux/macOS 不会自动修改 shell 启动文件，启用后按命令输出把 `~/.cargoc/bin` 放到 PATH 前面。

如果不想接管默认命令，也可以只初始化配置：在引导问题中回答 `N`，以后使用 `cargoc build`；原生 `cargo` 将保持不变。

### 确认实际使用的 Cargo

启用接管并刷新 PATH 后，`cargo --version` 同时展示两个版本，例如：

```text
cargo 1.99.1 (...)
cargoc 0.1.3
```

`cargo -V`、`cargo -vV`、`cargo version`、`cargo +stable --version` 同样展示两个版本。Cargo 的详细版本元数据和失败退出码保持不变。`cargoc --version` 仍只显示 cargoc 自身版本。

接管时优先使用显式指定的 `--cargo PATH`，其次沿用已有配置中的 Cargo；否则按 PATH 目录顺序查找，并排除 cargoc 自己的 shim。修改集成前会验证 Cargo 身份，保留 Rustup 的 `cargo` 调用路径，不把符号链接解析成 `rustup` 后直接启动。不覆盖或删除不属于 cargoc 的 shim 文件。

`cargoc setup --status` 会区分是否已登记、shim 是否完整安装，以及当前进程的 PATH 是否真正选中了 shim。Shell 别名、函数和可执行文件绝对路径仍可绕过接管；需要时用 PowerShell 的 `Get-Command cargo` 或 POSIX shell 的 `command -v cargo` 确认。Windows 系统级 PATH 项可能优先于用户 PATH，因此仅安装 shim 不代表所有终端都已接管。

## 日常使用

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

`build`、`check`、`test`、`run` 等非保留命令将参数转发给 Cargo。支持 `--manifest-path`。显式选择 Rustup 工具链可用 `cargoc exec +stable build`。

| cargoc 命令 | 行为 |
| --- | --- |
| `setup [设置项] [--enable\|--disable\|--status]` | 配置 cargoc，并可选择启用、恢复或检查默认 Cargo 接管。 |
| `init [设置项]` | 创建用户配置和带所有权标记的缓存根目录。 |
| `config [设置项]` | 显示配置，或更新指定设置项。 |
| `status [--json]` | 显示受管理工作区／安装缓存大小及缓存所在卷的剩余空间；JSON 还包含配置。 |
| `audit --path PATH` | 只读盘点非托管旧缓存的逻辑字节、文件数和已有 Cargo 锁状态，输出 JSON；不接管、迁移或删除。 |
| `clean [--dry-run]` | 清空**所有空闲的受管理中间缓存**，不只处理过期或超预算缓存。预演报告拟删除内容，不删除缓存文件。 |
| `self-update [--check\|--rollback\|--force]` | 检查或安装最新 GitHub Release，也可恢复本地最新回滚备份。 |
| `exec <Cargo 参数...>` | 转发与 cargoc 命令重名的 Cargo 命令，或以 Cargo 选项、工具链选择器开头的调用。 |

**`cargoc clean` 不等于 `cargo clean`。** 前者不删除最终产物。相反：

```sh
cargoc exec clean
cargoc exec init new-project
```

`cargoc exec clean` 执行 **Cargo 原生 clean**，可能删除 Cargo 所选目标目录中的最终产物及选定的中间缓存，请谨慎使用。`exec` 消除命令名冲突，不是通用的“关闭管理”开关。

部分命令（例如 Cargo `clean`、`fmt`、`new`、`init`、`metadata`、`fetch`、`update`）直接透传，并提示不使用受管理缓存，但仍需要 cargoc 配置。受管理命令拒绝 `--build-dir`、通过 Cargo `--config` 覆盖 `build.build-dir`，以及 Cargo `-C`；请切换工作目录或使用 `--manifest-path`。

`cargoc install`／`cargoc exec install` 的 registry、git 和 `--path` 安装现在使用受管理中间缓存，无需在当前目录存在工作区。`install --list`、`install --help` 和 `help install` 仍直接透传。安装缓存与构建共用租约、预算、空间监测及清理规则；缓存身份包含 Cargo 版本、显式工具链和完整参数（本地源路径规范化），只保存摘要，不把 URL 凭据写入状态或 cargoc 日志。同一身份的并发安装会被拒绝；参数不同可能生成不同缓存。Cargo 自己的输出仍可能包含源地址。

已安装程序仍在 Cargo 选择的安装根目录，`--target-dir` 也保留原生语义；两者不由 cargoc 清理。不要通过全局 Cargo 配置把源码、安装根目录或最终 target-dir 指进受管理根目录。cargoc 检查显式路径、相关环境变量和显式 `--config` 输出路径，但不会完整求值 Cargo 所有隐式配置层级。

## 更新 cargoc

### 更新提示

交互式受管理构建通过独立的短生命周期进程检查新版本，前台不等待 HTTP。成功结果缓存 24 小时，保存在 `~/.cargoc/updates/<source>/check.json`；失败静默处理，最多每小时重试。旧版本信息最多沿用七天，失败不会延长成功结果的新鲜度。第一次命令执行很快时，检查可能稍后完成，提示会在后续调用中出现。

提示只在构建成功后写入 stderr，不会自动安装。CI、stderr 非终端／被重定向、帮助／版本、机器可读输出、`--offline`、`--frozen` 和 `CARGO_NET_OFFLINE=true` 场景均不检查。设置 `CARGOC_DISABLE_UPDATE_CHECK=1` 可完全关闭。通过 Cargo registry 安装时检查 crates.io，其他来源检查 GitHub 稳定版，避免误把尚未发布到 crates.io 的版本当作 Cargo 渠道更新。

升级命令按检测到的安装来源提示：

| 安装来源 | 升级命令 |
| --- | --- |
| 一键安装／独立二进制／本地源码构建 | `cargoc self-update` |
| crates.io（`cargo install`） | `cargo install cargoc --locked` |
| Homebrew | `brew update && brew upgrade bigfish1913/tap/cargoc` |
| Scoop | `scoop update && scoop update cargoc` |

来源检测基于程序路径和 Cargo 的 registry 安装记录，属于尽力识别。识别到包管理器安装后，直接 `self-update`／回滚会被拒绝，即使传入 `--force` 也不会覆盖其文件；请使用对应包管理器。`self-update --check` 会报告来源与合适的升级命令，不替换文件。

### 独立二进制更新

`cargoc self-update` 会访问官方 GitHub Releases API，根据当前系统和 CPU 选择对应压缩包，通过 HTTPS 下载，校验 Release 提供的 SHA-256 文件，只提取预期位置的二进制、不展开其余路径，并在替换前验证版本：

```sh
cargoc self-update --check  # 只检查，不修改文件
cargoc self-update          # 下载并替换 cargoc
cargoc self-update --rollback
```

更新只替换当前运行的 `cargoc` 可执行文件，绝不修改 Rustup 的 `cargo`、Cargo 的 `config.toml`、`~/.cargo/cargoc.toml`、接管 shim 或受管理缓存，也不要求先配置缓存。替换前在 `~/.cargoc/rollback/` 按安装路径隔离保存回滚副本；`--rollback` 校验副本的 SHA-256 和版本后恢复，损坏的副本会被拒绝。同一安装的并发更新会被拒绝。只有明确需要重装相同或更旧版本时才使用 `--force`。下载上限为 128 MiB、提取的二进制上限为 64 MiB，每个 HTTP 请求限时 60 秒；不支持的平台或 ABI 直接拒绝（例如不能用 GNU 发行包替换 musl 构建）。

仅选择已公开的稳定版 Release。SHA-256 用来检测下载损坏，**不等于独立的发布者签名**；更新器信任本项目的 GitHub 账号、Release 资产及 HTTPS。必须先由发布工作流成功发布兼容的 Release，才有可下载的更新。

**安装更新仍须显式触发**：后台发现新版本只显示提示。仅对**独立二进制**，可通过操作系统计划任务主动启用自动安装更新，而不是隐式后台服务；安装 cargoc 不会替你创建任务。确认发布策略后，可用下面任一方式每日自动检查并安装；已是最新版本时不改动可执行文件。

Windows PowerShell（为当前用户创建任务，无需管理员权限；执行时该用户需已登录）：

```powershell
$action = New-ScheduledTaskAction -Execute (Get-Command cargoc -CommandType Application).Source -Argument 'self-update'
$trigger = New-ScheduledTaskTrigger -Daily -At '10:00'
Register-ScheduledTask -TaskName 'cargoc-self-update' -Action $action -Trigger $trigger -Description 'Update cargoc from its stable GitHub Release'
# 关闭自动更新：
# Unregister-ScheduledTask -TaskName 'cargoc-self-update' -Confirm:$false
```

Linux/macOS：用 `crontab -e` 添加下面一行（若安装在其他位置，请调整可执行文件路径）：

```cron
0 10 * * * "$HOME/.cargo/bin/cargoc" self-update >> "$HOME/.cargoc-self-update.log" 2>&1
```

删除该行即可关闭自动更新。不要仅为更新用户目录中的程序而使用管理员／root 权限；更新由其他包管理器托管的安装前，请保留可恢复副本。

## 配置

cargoc 只写入**自己的** TOML 配置，不修改全局 Cargo 配置。`--cargoc-config PATH` 优先于环境变量 `CARGOC_CONFIG`；否则使用默认路径：

- 所有平台：设置 `CARGO_HOME` 时使用 `$CARGO_HOME/cargoc.toml`。
- Windows 默认：`%USERPROFILE%/.cargo/cargoc.toml`。
- Linux/macOS 默认：`~/.cargo/cargoc.toml`。

Cargo 自己的 `config.toml` 默认保持独立；只有 setup 中用户明确选择移除外部动态 `build-dir` 时才会修改，并会先留下备份。新默认文件不存在时，会从旧位置 `%APPDATA%/cargoc/config.toml`（Windows）或 `$XDG_CONFIG_HOME/cargoc/config.toml` / `~/.config/cargoc/config.toml`（Linux/macOS）复制经过校验的配置，保留原文件作为恢复副本，不覆盖已有的新配置。显式指定 `--cargoc-config` / `CARGOC_CONFIG` 时不迁移。此前已启用默认 Cargo 接管的用户，运行一次 `cargoc setup --enable`，即可刷新入口固定的配置路径；缓存目录、阈值不会改变。

```sh
cargoc --cargoc-config ./cargoc-local.toml init --cache-dir D:/BuildCache/cargoc
cargoc --cargoc-config ./cargoc-local.toml build
cargoc config
cargoc config --max-cache-gib 80 --low-water-ratio 0.8
cargoc config --auto-clean false
cargoc config --auto-clean true --abort-free-gib 1
```

配置文件必须放在可清理缓存根目录之外。以下设置同时适用于 `init`、`config`；TOML 键名用下划线而不是连字符。

| 参数 | 默认值 | 含义 |
| --- | --- | --- |
| `--cache-dir PATH` | 平台缓存目录下的 `cargoc/cache` | 专用受管理根目录；命令行路径转换为绝对路径。 |
| `--max-cache-gib N` | `80` | 受管理中间缓存的软预算，不是全部磁盘用量；必须为正数。 |
| `--low-water-ratio N` | `0.8` | 压力清理目标比例，严格介于 0 和 1；默认目标 **64 GiB**。 |
| `--min-free-gib N` | `20` | 缓存卷的期望剩余空间；启动受管理 Cargo 调用前也会检查。 |
| `--abort-free-gib N` | `1` | 紧急停止阈值，只停止本次 Cargo 进程树；`0` 禁用此停止功能。 |
| `--max-age-days N` | `7` | 按最后一次 cargoc 使用时间判断过期，**不是文件 mtime**；`0` 禁用按年龄清理。 |
| `--poll-seconds N` | `5` | 自动清理间隔，范围 `0.1`～`86400` 秒；紧急空间检查更频繁（每轮最多一次，默认约 250 毫秒）。 |
| `--auto-clean true\|false` | `true` | 在受管理命令开始前、运行中、结束后自动清理；关闭后仍保留启动空间检查和紧急停止。 |
| `--cargo PATH` | `cargo` | 真实 Cargo 可执行文件，不是 shell 命令；不能指回 cargoc。 |

默认缓存基目录：Windows 为 `%LOCALAPPDATA%`，Linux/macOS 为 `$XDG_CACHE_HOME`；未设置时回退到 `~/.cache`。数值必须有限且非负，并满足上表更严格的限制。`min_free_gib` 非零时，`abort_free_gib` 不得大于它。

## 所有权与清理安全边界

- 按解析后的工作区绝对路径计算 SHA256（Windows 上统一大小写和扩展路径前缀），缓存位于 `<cache-dir>/workspaces/<SHA256>`。根标记 `.cargoc-root.json` 和校验过的工作区记录识别受管理数据。
- 自动清理按最后使用时间从旧到新选择空闲缓存：过期或空间不足时清理；磁盘健康但超过预算时，预算作为软上限，只回收已过期缓存，避免项目之间反复删除和重新编译。磁盘空间不足时会突破年龄保护以恢复安全余量；当前工作区在编译前后都会受到保护。
- 工作区租约锁、清理器锁和 Cargo 原生锁保护活跃缓存。同一工作区的并发 cargoc 构建会被拒绝，不共享可写缓存；已锁定候选项会跳过。
- 手动 `clean` 即使在 `auto_clean = false` 时也处理所有空闲受管理缓存，但不清理未知内容。清理后可能保留部分锁文件和目录。
- 切换 `cache_dir` **不会**迁移或删除旧根目录。既有非 cargoc 缓存保持不变，也不计入预算。不要手动添加所有权标记接管旧缓存。
- 包装器仅为每次受管理调用提供 `build.build-dir`，不修改 Cargo 原有 target-dir 配置，也不移动现有产物。旧缓存应由其所有者另行处理。
- 紧急停止仅针对本次调用拥有的 Windows Job Object 或 Unix 进程组，不按进程名终止无关 Cargo/rustc。这是进程所有权边界，不是任意构建脚本的沙箱。

**这是软缓存预算，不是硬磁盘配额，也不保证磁盘绝不会写满。** 当前构建可能超预算，活跃缓存受保护，空间检查是周期性的。最终产物、注册表、其他缓存和无关程序用量不在预算内。缓存与最终产物位于不同卷时，检查缓存卷不能保护产物卷。启用的安全清理结束后，如果空间仍低于配置的最小/紧急阈值，将拒绝启动受管理构建。

只有实际通过 cargoc 启动的命令才受管理。仅安装 cargoc 不会改变普通 `cargo` 命令；启用 shim 并使其排在 PATH 前面后，直接执行 `cargo build`、`cargo test` 等命令就会经过 cargoc。IDE 或脚本若使用真实 Cargo 可执行文件的绝对路径，仍会绕过接管。

自动清理和 Cargo 退出事件写入 cargoc 配置旁的 `events.jsonl`，日志达到约 2 MiB 时轮换。清理或日志写入失败会警告，但不会覆盖 Cargo 的退出码，也不会强删不安全的候选项。空间统计使用逻辑字节数；硬链接等情况会使实际释放空间与逻辑删除量不同。

专用缓存根目录属于可丢弃存储，**不要在里面存源码、手写笔记或最终 `target-dir` 产物**。缓存路径中的符号链接和重解析点会被拒绝。本工具面向可信用户的日常维护，不提供抵御其他进程恶意替换文件系统条目的安全沙箱。

### 非托管旧缓存的处理

如果 Cargo 全局配置仍含 `build.build-dir = "D:/BuildCache/cargo/{workspace-path-hash}"`，绕过 cargoc 的调用仍会写入该目录；它与 `D:/BuildCache/cargoc` 是两套独立缓存。先修正调用入口，再盘点旧数据：

```sh
cargoc audit --path D:/BuildCache/cargo
cargoc status --json
cargoc clean --dry-run
```

`audit` 不需要初始化配置，也不触发旧配置迁移。它拒绝符号链接／重解析点、与受管理根目录重叠的路径和含 cargoc 所有权标记的目录。`busy` 表示观察到锁竞争，`unavailable` 表示无法判断；即使是 `available` 或没有锁，也**不代表可以删除**。文件可能在检查后继续写入，也可能含最终产物。

旧数据由其原所有者单独清理：停止相关 IDE、构建和后台任务，核对原 `build-dir`／`target-dir`、保留所需最终产物及恢复副本，再显式决定是否删除。不要把旧目录直接改名塞进 cargoc 根目录、补所有权标记，或把原生 Cargo 的全局 `build-dir` 指到 cargoc 的 `workspaces`；这些操作绕过租约，可能造成误删。`audit` 故意不提供自动删除选项，`cargoc clean` 也不会触碰旧目录。

## 预编译发布

推送到 `main` 后，`.github/workflows/release.yml` 自动发布新的项目版本，使用 GitHub 托管的原生 runner 构建以下平台：

| 平台 | 压缩包 |
| --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc`（`.zip`） |
| Windows ARM64 | `aarch64-pc-windows-msvc`（`.zip`） |
| Linux x64 | `x86_64-unknown-linux-gnu`（`.tar.gz`） |
| Linux ARM64 | `aarch64-unknown-linux-gnu`（`.tar.gz`） |
| macOS Intel | `x86_64-apple-darwin`（`.tar.gz`） |
| macOS Apple Silicon | `aarch64-apple-darwin`（`.tar.gz`） |

发行构建成功后，可在 [GitHub Releases](https://github.com/bigfish1913/cargoc/releases) 下载，例如 `cargoc-v0.1.3-x86_64-pc-windows-msvc.zip`。每个包包含 `cargoc` 可执行文件、双语 README 和 MIT 许可证，另附 `.sha256` 校验文件；Release 还提供汇总的 `SHA256SUMS`。

选择对应架构的压缩包，核对 SHA-256 后解压，将 **`cargoc` / `cargoc.exe`** 放入 PATH 中已有的目录即可，不要覆盖 Rustup 的 `cargo`。安装预编译程序不需要 Rust 编译器，但受管理构建仍需要 Cargo 1.91+。Linux 包使用 glibc，在 Ubuntu 22.04（glibc 2.35）上构建，不提供 Alpine/musl 版本。Windows/macOS 程序未进行 Authenticode/Developer ID 签名或公证。

维护者发布时，先将 `Cargo.toml` **及 `Cargo.lock`** 更新到新版本（例如 `0.1.3`），再把审核后的改动提交并推送到 `main`：

```sh
git add Cargo.toml Cargo.lock # 同时暂存审核后的源码／文档改动
git commit -m "Release 0.1.3"
git push origin main
```

稳定版还会附带生成的 `cargoc.rb`、`cargoc.json` 安装源清单。可选的源同步任务需要 `CHANNELS_TOKEN` Secret，授权范围仅需两个安装源仓库的 Contents 写权限；未配置时按渠道维护文档在本地同步，不要把令牌写入代码库。

不需要手动打标签。格式、Clippy、MSRV、六个平台的原生测试／构建、打包后二进制测试及校验和验证全部通过后，工作流自动在**实际测试的提交**上创建 `v{version}`，将资源上传至草稿，再公开发布。它不会自行递增项目版本，也不会回写 `main`。推送的版本已经发布时，跳过重复发布；下一次发行需要提升版本号。仍支持手动推送 `v*` 标签，但必须与项目版本完全一致。

`0.1.3-rc.1` 等版本会标记为预发布。不覆盖已有 Release、资源或指向其他提交的标签。上传失败可能留下草稿，重试前需检查并删除该草稿。由 `GITHUB_TOKEN` 创建的标签／Release 不会再次触发发行任务。此流程不向 crates.io 发布。

只想预编译验证时，在 **Actions → Release → Run workflow** 手动运行（`workflow_dispatch`），产物作为 Actions Artifacts 保留 14 天，不创建标签或公开 Release。GitHub 托管 runner 标签可能调整，届时需更新矩阵。

## 开发

CI 使用稳定版 Rust 及 `rustfmt`、`clippy`，在 Windows、Ubuntu、macOS 上执行：

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

工作流通过 `CARGO_TARGET_DIR` 和 `CARGO_BUILD_BUILD_DIR` 将最终产物、中间产物分别放入各任务本地临时目录，直接调用 Cargo 而非 cargoc，不修改全局配置。另有 Rust 1.88 任务执行全部目标的检查，验证声明的 MSRV。

## 贡献者

欢迎提交代码、问题反馈和文档改进。完整名单见 GitHub [贡献者列表](https://github.com/bigfish1913/cargoc/graphs/contributors)。

[![cargoc 贡献者](https://contrib.rocks/image?repo=bigfish1913/cargoc)](https://github.com/bigfish1913/cargoc/graphs/contributors)

贡献者头像由 [contrib.rocks](https://contrib.rocks) 动态生成。

## Star 趋势

[![cargoc Star 趋势图](https://api.star-history.com/svg?repos=bigfish1913/cargoc&type=Date)](https://www.star-history.com/#bigfish1913/cargoc&Date)

[查看交互式 Star 趋势](https://www.star-history.com/#bigfish1913/cargoc&Date) · [前往仓库 Star 列表](https://github.com/bigfish1913/cargoc/stargazers)

趋势图和头像依赖第三方服务，可能因 API 限流暂时不可用。图片无法加载时，仍可使用上面的文字链接。

## 许可证

[MIT](LICENSE)。Copyright 2026 cargoc contributors.
