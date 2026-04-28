#![allow(
    clippy::disallowed_methods,
    reason = "我们不在异步环境中，因此 std::process::Command 完全可用"
)]
#![cfg_attr(
    any(target_os = "linux", target_os = "freebsd", target_os = "windows"),
    allow(dead_code)
)]

use anyhow::{Context as _, Result};
use clap::Parser;
use cli::{CliRequest, CliResponse, IpcHandshake, ipc::IpcOneShotServer};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::Arc,
    thread::{self, JoinHandle},
};
use tempfile::{NamedTempFile, TempDir};
use util::paths::PathWithPosition;
use walkdir::WalkDir;

use std::io::IsTerminal;

const URL_PREFIX: [&'static str; 5] = ["zed://", "http://", "https://", "file://", "ssh://"];

struct Detect;

trait InstalledApp {
    fn zed_version_string(&self) -> String;
    fn launch(&self, ipc_url: String, user_data_dir: Option<&str>) -> anyhow::Result<()>;
    fn run_foreground(
        &self,
        ipc_url: String,
        user_data_dir: Option<&str>,
    ) -> io::Result<ExitStatus>;
    fn path(&self) -> PathBuf;
}

#[derive(Parser, Debug)]
#[command(
    name = "zed",
    disable_version_flag = true,
    before_help = "Zed 命令行工具。
此 CLI 是一个独立二进制文件，用于调用 Zed 主程序。

使用示例：
    `zed`
          直接打开 Zed
    `zed --foreground`
          前台运行（显示所有日志）
    `zed 项目路径`
          在 Zed 中打开项目
    `zed -n 文件路径`
          在新窗口中打开文件/文件夹",
    after_help = "要从标准输入读取内容，追加 '-'，例如：ps axf | zed -"
)]
struct Args {
    /// 等待所有指定路径被打开/关闭后再退出
    ///
    /// 打开目录时，等待创建的窗口被关闭
    #[arg(short, long)]
    wait: bool,

    /// 将文件添加到当前打开的工作区
    #[arg(short, long, overrides_with_all = ["new", "reuse", "existing", "classic"])]
    add: bool,

    /// 创建新的工作区
    #[arg(short, long, overrides_with_all = ["add", "reuse", "existing", "classic"])]
    new: bool,

    /// 复用现有窗口，替换其工作区（隐藏选项）
    #[arg(short, long, overrides_with_all = ["add", "new", "existing", "classic"], hide = true)]
    reuse: bool,

    /// 在现有 Zed 窗口中打开
    #[arg(short = 'e', long = "existing", overrides_with_all = ["add", "new", "reuse", "classic"])]
    existing: bool,

    /// 使用经典打开行为：目录新建窗口，文件复用窗口（隐藏选项）
    #[arg(long, hide = true, overrides_with_all = ["add", "new", "reuse", "existing"])]
    classic: bool,

    /// 为所有用户数据设置自定义目录（如数据库、扩展、日志）
    /// 此选项会覆盖默认的平台专属数据目录位置：
    #[cfg_attr(target_os = "macos", doc = "`~/Library/Application Support/Zed`。")]
    #[cfg_attr(target_os = "windows", doc = "`%LOCALAPPDATA%\\Zed`。")]
    #[cfg_attr(
        not(any(target_os = "windows", "macos")),
        doc = "`$XDG_DATA_HOME/zed`。"
    )]
    #[arg(long, value_name = "目录")]
    user_data_dir: Option<String>,

    /// 要在 Zed 中打开的路径（空格分隔）
    ///
    /// 使用 `路径:行号:列号` 语法在指定位置打开文件
    paths_with_position: Vec<String>,

    /// 打印 Zed 版本号和应用路径
    #[arg(short, long)]
    version: bool,

    /// 前台运行 zed（适用于调试）
    #[arg(long)]
    foreground: bool,

    /// 自定义 Zed.app 或 zed 二进制文件路径
    #[arg(long)]
    zed: Option<PathBuf>,

    /// 以开发服务器模式运行 zed（已废弃）
    #[arg(long)]
    dev_server_token: Option<String>,

    /// 打开路径时使用的用户名和 WSL 发行版。如未指定，
    /// Zed 将尝试直接打开路径。
    ///
    /// 用户名为可选项，未指定时将使用发行版的默认用户。
    ///
    /// 示例：`me@Ubuntu` 或 `Ubuntu`。
    ///
    /// 警告：请勿手动填写此参数。
    #[cfg(target_os = "windows")]
    #[arg(long, value_name = "用户@发行版")]
    wsl: Option<String>,

    /// Zed CLI 不支持，仅主程序支持
    /// 会尝试给出正确的运行命令
    #[arg(long)]
    system_specs: bool,

    /// 在开发容器中打开项目
    ///
    /// 如果在项目目录中找到 `.devcontainer/` 配置，
    /// 自动触发“在开发容器中重新打开”
    #[arg(long)]
    dev_container: bool,

    /// 要对比的文件路径对。可多次指定。
    /// 提供目录时，会递归遍历并在单个多文件对比视图中显示所有变更
    #[arg(long, action = clap::ArgAction::Append, num_args = 2, value_names = ["旧路径", "新路径"])]
    diff: Vec<String>,

    /// 从用户系统中卸载 Zed
    #[cfg(all(
        any(target_os = "linux", target_os = "macos"),
        not(feature = "no-bundled-uninstall")
    ))]
    #[arg(long)]
    uninstall: bool,

    /// 用于 SSH/Git 密码验证，无需依赖 netcat
    /// 让 Zed 像 netcat 一样通过 Unix 套接字通信
    #[arg(long, hide = true)]
    askpass: Option<String>,
}

