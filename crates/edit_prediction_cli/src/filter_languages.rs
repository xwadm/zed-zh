//! `ep filter-languages` 实现。
//!
//! 该命令用于筛选 JSONL 数据集，仅保留光标位于指定编程语言文件中的示例。
//!
//! # 使用方法
//!
//! ```text
//! ep filter-languages [input.jsonl] --languages rust,python,go
//! ```
//!
//! # 语言检测
//!
//! 语言根据 `cursor_path` 字段的文件扩展名进行检测。
//! 扩展名到语言的映射关系从 `grammars` 库中嵌入的语言配置文件构建。

use anyhow::{Context as _, Result, bail};
use clap::Args;
use collections::HashMap;
use serde::Deserialize;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

#[cfg(not(feature = "dynamic_prompts"))]
mod language_configs_embedded {
    use rust_embed::RustEmbed;

    #[derive(RustEmbed)]
    #[folder = "../grammars/src/"]
    #[include = "*/config.toml"]
    pub struct LanguageConfigs;
}

#[cfg(not(feature = "dynamic_prompts"))]
use language_configs_embedded::LanguageConfigs;

/// 语言配置结构体
#[derive(Debug, Deserialize)]
struct LanguageConfig {
    name: String,
    #[serde(default)]
    path_suffixes: Vec<String>,
}

/// `ep filter-languages` 命令行参数
#[derive(Debug, Args, Clone)]
#[command(
    about = "根据光标文件路径的扩展名，按编程语言筛选 JSONL 数据集",
    after_help = r#"示例：
  # 仅保留 Rust、Python 和 Go 代码
  ep filter-languages 输入文件.jsonl --languages rust,python,go -o 筛选结果.jsonl

  # 按语言筛选，并额外包含指定扩展名文件
  ep filter-languages 输入文件.jsonl --languages rust,python --extensions txt,md -o 筛选结果.jsonl

  # 仅按扩展名筛选（不限制语言）
  ep filter-languages 输入文件.jsonl --extensions cs,java,swift -o 筛选结果.jsonl

  # 列出所有支持的编程语言
  ep filter-languages --list

  # 查看输入文件中的编程语言统计信息
  ep filter-languages 输入文件.jsonl --stats

说明：
  编程语言名称不区分大小写。
  扩展名无需添加点号（例如填写 txt，而非 .txt）。
  可使用 --list 查看所有支持的语言名称。
"#
)]
pub struct FilterLanguagesArgs {
    /// 要包含的编程语言列表（逗号分隔）
    #[arg(long, short = 'l', value_delimiter = ',')]
    pub languages: Option<Vec<String>>,

    /// 要包含的文件扩展名列表（逗号分隔，无需带点）
    #[arg(long, short = 'e', value_delimiter = ',')]
    pub extensions: Option<Vec<String>>,

    /// 列出所有可用的编程语言及其扩展名
    #[arg(long)]
    pub list: bool,

    /// 显示输入数据中的语言分布统计
    #[arg(long)]
    pub stats: bool,

    /// 包含无法检测语言的示例
    #[arg(long)]
    pub include_unknown: bool,

    /// 显示筛选后被排除的前 N 个文件扩展名
    #[arg(long, value_name = "N")]
    pub show_top_excluded: Option<usize>,
}

#[cfg(not(feature = "dynamic_prompts"))]
/// 构建【文件扩展名 -> 语言名称】映射表（嵌入配置模式）
fn build_extension_to_language_map() -> HashMap<String, String> {
    let mut map = HashMap::default();

    for file_path in LanguageConfigs::iter() {
        if let Some(content) = LanguageConfigs::get(&file_path) {
            let content_str = match std::str::from_utf8(&content.data) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let config: LanguageConfig = match toml::from_str(content_str) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for suffix in &config.path_suffixes {
                map.insert(suffix.to_lowercase(), config.name.clone());
            }
        }
    }

    map
}

