# 在 Windows 上构建 Zed
> 以下命令可在任意终端中执行。

## 克隆仓库
克隆 [Zed 代码仓库](https://github.com/zed-industries/zed)。

## 安装依赖
- 安装 [rustup](https://www.rust-lang.org/tools/install)
- 安装以下任意一种环境：
  - **Visual Studio**：勾选可选组件 `MSVC v*** - VS YYYY C++ x64/x86 生成工具` 和 `MSVC v*** - VS YYYY C++ x64/x86 Spectre 缓解库（最新版）`（`v***` 为 VS 版本，`YYYY` 为发布年份，可根据需要调整架构）。
  - **轻量生成工具**：仅安装 [Visual Studio 生成工具](https://visualstudio.microsoft.com/visual-cpp-build-tools/) + 上述库文件 + `使用 C++ 的桌面开发` 工作负载。
    → 该环境不会被 rustup 自动识别，编译前需通过开始菜单/Windows Terminal 打开开发者命令行（cmd/PowerShell）初始化环境变量。
- 安装适配你系统的 **Windows 11/10 SDK**，确保至少安装 `Windows 10 SDK 2104 版 (10.0.20348.0)`，可从 [Windows SDK 归档页面](https://developer.microsoft.com/windows/downloads/windows-sdk/) 下载。
- 安装 [CMake](https://cmake.org/download)（[wasmtime 依赖](https://docs.rs/wasmtime-c-api-impl/latest/wasmtime_c_api/) 必需）；也可通过 Visual Studio 安装器安装，然后手动将 `bin` 目录添加到系统 `PATH`，示例：`C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin`。

---
### Visual Studio 必需组件（完整安装）
```json
{
  "version": "1.0",
  "components": [
    "Microsoft.VisualStudio.Component.CoreEditor",
    "Microsoft.VisualStudio.Workload.CoreEditor",
    "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
    "Microsoft.VisualStudio.ComponentGroup.WebToolsExtensions.CMake",
    "Microsoft.VisualStudio.Component.VC.CMake.Project",
    "Microsoft.VisualStudio.Component.Windows11SDK.26100",
    "Microsoft.VisualStudio.Component.VC.Runtimes.x86.x64.Spectre"
  ],
  "extensions": []
}
```

### 仅生成工具必需组件
```json
{
  "version": "1.0",
  "components": [
    "Microsoft.VisualStudio.Component.Roslyn.Compiler",
    "Microsoft.Component.MSBuild",
    "Microsoft.VisualStudio.Component.CoreBuildTools",
    "Microsoft.VisualStudio.Workload.MSBuildTools",
    "Microsoft.VisualStudio.Component.Windows10SDK",
    "Microsoft.VisualStudio.Component.VC.CoreBuildTools",
    "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
    "Microsoft.VisualStudio.Component.VC.Redist.14.Latest",
    "Microsoft.VisualStudio.Component.Windows11SDK.26100",
    "Microsoft.VisualStudio.Component.VC.CMake.Project",
    "Microsoft.VisualStudio.Component.TextTemplating",
    "Microsoft.VisualStudio.Component.VC.CoreIde",
    "Microsoft.VisualStudio.ComponentGroup.NativeDesktop.Core",
    "Microsoft.VisualStudio.Workload.VCTools",
    "Microsoft.VisualStudio.Component.VC.Runtimes.x86.x64.Spectre"
  ],
  "extensions": []
}
```

#### 导出组件配置
打开 Visual Studio 安装器 → 已安装标签页 → 更多 → 导出配置。

---

## 补充配置（PostgreSQL）
修改 `data` 目录下的 `pg_hba.conf`，将 `host` 认证方式从 `scram-sha-256` 改为 `trust`，否则会提示密码认证失败。
文件默认路径：`C:\Program Files\PostgreSQL\17\data\pg_hba.conf`
修改后示例：
```conf
# IPv4 local connections:
host    all             all             127.0.0.1/32            trust
# IPv6 local connections:
host    all             all             ::1/128                 trust
```

若 Windows 系统区域为非拉丁语言，修改 `postgresql.conf` 中的 `lc_messages` 为 `English_United States.1252`（或其他 UTF-8 兼容编码），避免数据库崩溃：
```conf
# lc_messages = 'Chinese (Simplified)_China.936'
lc_messages = 'English_United States.1252'
```

**重启 PostgreSQL 服务**：
按 `Win+R` → 输入 `services.msc` → 找到 `postgresql-x64-XX` → 右键重启。

## 从源码构建
依赖安装完成后，使用 [Cargo](https://doc.rust-lang.org/cargo/) 构建：

调试版构建：
```sh
cargo run
```

发行版构建：
```sh
cargo run --release
```

运行测试：
```sh
cargo test --workspace
```

> 注意：视觉回归测试目前仅支持 macOS，且需要屏幕录制权限，详情查看 [在 macOS 上构建 Zed](./macos.md#visual-regression-tests)。

## MSYS2 安装说明
Zed 不支持非官方的 Mingw-w64 版 MSYS2 包。
若使用 [mingw-w64-zed](https://packages.msys2.org/base/mingw-w64-zed) 出现问题，请反馈至 [msys2/MINGW-packages/issues](https://github.com/msys2/MINGW-packages/issues?q=is%3Aissue+is%3Aopen+zed)。
优先参考 [MSYS2 官方文档](https://www.msys2.org/docs/ides-editors/#zed)。

## 故障排查
### 1. 设置 `RUSTFLAGS` 环境变量导致构建失败
手动设置的 `RUSTFLAGS` 会覆盖 `.cargo/config.toml` 中的必需配置，导致构建失败。

**正确添加自定义编译参数**：
方式 1：在构建配置中添加
```toml
[build]
rustflags = ["-C", "symbol-mangling-version=v0", "--cfg", "tokio_unstable"]
```

方式 2：Windows 目标专属配置
```toml
[target.'cfg(target_os = "windows")']
rustflags = [
    "--cfg",
    "windows_slim_errors",
    "-C",
    "target-feature=+crt-static",
]
```

方式 3：父目录配置（不修改仓库原文件，适合 CI）
```
上级目录
├── .cargo          // 新建此文件夹
│   └── config.toml // 新建配置文件
└── zed             // Zed 仓库
```

配置示例：
```toml
[target.'cfg(all())']
rustflags = ["--cfg", "gles"]
```

### 2. Cargo 提示依赖使用了不稳定特性
执行清理并重新构建：
```sh
cargo clean
cargo build
```

### 3. `STATUS_ACCESS_VIOLATION` 错误
通常由 `rust-lld.exe` 链接器导致，尝试更换链接器；
全局配置时可将仓库移至嵌套目录，在父目录 `.cargo/config.toml` 自定义链接器。
详情查看 [#12041](https://github.com/zed-industries/zed/issues/12041)。

### 4. 无效的 RC 路径
编译报错 `系统找不到指定路径` 时，手动设置环境变量：
`ZED_RC_TOOLKIT_PATH`
通常路径：`C:\Program Files (x86)\Windows Kits\10\bin\<SDK版本>\x64`
详情查看 [issue](https://github.com/zed-industries/zed/issues/18393)。

### 5. 构建失败：路径过长
开启 Windows 和 Git 的长路径支持：

Git 配置：
```sh
git config --system core.longpaths true
```

PowerShell 执行（管理员）：
```powershell
New-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem" -Name "LongPathsEnabled" -Value 1 -PropertyType DWORD -Force
```
→ 修改后**重启电脑**生效。

### 6. 图形相关问题
Zed 在 Windows 上使用 **Vulkan** 图形接口，启动失败大概率是 Vulkan 问题。

查看日志：`C:\Users\你的用户名\AppData\Local\Zed\logs\Zed.log`

常见错误：
- `Zed failed to open a window: NoSupportedDeviceFound`
- `ERROR_INITIALIZATION_FAILED`
- `GPU Crashed`

**解决方案**：更新显卡驱动。
若日志无 Vulkan 错误且安装了 Bandicam，卸载该软件（当前不兼容）。