/// 解析包含位置信息的路径（例如 `路径:行号:列号`）
/// 并返回其规范化字符串表示
///
/// 如果路径的某部分不存在，会规范化已存在部分并追加未存在部分
///
/// 此方法必须返回绝对路径，因为许多 Zed 库都假定使用绝对路径
fn parse_path_with_position(argument_str: &str) -> anyhow::Result<String> {
    match Path::new(argument_str).canonicalize() {
        Ok(existing_path) => Ok(PathWithPosition::from_path(existing_path)),
        Err(_) => PathWithPosition::parse_str(argument_str).map_path(|mut path| {
            let curdir = env::current_dir().context("获取当前目录失败")?;
            let mut children = Vec::new();
            let root;
            loop {
                // canonicalize 处理 ./ 和 /
                if let Ok(canonicalized) = fs::canonicalize(&path) {
                    root = canonicalized;
                    break;
                }
                // 与 curdir 比较只是快捷方式，因为我们知道它是规范化的
                // 另一种情况是 argument_str 以名称开头（例如 "foo/bar"）
                if path == curdir || path == Path::new("") {
                    root = curdir;
                    break;
                }
                children.push(
                    path.file_name()
                        .with_context(|| format!("解析带位置的路径失败 {argument_str}"))?
                        .to_owned(),
                );
                if !path.pop() {
                    unreachable!("解析带位置的路径失败 {argument_str}");
                }
            }
            Ok(children.iter().rev().fold(root, |mut path, child| {
                path.push(child);
                path
            }))
        }),
    }
    .map(|path_with_pos| path_with_pos.to_string(&|path| path.to_string_lossy().into_owned()))
}

fn expand_directory_diff_pairs(
    diff_pairs: Vec<[String; 2]>,
) -> anyhow::Result<(Vec<[String; 2]>, Vec<TempDir>)> {
    let mut expanded = Vec::new();
    let mut temp_dirs = Vec::new();

    for pair in diff_pairs {
        let left = PathBuf::from(&pair[0]);
        let right = PathBuf::from(&pair[1]);

        if left.is_dir() && right.is_dir() {
            let (mut pairs, temp_dir) = expand_directory_pair(&left, &right)?;
            expanded.append(&mut pairs);
            if let Some(temp_dir) = temp_dir {
                temp_dirs.push(temp_dir);
            }
        } else {
            expanded.push(pair);
        }
    }

    Ok((expanded, temp_dirs))
}

fn expand_directory_pair(
    left: &Path,
    right: &Path,
) -> anyhow::Result<(Vec<[String; 2]>, Option<TempDir>)> {
    let left_files = collect_files(left)?;
    let right_files = collect_files(right)?;

    let mut rel_paths = BTreeSet::new();
    rel_paths.extend(left_files.keys().cloned());
    rel_paths.extend(right_files.keys().cloned());

    let mut temp_dir = TempDir::new()?;
    let mut temp_dir_used = false;
    let mut pairs = Vec::new();

    for rel in rel_paths {
        match (left_files.get(&rel), right_files.get(&rel)) {
            (Some(left_path), Some(right_path)) => {
                pairs.push([
                    left_path.to_string_lossy().into_owned(),
                    right_path.to_string_lossy().into_owned(),
                ]);
            }
            (Some(left_path), None) => {
                let stub = create_empty_stub(&mut temp_dir, &rel)?;
                temp_dir_used = true;
                pairs.push([
                    left_path.to_string_lossy().into_owned(),
                    stub.to_string_lossy().into_owned(),
                ]);
            }
            (None, Some(right_path)) => {
                let stub = create_empty_stub(&mut temp_dir, &rel)?;
                temp_dir_used = true;
                pairs.push([
                    stub.to_string_lossy().into_owned(),
                    right_path.to_string_lossy().into_owned(),
                ]);
            }
            (None, None) => {}
        }
    }

    let temp_dir = if temp_dir_used { Some(temp_dir) } else { None };
    Ok((pairs, temp_dir))
}

fn collect_files(root: &Path) -> anyhow::Result<BTreeMap<PathBuf, PathBuf>> {
    let mut files = BTreeMap::new();

    for entry in WalkDir::new(root) {
        let entry = entry?;
        if entry.file_type().is_file() {
            let rel = entry
                .path()
                .strip_prefix(root)
                .context("剥离目录前缀失败")?
                .to_path_buf();
            files.insert(rel, entry.into_path());
        }
    }

    Ok(files)
}