#[cfg(feature = "dynamic_prompts")]
/// 构建【文件扩展名 -> 语言名称】映射表（动态加载模式）
fn build_extension_to_language_map() -> HashMap<String, String> {
    const LANGUAGES_SRC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../grammars/src");

    let mut map = HashMap::default();

    let languages_dir = Path::new(LANGUAGES_SRC_DIR);
    let entries = match std::fs::read_dir(languages_dir) {
        Ok(e) => e,
        Err(_) => return map,
    };

    for entry in entries.flatten() {
        let config_path = entry.path().join("config.toml");
        if !config_path.exists() {
            continue;
        }

        let content_str = match std::fs::read_to_string(&config_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let config: LanguageConfig = match toml::from_str(&content_str) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for suffix in &config.path_suffixes {
            map.insert(suffix.to_lowercase(), config.name.clone());
        }
    }

    map
}

/// 获取所有语言及其对应的扩展名列表
fn get_all_languages(extension_map: &HashMap<String, String>) -> Vec<(String, Vec<String>)> {
    let mut language_to_extensions: HashMap<String, Vec<String>> = HashMap::default();

    for (ext, lang) in extension_map {
        language_to_extensions
            .entry(lang.clone())
            .or_default()
            .push(ext.clone());
    }

    let mut result: Vec<_> = language_to_extensions.into_iter().collect();
    result.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    for (_, extensions) in &mut result {
        extensions.sort();
    }
    result
}

/// 根据文件路径检测编程语言
fn detect_language(cursor_path: &str, extension_map: &HashMap<String, String>) -> Option<String> {
    let path = Path::new(cursor_path);

    if let Some(ext) = path.extension().and_then(OsStr::to_str) {
        if let Some(lang) = extension_map.get(&ext.to_lowercase()) {
            return Some(lang.clone());
        }
    }

    if let Some(file_name) = path.file_name().and_then(OsStr::to_str) {
        if let Some(lang) = extension_map.get(&file_name.to_lowercase()) {
            return Some(lang.clone());
        }
    }

    None
}

/// 获取文件路径的扩展名
fn get_extension(cursor_path: &str) -> Option<String> {
    let path = Path::new(cursor_path);

    if let Some(ext) = path.extension().and_then(OsStr::to_str) {
        return Some(ext.to_lowercase());
    }

    if let Some(file_name) = path.file_name().and_then(OsStr::to_str) {
        return Some(file_name.to_lowercase());
    }

    None
}

/// 流式读取文件行
fn read_lines_streaming(
    input: Option<&Path>,
) -> Result<Box<dyn Iterator<Item = io::Result<String>>>> {
    let reader: Box<dyn BufRead> = match input {
        Some(path) => {
            let file =
                File::open(path).with_context(|| format!("打开文件失败 '{}'", path.display()))?;
            Box::new(BufReader::new(file))
        }
        None => Box::new(BufReader::new(io::stdin())),
    };
    Ok(Box::new(reader.lines()))
}

/// 从 JSON 行中提取 cursor_path 字段
fn get_cursor_path(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    value
        .get("cursor_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 执行 filter-languages 命令主逻辑
pub fn run_filter_languages(
    args: &FilterLanguagesArgs,
    inputs: &[PathBuf],
    output: Option<&PathBuf>,
) -> Result<()> {
    let extension_map = build_extension_to_language_map();

    if args.list {
        let languages = get_all_languages(&extension_map);
        println!("可用编程语言（共 {} 种）：", languages.len());
        println!();
        for (lang, extensions) in languages {
            println!("  {}: {}", lang, extensions.join(", "));
        }
        return Ok(());
    }

    let input_path: Option<&Path> = match inputs.first().map(|p| p.as_path()) {
        Some(p) if p.as_os_str() == "-" => None,
        Some(p) => Some(p),
        None => None,
    };

    if args.stats {
        let stats_input =
            input_path.with_context(|| "--stats 需要指定输入文件（无法使用标准输入）")?;
        return run_stats(stats_input, &extension_map);
    }

    if args.languages.is_none() && args.extensions.is_none() {
        bail!(
            "--languages 和/或 --extensions 为必填参数（使用 --list 查看可用语言，或 --stats 查看输入分布）"
        );
    }

    let allowed_languages: std::collections::HashSet<String> = args
        .languages
        .as_ref()
        .map(|langs| langs.iter().map(|l| l.to_lowercase()).collect())
        .unwrap_or_default();

    let allowed_extensions: std::collections::HashSet<String> = args
        .extensions
        .as_ref()
        .map(|exts| {
            exts.iter()
                .map(|e| e.trim_start_matches('.').to_lowercase())
                .collect()
        })
        .unwrap_or_default();

    let language_name_lower_map: HashMap<String, String> = get_all_languages(&extension_map)
        .into_iter()
        .map(|(lang, _)| (lang.to_lowercase(), lang))
        .collect();

    if !allowed_languages.is_empty() {
        for lang in &allowed_languages {
            if !language_name_lower_map.contains_key(lang) {
                eprintln!(
                    "警告：'{}' 不是可识别的语言名称。请使用 --list 查看支持的语言列表。",
                    lang
                );
            }
        }
    }

    let lines = read_lines_streaming(input_path)?;

    let mut writer: Box<dyn Write> = match output {
        Some(path) => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("创建目录失败 '{}'", parent.display())
                    })?;
                }
            }
            let file = File::create(path)
                .with_context(|| format!("创建文件失败 '{}'", path.display()))?;
            Box::new(BufWriter::new(file))
        }
        None => Box::new(BufWriter::new(io::stdout())),
    };

    let mut total_count = 0usize;
    let mut included_count = 0usize;
    let mut unknown_count = 0usize;
    let mut excluded_extensions: HashMap<String, usize> = HashMap::default();

    for line_result in lines {
        let line = line_result.context("读取行失败")?;
        if line.trim().is_empty() {
            continue;
        }

        total_count += 1;

        let cursor_path = match get_cursor_path(&line) {
            Some(p) => p,
            None => {
                if args.include_unknown {
                    unknown_count += 1;
                    included_count += 1;
                    writeln!(writer, "{}", line)?;
                }
                continue;
            }
        };

        let language = detect_language(&cursor_path, &extension_map);
        let extension = get_extension(&cursor_path);

        let matches_language = match &language {
            Some(lang) => allowed_languages.contains(&lang.to_lowercase()),
            None => false,
        };

        let matches_extension = match &extension {
            Some(ext) => allowed_extensions.contains(ext),
            None => false,
        };

        let should_include = if matches_language || matches_extension {
            true
        } else if language.is_none() && args.include_unknown {
            unknown_count += 1;
            true
        } else {
            if let Some(ext) = &extension {
                *excluded_extensions.entry(ext.clone()).or_default() += 1;
            }
            false
        };

        if should_include {
            included_count += 1;
            writeln!(writer, "{}", line)?;
        }
    }

    writer.flush()?;

    eprintln!(
        "已筛选 {} 条示例，保留 {} 条（其中 {} 条语言未知）",
        total_count, included_count, unknown_count
    );

    if let Some(top_n) = args.show_top_excluded {
        if !excluded_extensions.is_empty() {
            let mut sorted: Vec<_> = excluded_extensions.into_iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(&a.1));
            eprintln!("\n被排除最多的前 {} 个扩展名：", top_n.min(sorted.len()));
            for (ext, count) in sorted.into_iter().take(top_n) {
                eprintln!("  {:>6}  .{}", count, ext);
            }
        }
    }

    Ok(())
}