fn create_empty_stub(temp_dir: &mut TempDir, rel: &Path) -> anyhow::Result<PathBuf> {
    let stub_path = temp_dir.path().join(rel);
    if let Some(parent) = stub_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::File::create(&stub_path)?;
    Ok(stub_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use util::path;
    use util::paths::SanitizedPath;
    use util::test::TempTree;

    macro_rules! assert_path_eq {
        ($left:expr, $right:expr) => {
            assert_eq!(
                SanitizedPath::new(Path::new(&$left)),
                SanitizedPath::new(Path::new(&$right))
            )
        };
    }

    fn cwd() -> PathBuf {
        env::current_dir().unwrap()
    }

    static CWD_LOCK: Mutex<()> = Mutex::new(());

    fn with_cwd<T>(path: &Path, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
        let _lock = CWD_LOCK.lock();
        let old_cwd = cwd();
        env::set_current_dir(path)?;
        let result = f();
        env::set_current_dir(old_cwd)?;
        result
    }

    #[test]
    fn test_parse_non_existing_path() {
        // 绝对路径
        let result = parse_path_with_position(path!("/non/existing/path.txt")).unwrap();
        assert_path_eq!(result, path!("/non/existing/path.txt"));

        // 当前目录中的绝对路径
        let path = cwd().join(path!("non/existing/path.txt"));
        let expected = path.to_string_lossy().to_string();
        let result = parse_path_with_position(&expected).unwrap();
        assert_path_eq!(result, expected);

        // 相对路径
        let result = parse_path_with_position(path!("non/existing/path.txt")).unwrap();
        assert_path_eq!(result, expected)
    }

    #[test]
    fn test_parse_existing_path() {
        let temp_tree = TempTree::new(json!({
            "file.txt": "",
        }));
        let file_path = temp_tree.path().join("file.txt");
        let expected = file_path.to_string_lossy().to_string();

        // 绝对路径
        let result = parse_path_with_position(file_path.to_str().unwrap()).unwrap();
        assert_path_eq!(result, expected);

        // 相对路径
        let result = with_cwd(temp_tree.path(), || parse_path_with_position("file.txt")).unwrap();
        assert_path_eq!(result, expected);
    }

    // 注意：
    // 虽然 Windows 在一定程度上支持 POSIX 符号链接，但需要用户手动启用，
    // 因此我们假定默认不支持。
    #[cfg(not(windows))]
    #[test]
    fn test_parse_symlink_file() {
        let temp_tree = TempTree::new(json!({
            "target.txt": "",
        }));
        let target_path = temp_tree.path().join("target.txt");
        let symlink_path = temp_tree.path().join("symlink.txt");
        std::os::unix::fs::symlink(&target_path, &symlink_path).unwrap();

        // 绝对路径
        let result = parse_path_with_position(symlink_path.to_str().unwrap()).unwrap();
        assert_eq!(result, target_path.to_string_lossy());

        // 相对路径
        let result =
            with_cwd(temp_tree.path(), || parse_path_with_position("symlink.txt")).unwrap();
        assert_eq!(result, target_path.to_string_lossy());
    }

    #[cfg(not(windows))]
    #[test]
    fn test_parse_symlink_dir() {
        let temp_tree = TempTree::new(json!({
            "some": {
                "dir": { // 符号链接目标
                    "ec": {
                        "tory": {
                            "file.txt": "",
        }}}}}));

        let target_file_path = temp_tree.path().join("some/dir/ec/tory/file.txt");
        let expected = target_file_path.to_string_lossy();

        let dir_path = temp_tree.path().join("some/dir");
        let symlink_path = temp_tree.path().join("symlink");
        std::os::unix::fs::symlink(&dir_path, &symlink_path).unwrap();

        // 绝对路径
        let result =
            parse_path_with_position(symlink_path.join("ec/tory/file.txt").to_str().unwrap())
                .unwrap();
        assert_eq!(result, expected);

        // 相对路径
        let result = with_cwd(temp_tree.path(), || {
            parse_path_with_position("symlink/ec/tory/file.txt")
        })
        .unwrap();
        assert_eq!(result, expected);
    }
}

fn parse_path_in_wsl(source: &str, wsl: &str) -> Result<String> {
    let mut source = PathWithPosition::parse_str(source);

    let (user, distro_name) = if let Some((user, distro)) = wsl.split_once('@') {
        if user.is_empty() {
            anyhow::bail!("wsl 参数中的用户名为空");
        }
        (Some(user), distro)
    } else {
        (None, wsl)
    };

    let mut args = vec!["--distribution", distro_name];
    if let Some(user) = user {
        args.push("--user");
        args.push(user);
    }

    let command = [
        OsStr::new("realpath"),
        OsStr::new("-s"),
        source.path.as_ref(),
    ];

    let output = util::command::new_std_command("wsl.exe")
        .args(&args)
        .arg("--exec")
        .args(&command)
        .output()?;
    let result = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        let fallback = util::command::new_std_command("wsl.exe")
            .args(&args)
            .arg("--")
            .args(&command)
            .output()?;
        String::from_utf8_lossy(&fallback.stdout).to_string()
    };

    source.path = Path::new(result.trim()).to_owned();

    Ok(source.to_string(&|path| path.to_string_lossy().into_owned()))
}

fn main() -> Result<()> {
    #[cfg(unix)]
    util::prevent_root_execution();

    // 如需退出 flatpak 沙箱
    #[cfg(target_os = "linux")]
    {
        flatpak::try_restart_to_host();
        flatpak::ld_extra_libs();
    }

    // 拦截版本通道参数
    #[cfg(target_os = "macos")]
    if let Some(channel) = std::env::args().nth(1).filter(|arg| arg.starts_with("--")) {
        // 当第一个参数是发布通道名称时，我们将生成该版本的 CLI，并传递后续参数
        use std::str::FromStr as _;

        if let Ok(channel) = release_channel::ReleaseChannel::from_str(&channel[2..]) {
            return mac_os::spawn_channel_cli(channel, std::env::args().skip(2).collect());
        }
    }
    let args = Args::parse();

    // `zed --askpass` 让 Zed 以类似 nc/netcat 模式运行，用于密码验证
    if let Some(socket) = &args.askpass {
        askpass::main(socket);
        return Ok(());
    }

    // 在任何路径操作前设置自定义数据目录
    let user_data_dir = args.user_data_dir.clone();
    if let Some(dir) = &user_data_dir {
        paths::set_custom_data_dir(dir);
    }

    #[cfg(target_os = "linux")]
    let args = flatpak::set_bin_if_no_escape(args);

    let app = Detect::detect(args.zed.as_deref()).context("应用包检测失败")?;

    if args.version {
        println!("{}", app.zed_version_string());
        return Ok(());
    }

    if args.system_specs {
        let path = app.path();
        let msg = [
            "`--system-specs` 参数仅支持 Zed 主程序，不支持 CLI 工具。",
            "要在命令行获取系统信息，请运行以下命令：",
            &format!("{} --system-specs", path.display()),
        ];
        anyhow::bail!(msg.join("\n"));
    }

    #[cfg(all(
        any(target_os = "linux", target_os = "macos"),
        not(feature = "no-bundled-uninstall")
    ))]
    if args.uninstall {
        static UNINSTALL_SCRIPT: &[u8] = include_bytes!("../../../script/uninstall.sh");

        let tmp_dir = tempfile::tempdir()?;
        let script_path = tmp_dir.path().join("uninstall.sh");
        fs::write(&script_path, UNINSTALL_SCRIPT)?;

        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))?;

        let status = std::process::Command::new("sh")
            .arg(&script_path)
            .env("ZED_CHANNEL", &*release_channel::RELEASE_CHANNEL_NAME)
            .status()
            .context("执行卸载脚本失败")?;

        std::process::exit(status.code().unwrap_or(1));
    }

    let (server, server_name) =
        IpcOneShotServer::<IpcHandshake>::new().context("Zed 启动前握手失败")?;
    let url = format!("zed-cli://{server_name}");

    let open_behavior = if args.new {
        cli::OpenBehavior::AlwaysNew
    } else if args.add {
        cli::OpenBehavior::Add
    } else if args.existing {
        cli::OpenBehavior::ExistingWindow
    } else if args.classic {
        cli::OpenBehavior::Classic
    } else if args.reuse {
        cli::OpenBehavior::Reuse
    } else {
        cli::OpenBehavior::Default
    };

    let env = {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            use collections::HashMap;

            // 在 Linux 上，桌面项使用 cli 启动 zed
            // 我们需要正确处理环境变量，因为 std::env::vars() 可能不包含
            // 项目专属变量（例如 direnv 设置的变量）
            // 在此处将 env 设置为 None，LSP 将使用工作树环境变量，这正是我们需要的
            if !std::io::stdout().is_terminal() {
                None
            } else {
                Some(std::env::vars().collect::<HashMap<_, _>>())
            }
        }

        #[cfg(target_os = "windows")]
        {
            // 在 Windows 上，默认情况下子进程会继承父进程的环境块
            // 因此我们无需显式传递环境变量
            None
        }

        #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "windows")))]
        {
            use collections::HashMap;

            Some(std::env::vars().collect::<HashMap<_, _>>())
        }
    };

    let exit_status = Arc::new(Mutex::new(None));
    let mut paths = vec![];
    let mut urls = vec![];
    let mut diff_paths = vec![];
    let mut stdin_tmp_file: Option<fs::File> = None;
    let mut anonymous_fd_tmp_files = vec![];

    // 检查是否有对比路径是目录，以决定是否启用全部对比模式
    let diff_all_mode = args
        .diff
        .chunks(2)
        .any(|pair| Path::new(&pair[0]).is_dir() || Path::new(&pair[1]).is_dir());

    for path in args.diff.chunks(2) {
        diff_paths.push([
            parse_path_with_position(&path[0])?,
            parse_path_with_position(&path[1])?,
        ]);
    }

    let (expanded_diff_paths, temp_dirs) = expand_directory_diff_pairs(diff_paths)?;
    diff_paths = expanded_diff_paths;
    // 阻止自动清理目录对比所需的临时空文件目录
    // CLI 进程可能在 Zed 读取这些文件前就退出（例如调用已运行的实例）
    // 这些文件位于系统临时目录，会在重启后清理
    for temp_dir in temp_dirs {
        let _ = temp_dir.keep();
    }

    #[cfg(target_os = "windows")]
    let wsl = args.wsl.as_ref();
    #[cfg(not(target_os = "windows"))]
    let wsl = None;

    for path in args.paths_with_position.iter() {
        if URL_PREFIX.iter().any(|&prefix| path.starts_with(prefix)) {
            urls.push(path.to_string());
        } else if path == "-" && args.paths_with_position.len() == 1 {
            let file = NamedTempFile::new()?;
            paths.push(file.path().to_string_lossy().into_owned());
            let (file, _) = file.keep()?;
            stdin_tmp_file = Some(file);
        } else if let Some(file) = anonymous_fd(path) {
            let tmp_file = NamedTempFile::new()?;
            paths.push(tmp_file.path().to_string_lossy().into_owned());
            let (tmp_file, _) = tmp_file.keep()?;
            anonymous_fd_tmp_files.push((file, tmp_file));
        } else if let Some(wsl) = wsl {
            urls.push(format!("file://{}", parse_path_in_wsl(path, wsl)?));
        } else {
            paths.push(parse_path_with_position(path)?);
        }
    }

    anyhow::ensure!(
        args.dev_server_token.is_none(),
        "开发服务器已在 v0.157.x 版本移除，请升级到 SSH 远程开发：https://zed.dev/docs/remote-development"
    );

    rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .stack_size(10 * 1024 * 1024)
        .thread_name(|ix| format!("线程池工作线程{}", ix))
        .build_global()
        .unwrap();

    let sender: JoinHandle<anyhow::Result<()>> = thread::Builder::new()
        .name("CLI接收线程".to_string())
        .spawn({
            let exit_status = exit_status.clone();
            let user_data_dir_for_thread = user_data_dir.clone();
            move || {
                let (_, handshake) = server.accept().context("Zed 启动后握手失败")?;
                let (tx, rx) = (handshake.requests, handshake.responses);

                #[cfg(target_os = "windows")]
                let wsl = args.wsl;
                #[cfg(not(target_os = "windows"))]
                let wsl = None;

                let open_request = CliRequest::Open {
                    paths,
                    urls,
                    diff_paths,
                    diff_all: diff_all_mode,
                    wsl,
                    wait: args.wait,
                    open_behavior,
                    env,
                    user_data_dir: user_data_dir_for_thread,
                    dev_container: args.dev_container,
                };

                tx.send(open_request)?;

                while let Ok(response) = rx.recv() {
                    match response {
                        CliResponse::Ping => {}
                        CliResponse::Stdout { message } => println!("{message}"),
                        CliResponse::Stderr { message } => eprintln!("{message}"),
                        CliResponse::Exit { status } => {
                            exit_status.lock().replace(status);
                            return Ok(());
                        }
                        CliResponse::PromptOpenBehavior => {
                            let behavior = prompt_open_behavior()
                                .unwrap_or(cli::CliBehaviorSetting::ExistingWindow);
                            tx.send(CliRequest::SetOpenBehavior { behavior })?;
                        }
                    }
                }

                Ok(())
            }
        })
        .unwrap();

    let stdin_pipe_handle: Option<JoinHandle<anyhow::Result<()>>> =
        stdin_tmp_file.map(|mut tmp_file| {
            thread::Builder::new()
                .name("CLI标准输入线程".to_string())
                .spawn(move || {
                    let mut stdin = std::io::stdin().lock();
                    if !io::IsTerminal::is_terminal(&stdin) {
                        io::copy(&mut stdin, &mut tmp_file)?;
                    }
                    Ok(())
                })
                .unwrap()
        });

    let anonymous_fd_pipe_handles: Vec<_> = anonymous_fd_tmp_files
        .into_iter()
        .map(|(mut file, mut tmp_file)| {
            thread::Builder::new()
                .name("CLI匿名文件描述符线程".to_string())
                .spawn(move || io::copy(&mut file, &mut tmp_file))
                .unwrap()
        })
        .collect();

    if args.foreground {
        app.run_foreground(url, user_data_dir.as_deref())?;
    } else {
        app.launch(url, user_data_dir.as_deref())?;
        sender.join().unwrap()?;
        if let Some(handle) = stdin_pipe_handle {
            handle.join().unwrap()?;
        }
        for handle in anonymous_fd_pipe_handles {
            handle.join().unwrap()?;
        }
    }

    if let Some(exit_status) = exit_status.lock().take() {
        std::process::exit(exit_status);
    }
    Ok(())
}