/// 运行语言分布统计
fn run_stats(input: &Path, extension_map: &HashMap<String, String>) -> Result<()> {
    let lines = read_lines_streaming(Some(input))?;

    let mut language_counts: HashMap<String, usize> = HashMap::default();
    let mut unknown_extensions: HashMap<String, usize> = HashMap::default();
    let mut total_count = 0usize;

    for line_result in lines {
        let line = line_result.context("读取行失败")?;
        if line.trim().is_empty() {
            continue;
        }

        total_count += 1;

        let cursor_path = match get_cursor_path(&line) {
            Some(p) => p,
            None => {
                *language_counts
                    .entry("<无 cursor_path 字段>".to_string())
                    .or_default() += 1;
                continue;
            }
        };

        match detect_language(&cursor_path, extension_map) {
            Some(lang) => {
                *language_counts.entry(lang).or_default() += 1;
            }
            None => {
                let ext = Path::new(&cursor_path)
                    .extension()
                    .and_then(OsStr::to_str)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        Path::new(&cursor_path)
                            .file_name()
                            .and_then(OsStr::to_str)
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "<无扩展名>".to_string())
                    });
                *unknown_extensions.entry(ext).or_default() += 1;
                *language_counts.entry("<未知语言>".to_string()).or_default() += 1;
            }
        }
    }

    let mut sorted_counts: Vec<_> = language_counts.into_iter().collect();
    sorted_counts.sort_by(|a, b| b.1.cmp(&a.1));

    println!("语言分布统计（总示例数：{}）：", total_count);
    println!();
    for (lang, count) in &sorted_counts {
        let pct = (*count as f64 / total_count as f64) * 100.0;
        println!("  {:>6} ({:>5.1}%)  {}", count, pct, lang);
    }

    if !unknown_extensions.is_empty() {
        println!();
        println!("未知扩展名统计：");
        let mut sorted_unknown: Vec<_> = unknown_extensions.into_iter().collect();
        sorted_unknown.sort_by(|a, b| b.1.cmp(&a.1));
        for (ext, count) in sorted_unknown.iter().take(30) {
            println!("  {:>6}  .{}", count, ext);
        }
        if sorted_unknown.len() > 30 {
            println!("  ... 及其他 {} 个", sorted_unknown.len() - 30);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// 测试扩展名映射表构建
    fn test_build_extension_map() {
        let map = build_extension_to_language_map();
        assert!(!map.is_empty());
        assert_eq!(map.get("rs"), Some(&"Rust".to_string()));
        assert_eq!(map.get("py"), Some(&"Python".to_string()));
        assert_eq!(map.get("go"), Some(&"Go".to_string()));
    }

    #[test]
    /// 测试按扩展名检测语言
    fn test_detect_language_by_extension() {
        let map = build_extension_to_language_map();

        assert_eq!(
            detect_language("src/main.rs", &map),
            Some("Rust".to_string())
        );
        assert_eq!(
            detect_language("lib/foo.py", &map),
            Some("Python".to_string())
        );
        assert_eq!(
            detect_language("cmd/server.go", &map),
            Some("Go".to_string())
        );
        assert_eq!(detect_language("index.tsx", &map), Some("TSX".to_string()));
    }

    #[test]
    /// 测试按文件名检测语言
    fn test_detect_language_by_filename() {
        let map = build_extension_to_language_map();

        // PKGBUILD 是基于文件名匹配 Shell Script
        assert_eq!(
            detect_language("PKGBUILD", &map),
            Some("Shell Script".to_string())
        );
        assert_eq!(
            detect_language("project/PKGBUILD", &map),
            Some("Shell Script".to_string())
        );
        // .env 文件也属于 Shell Script
        assert_eq!(
            detect_language(".env", &map),
            Some("Shell Script".to_string())
        );
    }

    #[test]
    /// 测试检测未知语言
    fn test_detect_language_unknown() {
        let map = build_extension_to_language_map();

        assert_eq!(detect_language("file.xyz123", &map), None);
        assert_eq!(detect_language("random_file", &map), None);
    }

    #[test]
    /// 测试提取 cursor_path 字段
    fn test_get_cursor_path() {
        let line = r#"{"cursor_path": "src/main.rs", "other": "data"}"#;
        assert_eq!(get_cursor_path(line), Some("src/main.rs".to_string()));

        let line_no_cursor = r#"{"other": "data"}"#;
        assert_eq!(get_cursor_path(line_no_cursor), None);

        let invalid_json = "not json";
        assert_eq!(get_cursor_path(invalid_json), None);
    }

    #[test]
    /// 测试获取所有语言列表
    fn test_get_all_languages() {
        let map = build_extension_to_language_map();
        let languages = get_all_languages(&map);

        assert!(!languages.is_empty());

        let rust_entry = languages.iter().find(|(name, _)| name == "Rust");
        assert!(rust_entry.is_some());
        let (_, rust_extensions) = rust_entry.unwrap();
        assert!(rust_extensions.contains(&"rs".to_string()));
    }
}