fn anonymous_fd(path: &str) -> Option<fs::File> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::{self, FromRawFd};

        let fd_str = path.strip_prefix("/proc/self/fd/")?;

        let link = fs::read_link(path).ok()?;
        if !link.starts_with("memfd:") {
            return None;
        }

        let fd: fd::RawFd = fd_str.parse().ok()?;
        let file = unsafe { fs::File::from_raw_fd(fd) };
        Some(file)
    }
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    {
        use std::os::{
            fd::{self, FromRawFd},
            unix::fs::FileTypeExt,
        };

        let fd_str = path.strip_prefix("/dev/fd/")?;

        let metadata = fs::metadata(path).ok()?;
        let file_type = metadata.file_type();
        if !file_type.is_fifo() && !file_type.is_socket() {
            return None;
        }
        let fd: fd::RawFd = fd_str.parse().ok()?;
        let file = unsafe { fs::File::from_raw_fd(fd) };
        Some(file)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))]
    {
        _ = path;
        // 暂未实现 BSD、Windows 支持
        None
    }
}

/// 显示交互式提示，让用户选择 `zed <路径>` 的默认打开行为
/// 如果无法显示提示（例如标准输入不是终端）或用户取消，返回 None
fn prompt_open_behavior() -> Option<cli::CliBehaviorSetting> {
    if !std::io::stdin().is_terminal() {
        return None;
    }

    let blue = console::Style::new().blue();
    let items = [
        format!(
            "添加到现有 Zed 窗口（{}）",
            blue.apply_to("zed --existing")
        ),
        format!("打开新窗口（{}）", blue.apply_to("zed --classic")),
    ];

    let prompt = format!(
        "配置 {} 的默认行为\n{}",
        blue.apply_to("zed <路径>"),
        console::style("你可以稍后在 Zed 设置中修改"),
    );

    let selection = dialoguer::Select::new()
        .with_prompt(&prompt)
        .items(&items)
        .default(0)
        .interact()
        .ok()?;

    Some(if selection == 0 {
        cli::CliBehaviorSetting::ExistingWindow
    } else {
        cli::CliBehaviorSetting::NewWindow
    })
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux {
    use std::{
        env,
        ffi::OsString,
        io,
        os::unix::net::{SocketAddr, UnixDatagram},
        path::{Path, PathBuf},
        process::{self, ExitStatus},
        thread,
        time::Duration,
    };

    use anyhow::{Context as _, anyhow};
    use cli::FORCE_CLI_MODE_ENV_VAR_NAME;
    use fork::Fork;

    use crate::{Detect, InstalledApp};

    struct App(PathBuf);

    impl Detect {
        pub fn detect(path: Option<&Path>) -> anyhow::Result<impl InstalledApp> {
            let path = if let Some(path) = path {
                path.to_path_buf().canonicalize()?
            } else {
                let cli = env::current_exe()?;
                let dir = cli.parent().context("CLI 无父路径")?;

                // libexec 是标准路径，lib/zed 适用于 Arch（及其他非 libexec 发行版）
                // ./zed 适用于开发构建的 target 目录
                let possible_locations =
                    ["../libexec/zed-editor", "../lib/zed/zed-editor", "./zed"];
                possible_locations
                    .iter()
                    .find_map(|p| dir.join(p).canonicalize().ok().filter(|path| path != &cli))
                    .with_context(|| {
                        format!("找不到以下任一文件：{}", possible_locations.join(", "))
                    })?
            };

            Ok(App(path))
        }
    }

    impl InstalledApp for App {
        fn zed_version_string(&self) -> String {
            format!(
                "Zed {}{}{} – {}",
                if *release_channel::RELEASE_CHANNEL_NAME == "stable" {
                    "".to_string()
                } else {
                    format!("{} ", *release_channel::RELEASE_CHANNEL_NAME)
                },
                option_env!("RELEASE_VERSION").unwrap_or_default(),
                match option_env!("ZED_COMMIT_SHA") {
                    Some(commit_sha) => format!(" {commit_sha} "),
                    None => "".to_string(),
                },
                self.0.display(),
            )
        }

        fn launch(&self, ipc_url: String, user_data_dir: Option<&str>) -> anyhow::Result<()> {
            let data_dir = user_data_dir
                .map(PathBuf::from)
                .unwrap_or_else(|| paths::data_dir().clone());

            let sock_path = data_dir.join(format!(
                "zed-{}.sock",
                *release_channel::RELEASE_CHANNEL_NAME
            ));
            let sock = UnixDatagram::unbound()?;
            if sock.connect(&sock_path).is_err() {
                self.boot_background(ipc_url, user_data_dir)?;
            } else {
                sock.send(ipc_url.as_bytes())?;
            }
            Ok(())
        }

        fn run_foreground(
            &self,
            ipc_url: String,
            user_data_dir: Option<&str>,
        ) -> io::Result<ExitStatus> {
            let mut cmd = std::process::Command::new(self.0.clone());
            cmd.arg(ipc_url);
            if let Some(dir) = user_data_dir {
                cmd.arg("--user-data-dir").arg(dir);
            }
            cmd.status()
        }

        fn path(&self) -> PathBuf {
            self.0.clone()
        }
    }

    impl App {
        fn boot_background(
            &self,
            ipc_url: String,
            user_data_dir: Option<&str>,
        ) -> anyhow::Result<()> {
            let path = &self.0;

            match fork::fork() {
                Ok(Fork::Parent(_)) => Ok(()),
                Ok(Fork::Child) => {
                    unsafe { std::env::set_var(FORCE_CLI_MODE_ENV_VAR_NAME, "") };
                    if fork::setsid().is_err() {
                        eprintln!("setsid 失败：{}", std::io::Error::last_os_error());
                        process::exit(1);
                    }
                    if fork::close_fd().is_err() {
                        eprintln!("关闭文件描述符失败：{}", std::io::Error::last_os_error());
                    }
                    let mut args: Vec<OsString> =
                        vec![path.as_os_str().to_owned(), OsString::from(ipc_url)];
                    if let Some(dir) = user_data_dir {
                        args.push(OsString::from("--user-data-dir"));
                        args.push(OsString::from(dir));
                    }
                    let error = exec::execvp(path.clone(), &args);
                    // 如果 exec 成功，代码不会执行到这里
                    eprintln!("执行失败 {:?}：{}", path, error);
                    process::exit(1)
                }
                Err(_) => Err(anyhow!(io::Error::last_os_error())),
            }
        }

        fn wait_for_socket(
            &self,
            sock_addr: &SocketAddr,
            sock: &mut UnixDatagram,
        ) -> Result<(), std::io::Error> {
            for _ in 0..100 {
                thread::sleep(Duration::from_millis(10));
                if sock.connect_addr(sock_addr).is_ok() {
                    return Ok(());
                }
            }
            sock.connect_addr(sock_addr)
        }
    }
}

#[cfg(target_os = "linux")]
mod flatpak {
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::process::Command;
    use std::{env, process};

    const EXTRA_LIB_ENV_NAME: &str = "ZED_FLATPAK_LIB_PATH";
    const NO_ESCAPE_ENV_NAME: &str = "ZED_FLATPAK_NO_ESCAPE";

    /// 如果运行在 flatpak 中，将捆绑库添加到 LD_LIBRARY_PATH
    pub fn ld_extra_libs() {
        let mut paths = if let Ok(paths) = env::var("LD_LIBRARY_PATH") {
            env::split_paths(&paths).collect()
        } else {
            Vec::new()
        };

        if let Ok(extra_path) = env::var(EXTRA_LIB_ENV_NAME) {
            paths.push(extra_path.into());
        }

        unsafe { env::set_var("LD_LIBRARY_PATH", env::join_paths(paths).unwrap()) };
    }

    /// 如果当前在沙箱中，重启到宿主系统
    pub fn try_restart_to_host() {
        if let Some(flatpak_dir) = get_flatpak_dir() {
            let mut args = vec!["/usr/bin/flatpak-spawn".into(), "--host".into()];
            args.append(&mut get_xdg_env_args());
            args.push("--env=ZED_UPDATE_EXPLANATION=请使用 flatpak 更新 zed".into());
            args.push(
                format!(
                    "--env={EXTRA_LIB_ENV_NAME}={}",
                    flatpak_dir.join("lib").to_str().unwrap()
                )
                .into(),
            );
            args.push(flatpak_dir.join("bin").join("zed").into());

            let mut is_app_location_set = false;
            for arg in &env::args_os().collect::<Vec<_>>()[1..] {
                args.push(arg.clone());
                is_app_location_set |= arg == "--zed";
            }

            if !is_app_location_set {
                args.push("--zed".into());
                args.push(flatpak_dir.join("libexec").join("zed-editor").into());
            }

            let error = exec::execvp("/usr/bin/flatpak-spawn", args);
            eprintln!("在宿主系统重启 CLI 失败：{:?}", error);
            process::exit(1);
        }
    }

    pub fn set_bin_if_no_escape(mut args: super::Args) -> super::Args {
        if env::var(NO_ESCAPE_ENV_NAME).is_ok()
            && env::var("FLATPAK_ID").is_ok_and(|id| id.starts_with("dev.zed.Zed"))
            && args.zed.is_none()
        {
            args.zed = Some("/app/libexec/zed-editor".into());
            unsafe { env::set_var("ZED_UPDATE_EXPLANATION", "请使用 flatpak 更新 zed") };
        }
        args
    }

    fn get_flatpak_dir() -> Option<PathBuf> {
        if env::var(NO_ESCAPE_ENV_NAME).is_ok() {
            return None;
        }

        if let Ok(flatpak_id) = env::var("FLATPAK_ID") {
            if !flatpak_id.starts_with("dev.zed.Zed") {
                return None;
            }

            let install_dir = Command::new("/usr/bin/flatpak-spawn")
                .arg("--host")
                .arg("flatpak")
                .arg("info")
                .arg("--show-location")
                .arg(flatpak_id)
                .output()
                .unwrap();
            let install_dir = PathBuf::from(String::from_utf8(install_dir.stdout).unwrap().trim());
            Some(install_dir.join("files"))
        } else {
            None
        }
    }

    fn get_xdg_env_args() -> Vec<OsString> {
        let xdg_keys = [
            "XDG_DATA_HOME",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
        ];
        env::vars()
            .filter(|(key, _)| xdg_keys.contains(&key.as_str()))
            .map(|(key, val)| format!("--env=FLATPAK_{}={}", key, val).into())
            .collect()
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use anyhow::Context;
    use release_channel::app_identifier;
    use windows::{
        Win32::{
            Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GENERIC_WRITE, GetLastError},
            Storage::FileSystem::{
                CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE, OPEN_EXISTING, WriteFile,
            },
            System::Threading::CreateMutexW,
        },
        core::HSTRING,
    };

    use crate::{Detect, InstalledApp};
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::ExitStatus;

    fn check_single_instance() -> bool {
        let mutex = unsafe {
            CreateMutexW(
                None,
                false,
                &HSTRING::from(format!("{}-实例互斥锁", app_identifier())),
            )
            .expect("创建实例同步事件失败")
        };
        let last_err = unsafe { GetLastError() };
        let _ = unsafe { CloseHandle(mutex) };
        last_err != ERROR_ALREADY_EXISTS
    }

    struct App(PathBuf);

    impl InstalledApp for App {
        fn zed_version_string(&self) -> String {
            format!(
                "Zed {}{}{} – {}",
                if *release_channel::RELEASE_CHANNEL_NAME == "stable" {
                    "".to_string()
                } else {
                    format!("{} ", *release_channel::RELEASE_CHANNEL_NAME)
                },
                option_env!("RELEASE_VERSION").unwrap_or_default(),
                match option_env!("ZED_COMMIT_SHA") {
                    Some(commit_sha) => format!(" {commit_sha} "),
                    None => "".to_string(),
                },
                self.0.display(),
            )
        }

        fn launch(&self, ipc_url: String, user_data_dir: Option<&str>) -> anyhow::Result<()> {
            if check_single_instance() {
                let mut cmd = std::process::Command::new(self.0.clone());
                cmd.arg(ipc_url);
                if let Some(dir) = user_data_dir {
                    cmd.arg("--user-data-dir").arg(dir);
                }
                cmd.spawn()?;
            } else {
                unsafe {
                    let pipe = CreateFileW(
                        &HSTRING::from(format!("\\\\.\\pipe\\{}-命名管道", app_identifier())),
                        GENERIC_WRITE.0,
                        FILE_SHARE_MODE::default(),
                        None,
                        OPEN_EXISTING,
                        FILE_FLAGS_AND_ATTRIBUTES::default(),
                        None,
                    )?;
                    let message = ipc_url.as_bytes();
                    let mut bytes_written = 0;
                    WriteFile(pipe, Some(message), Some(&mut bytes_written), None)?;
                    CloseHandle(pipe)?;
                }
            }
            Ok(())
        }

        fn run_foreground(
            &self,
            ipc_url: String,
            user_data_dir: Option<&str>,
        ) -> io::Result<ExitStatus> {
            let mut cmd = std::process::Command::new(self.0.clone());
            cmd.arg(ipc_url).arg("--foreground");
            if let Some(dir) = user_data_dir {
                cmd.arg("--user-data-dir").arg(dir);
            }
            cmd.spawn()?.wait()
        }

        fn path(&self) -> PathBuf {
            self.0.clone()
        }
    }

    impl Detect {
        pub fn detect(path: Option<&Path>) -> anyhow::Result<impl InstalledApp> {
            let path = if let Some(path) = path {
                path.to_path_buf().canonicalize()?
            } else {
                let cli = std::env::current_exe()?;
                let dir = cli.parent().context("CLI 无父路径")?;

                // ../Zed.exe 是标准路径，lib/zed 适用于 MSYS2，./zed.exe 适用于开发构建
                let possible_locations = ["../Zed.exe", "../lib/zed/zed-editor.exe", "./zed.exe"];
                possible_locations
                    .iter()
                    .find_map(|p| dir.join(p).canonicalize().ok().filter(|path| path != &cli))
                    .context(format!(
                        "找不到以下任一文件：{}",
                        possible_locations.join(", ")
                    ))?
            };

            Ok(App(path))
        }
    }
}

#[cfg(target_os = "macos")]
mod mac_os {
    use anyhow::{Context as _, Result};
    use core_foundation::{
        array::{CFArray, CFIndex},
        base::TCFType as _,
        string::kCFStringEncodingUTF8,
        url::{CFURL, CFURLCreateWithBytes},
    };
    use core_services::{
        LSLaunchURLSpec, LSOpenFromURLSpec, kLSLaunchDefaults, kLSLaunchDontSwitch,
    };
    use serde::Deserialize;
    use std::{
        ffi::OsStr,
        fs, io,
        path::{Path, PathBuf},
        process::{Command, ExitStatus},
        ptr,
    };

    use cli::FORCE_CLI_MODE_ENV_VAR_NAME;

    use crate::{Detect, InstalledApp};

    #[derive(Debug, Deserialize)]
    struct InfoPlist {
        #[serde(rename = "CFBundleShortVersionString")]
        bundle_short_version_string: String,
    }

    enum Bundle {
        App {
            app_bundle: PathBuf,
            plist: InfoPlist,
        },
        LocalPath {
            executable: PathBuf,
        },
    }

    fn locate_bundle() -> Result<PathBuf> {
        let cli_path = std::env::current_exe()?.canonicalize()?;
        let mut app_path = cli_path.clone();
        while app_path.extension() != Some(OsStr::new("app")) {
            anyhow::ensure!(
                app_path.pop(),
                "无法找到包含 {cli_path:?} 的应用包"
            );
        }
        Ok(app_path)
    }

    impl Detect {
        pub fn detect(path: Option<&Path>) -> anyhow::Result<impl InstalledApp> {
            let bundle_path = if let Some(bundle_path) = path {
                bundle_path
                    .canonicalize()
                    .with_context(|| format!("参数应用包路径 {bundle_path:?} 规范化失败"))?
            } else {
                locate_bundle().context("应用包自动发现失败")?
            };

            match bundle_path.extension().and_then(|ext| ext.to_str()) {
                Some("app") => {
                    let plist_path = bundle_path.join("Contents/Info.plist");
                    let plist =
                        plist::from_file::<_, InfoPlist>(&plist_path).with_context(|| {
                            format!("读取应用包 plist 文件失败 {plist_path:?}")
                        })?;
                    Ok(Bundle::App {
                        app_bundle: bundle_path,
                        plist,
                    })
                }
                _ => Ok(Bundle::LocalPath {
                    executable: bundle_path,
                }),
            }
        }
    }

    impl InstalledApp for Bundle {
        fn zed_version_string(&self) -> String {
            format!("Zed {} – {}", self.version(), self.path().display(),)
        }

        fn launch(&self, url: String, user_data_dir: Option<&str>) -> anyhow::Result<()> {
            match self {
                Self::App { app_bundle, .. } => {
                    let app_path = app_bundle;

                    let status = unsafe {
                        let app_url = CFURL::from_path(app_path, true)
                            .with_context(|| format!("无效应用路径 {app_path:?}"))?;
                        let url_to_open = CFURL::wrap_under_create_rule(CFURLCreateWithBytes(
                            ptr::null(),
                            url.as_ptr(),
                            url.len() as CFIndex,
                            kCFStringEncodingUTF8,
                            ptr::null(),
                        ));
                        // 等价于：open zed-cli:... -a /Applications/Zed\ Preview.app
                        let urls_to_open =
                            CFArray::from_copyable(&[url_to_open.as_concrete_TypeRef()]);
                        LSOpenFromURLSpec(
                            &LSLaunchURLSpec {
                                appURL: app_url.as_concrete_TypeRef(),
                                itemURLs: urls_to_open.as_concrete_TypeRef(),
                                passThruParams: ptr::null(),
                                launchFlags: kLSLaunchDefaults | kLSLaunchDontSwitch,
                                asyncRefCon: ptr::null_mut(),
                            },
                            ptr::null_mut(),
                        )
                    };

                    anyhow::ensure!(
                        status == 0,
                        "无法启动应用包 {}",
                        self.zed_version_string()
                    );
                }

                Self::LocalPath { executable, .. } => {
                    let executable_parent = executable
                        .parent()
                        .with_context(|| format!("可执行文件 {executable:?} 无父路径"))?;
                    let subprocess_stdout_file = fs::File::create(
                        executable_parent.join("zed_dev.log"),
                    )
                    .with_context(|| format!("在 {executable_parent:?} 创建日志文件失败"))?;
                    let subprocess_stdin_file =
                        subprocess_stdout_file.try_clone().with_context(|| {
                            format!("克隆文件描述符失败 {subprocess_stdout_file:?}")
                        })?;
                    let mut command = std::process::Command::new(executable);
                    command.env(FORCE_CLI_MODE_ENV_VAR_NAME, "");
                    if let Some(dir) = user_data_dir {
                        command.arg("--user-data-dir").arg(dir);
                    }
                    command
                        .stderr(subprocess_stdout_file)
                        .stdout(subprocess_stdin_file)
                        .arg(url);

                    command
                        .spawn()
                        .with_context(|| format!("启动进程失败 {command:?}"))?;
                }
            }

            Ok(())
        }

        fn run_foreground(
            &self,
            ipc_url: String,
            user_data_dir: Option<&str>,
        ) -> io::Result<ExitStatus> {
            let path = match self {
                Bundle::App { app_bundle, .. } => app_bundle.join("Contents/MacOS/zed"),
                Bundle::LocalPath { executable, .. } => executable.clone(),
            };

            let mut cmd = std::process::Command::new(path);
            cmd.arg(ipc_url);
            if let Some(dir) = user_data_dir {
                cmd.arg("--user-data-dir").arg(dir);
            }
            cmd.status()
        }

        fn path(&self) -> PathBuf {
            match self {
                Bundle::App { app_bundle, .. } => app_bundle.join("Contents/MacOS/zed"),
                Bundle::LocalPath { executable, .. } => executable.clone(),
            }
        }
    }

    impl Bundle {
        fn version(&self) -> String {
            match self {
                Self::App { plist, .. } => plist.bundle_short_version_string.clone(),
                Self::LocalPath { .. } => "<开发版本>".to_string(),
            }
        }

        fn path(&self) -> &Path {
            match self {
                Self::App { app_bundle, .. } => app_bundle,
                Self::LocalPath { executable, .. } => executable,
            }
        }
    }

    pub(super) fn spawn_channel_cli(
        channel: release_channel::ReleaseChannel,
        leftover_args: Vec<String>,
    ) -> Result<()> {
        use anyhow::bail;

        let app_path_prompt = format!(
            "POSIX path of (path to application \"{}\")",
            channel.display_name()
        );
        let app_path_output = Command::new("osascript")
            .arg("-e")
            .arg(&app_path_prompt)
            .output()?;
        if !app_path_output.status.success() {
            bail!(
                "无法获取 {} 的应用路径",
                channel.display_name()
            );
        }
        let app_path = String::from_utf8(app_path_output.stdout)?.trim().to_owned();
        let cli_path = format!("{app_path}/Contents/MacOS/cli");
        Command::new(cli_path).args(leftover_args).spawn()?;
        Ok(())
    }
}