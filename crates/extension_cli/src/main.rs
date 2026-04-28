use std::collections::BTreeSet;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ::fs::{CopyOptions, Fs, RealFs, copy_recursive};
use anyhow::{Context as _, Result, anyhow, bail};
use clap::Parser;
use cloud_api_types::ExtensionProvides;
use extension::extension_builder::{CompileExtensionOptions, ExtensionBuilder};
use extension::{ExtensionManifest, ExtensionSnippets};
use language::LanguageConfig;
use reqwest_client::ReqwestClient;
use settings_content::SemanticTokenRules;
use snippet_provider::file_to_snippets;
use snippet_provider::format::VsSnippetsFile;
use task::TaskTemplates;
use tokio::process::Command;
use tree_sitter::{Language, Query, WasmStore};

/// Zed 扩展打包命令行参数
#[derive(Parser, Debug)]
#[command(name = "zed-extension")]
struct Args {
    /// 扩展目录的路径
    #[arg(long)]
    source_dir: PathBuf,
    /// 用于存放打包后扩展的输出目录
    #[arg(long)]
    output_dir: PathBuf,
    /// 下载构建依赖项的临时目录路径
    #[arg(long)]
    scratch_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let args = Args::parse();
    let fs = Arc::new(RealFs::new(None, gpui_platform::background_executor()));
    let engine = wasmtime::Engine::default();
    let mut wasm_store = WasmStore::new(&engine)?;

    let extension_path = args
        .source_dir
        .canonicalize()
        .context("标准化 source_dir 路径失败")?;
    let scratch_dir = args
        .scratch_dir
        .canonicalize()
        .context("标准化 scratch_dir 路径失败")?;
    let output_dir = if args.output_dir.is_relative() {
        env::current_dir()?.join(&args.output_dir)
    } else {
        args.output_dir
    };

    log::info!("正在加载扩展清单");
    let mut manifest = ExtensionManifest::load(fs.clone(), &extension_path).await?;

    log::info!("正在编译扩展");

    let user_agent = format!(
        "Zed Extension CLI/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let http_client = Arc::new(ReqwestClient::user_agent(&user_agent)?);

    let builder = ExtensionBuilder::new(http_client, scratch_dir);
    builder
        .compile_extension(
            &extension_path,
            &mut manifest,
            CompileExtensionOptions { release: true },
            fs.clone(),
        )
        .await
        .context("编译扩展失败")?;

    let extension_provides = manifest.provides();
    validate_extension_features(&extension_provides)?;

    let grammars = test_grammars(&manifest, &extension_path, &mut wasm_store)?;
    test_languages(&manifest, &extension_path, &grammars)?;
    test_themes(&manifest, &extension_path, fs.clone()).await?;
    test_snippets(&manifest, &extension_path, fs.clone()).await?;

    let archive_dir = output_dir.join("archive");
    fs::remove_dir_all(&archive_dir).ok();
    copy_extension_resources(&manifest, &extension_path, &archive_dir, fs.clone())
        .await
        .context("复制扩展资源失败")?;

    let tar_output = Command::new("tar")
        .current_dir(&output_dir)
        .args(["-czvf", "archive.tar.gz", "-C", "archive", "."])
        .output()
        .await
        .context("执行 tar 命令失败")?;
    if !tar_output.status.success() {
        bail!(
            "创建 archive.tar.gz 失败：{}",
            String::from_utf8_lossy(&tar_output.stderr)
        );
    }

    let manifest_json = serde_json::to_string(&cloud_api_types::ExtensionApiManifest {
        name: manifest.name,
        version: manifest.version,
        description: manifest.description,
        authors: manifest.authors,
        schema_version: Some(manifest.schema_version.0),
        repository: manifest
            .repository
            .context("扩展清单中缺少 repository 字段")?,
        wasm_api_version: manifest.lib.version.map(|version| version.to_string()),
        provides: extension_provides,
    })?;
    fs::remove_dir_all(&archive_dir)?;
    fs::write(output_dir.join("manifest.json"), manifest_json.as_bytes())?;

    Ok(())
}

/// 复制扩展所需的所有资源文件到输出目录
async fn copy_extension_resources(
    manifest: &ExtensionManifest,
    extension_path: &Path,
    output_dir: &Path,
    fs: Arc<dyn Fs>,
) -> Result<()> {
    fs::create_dir_all(output_dir).context("创建输出目录失败")?;

    let manifest_toml = toml::to_string(&manifest).context("序列化清单文件失败")?;
    fs::write(output_dir.join("extension.toml"), &manifest_toml)
        .context("写入 extension.toml 失败")?;

    if manifest.lib.kind.is_some() {
        fs::copy(
            extension_path.join("extension.wasm"),
            output_dir.join("extension.wasm"),
        )
        .context("复制 extension.wasm 失败")?;
    }

    if !manifest.grammars.is_empty() {
        let source_grammars_dir = extension_path.join("grammars");
        let output_grammars_dir = output_dir.join("grammars");
        fs::create_dir_all(&output_grammars_dir)?;
        for grammar_name in manifest.grammars.keys() {
            let mut grammar_filename = PathBuf::from(grammar_name.as_ref());
            grammar_filename.set_extension("wasm");
            fs::copy(
                source_grammars_dir.join(&grammar_filename),
                output_grammars_dir.join(&grammar_filename),
            )
            .with_context(|| format!("复制语法文件失败 '{}'", grammar_filename.display()))?;
        }
    }

    if !manifest.themes.is_empty() {
        let output_themes_dir = output_dir.join("themes");
        fs::create_dir_all(&output_themes_dir)?;
        for theme_path in &manifest.themes {
            let theme_path = theme_path.as_std_path();
            fs::copy(
                extension_path.join(theme_path),
                output_themes_dir.join(theme_path.file_name().context("无效的主题路径")?),
            )
            .with_context(|| format!("复制主题文件失败 '{}'", theme_path.display()))?;
        }
    }

    if !manifest.icon_themes.is_empty() {
        let output_icon_themes_dir = output_dir.join("icon_themes");
        fs::create_dir_all(&output_icon_themes_dir)?;
        for icon_theme_path in &manifest.icon_themes {
            let icon_theme_path = icon_theme_path.as_std_path();
            fs::copy(
                extension_path.join(icon_theme_path),
                output_icon_themes_dir.join(
                    icon_theme_path
                        .file_name()
                        .context("无效的图标主题路径")?,
                ),
            )
            .with_context(|| {
                format!("复制图标主题文件失败 '{}'", icon_theme_path.display())
            })?;
        }

        let output_icons_dir = output_dir.join("icons");
        fs::create_dir_all(&output_icons_dir)?;
        copy_recursive(
            fs.as_ref(),
            &extension_path.join("icons"),
            &output_icons_dir,
            CopyOptions {
                overwrite: true,
                ignore_if_exists: false,
            },
        )
        .await
        .context("复制图标资源失败")?;
    }

    for (_, agent_entry) in &manifest.agent_servers {
        if let Some(icon_path) = &agent_entry.icon {
            let source_icon = extension_path.join(icon_path);
            let dest_icon = output_dir.join(icon_path);

            // 如需则创建父目录
            if let Some(parent) = dest_icon.parent() {
                fs::create_dir_all(parent)?;
            }

            fs::copy(&source_icon, &dest_icon)
                .with_context(|| format!("复制代理服务图标失败 '{}'", icon_path))?;
        }
    }

    if !manifest.languages.is_empty() {
        let output_languages_dir = output_dir.join("languages");
        fs::create_dir_all(&output_languages_dir)?;
        for language_path in &manifest.languages {
            let language_path = language_path.as_std_path();
            copy_recursive(
                fs.as_ref(),
                &extension_path.join(language_path),
                &output_languages_dir
                    .join(language_path.file_name().context("无效的语言路径")?),
                CopyOptions {
                    overwrite: true,
                    ignore_if_exists: false,
                },
            )
            .await
            .with_context(|| {
                format!("复制语言目录失败 '{}'", language_path.display())
            })?;
        }
    }

    if !manifest.debug_adapters.is_empty() {
        for (debug_adapter, entry) in &manifest.debug_adapters {
            let schema_path = extension::build_debug_adapter_schema_path(debug_adapter, entry)?;
            let parent = schema_path
                .parent()
                .with_context(|| format!("调试适配器 {} 的 schema 路径为空", debug_adapter))?;
            let schema_path = schema_path.as_std_path();
            fs::create_dir_all(output_dir.join(parent))?;
            copy_recursive(
                fs.as_ref(),
                &extension_path.join(&schema_path),
                &output_dir.join(&schema_path),
                CopyOptions {
                    overwrite: true,
                    ignore_if_exists: false,
                },
            )
            .await
            .with_context(|| {
                format!(
                    "复制调试适配器 schema 失败 '{}'",
                    schema_path.display(),
                )
            })?;
        }
    }

    if let Some(snippets) = manifest.snippets.as_ref() {
        for snippets_path in snippets.paths() {
            let parent = snippets_path.parent();
            if let Some(parent) = parent.filter(|p| p.components().next().is_some()) {
                fs::create_dir_all(output_dir.join(parent))?;
            }
            copy_recursive(
                fs.as_ref(),
                &extension_path.join(&snippets_path),
                &output_dir.join(&snippets_path),
                CopyOptions {
                    overwrite: true,
                    ignore_if_exists: false,
                },
            )
            .await
            .with_context(|| {
                format!("复制代码片段失败 '{}'", snippets_path.display())
            })?;
        }
    }

    Ok(())
}

/// 验证扩展提供的功能是否符合规范
fn validate_extension_features(provides: &BTreeSet<ExtensionProvides>) -> Result<()> {
    if provides.is_empty() {
        bail!("扩展未提供任何功能");
    }

    if provides.contains(&ExtensionProvides::Themes) && provides.len() != 1 {
        bail!("主题扩展不能同时提供其他功能");
    }

    if provides.contains(&ExtensionProvides::IconThemes) && provides.len() != 1 {
        bail!("图标主题扩展不能同时提供其他功能");
    }

    Ok(())
}

/// 测试并加载所有语法文件
fn test_grammars(
    manifest: &ExtensionManifest,
    extension_path: &Path,
    wasm_store: &mut WasmStore,
) -> Result<HashMap<String, Language>> {
    let mut grammars = HashMap::default();
    let grammars_dir = extension_path.join("grammars");

    for grammar_name in manifest.grammars.keys() {
        let mut grammar_path = grammars_dir.join(grammar_name.as_ref());
        grammar_path.set_extension("wasm");

        let wasm = fs::read(&grammar_path)?;
        let language = wasm_store.load_language(grammar_name, &wasm)?;
        log::info!("已加载语法 {grammar_name}");
        grammars.insert(grammar_name.to_string(), language);
    }

    Ok(grammars)
}

/// 测试并验证所有语言配置
fn test_languages(
    manifest: &ExtensionManifest,
    extension_path: &Path,
    grammars: &HashMap<String, Language>,
) -> Result<()> {
    for relative_language_dir in &manifest.languages {
        let language_dir = extension_path.join(relative_language_dir);
        let config_path = language_dir.join(LanguageConfig::FILE_NAME);
        let config = LanguageConfig::load(&config_path)?;
        let grammar = if let Some(name) = &config.grammar {
            Some(
                grammars
                    .get(name.as_ref())
                    .with_context(|| format!("未找到语法文件：'{name}'"))?,
            )
        } else {
            None
        };

        let query_entries = fs::read_dir(&language_dir)?;
        for entry in query_entries {
            let entry = entry?;
            let file_path = entry.path();

            let Some(file_name) = file_path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };

            match file_name {
                LanguageConfig::FILE_NAME => {
                    // 已在上方加载
                }
                SemanticTokenRules::FILE_NAME => {
                    let _token_rules = SemanticTokenRules::load(&file_path)?;
                }
                TaskTemplates::FILE_NAME => {
                    let task_file_content = std::fs::read(&file_path).with_context(|| {
                        anyhow!(
                            "读取任务文件失败 {path}",
                            path = file_path.display()
                        )
                    })?;
                    let _task_templates =
                        serde_json_lenient::from_slice::<TaskTemplates>(&task_file_content)
                            .with_context(|| {
                                anyhow!(
                                    "解析任务文件失败 {path}",
                                    path = file_path.display()
                                )
                            })?;
                }
                _ if file_name.ends_with(".scm") => {
                    let grammar = grammar.with_context(|| {
                        format! {
                            "语言 {} 提供了查询文件 {} 但未关联语法",
                            config.name,
                            file_path.display()
                        }
                    })?;

                    let query_source = fs::read_to_string(&file_path)?;
                    let _query = Query::new(grammar, &query_source)?;
                }
                _ => {}
            }
        }

        log::info!("已加载语言 {}", config.name);
    }

    Ok(())
}

/// 测试并验证所有主题文件
async fn test_themes(
    manifest: &ExtensionManifest,
    extension_path: &Path,
    fs: Arc<dyn Fs>,
) -> Result<()> {
    for relative_theme_path in &manifest.themes {
        let theme_path = extension_path.join(relative_theme_path);
        let theme_family =
            theme_settings::deserialize_user_theme(&fs.load_bytes(&theme_path).await?)?;
        log::info!("已加载主题组 {}", theme_family.name);

        for theme in &theme_family.themes {
            if theme
                .style
                .colors
                .deprecated_scrollbar_thumb_background
                .is_some()
            {
                bail!(
                    r#"主题 "{theme_name}" 使用了已废弃的样式属性：scrollbar_thumb.background，请改用 `scrollbar.thumb.background`"#,
                    theme_name = theme.name
                )
            }
        }
    }

    Ok(())
}

/// 测试并验证所有代码片段
async fn test_snippets(
    manifest: &ExtensionManifest,
    extension_path: &Path,
    fs: Arc<dyn Fs>,
) -> Result<()> {
    for relative_snippet_path in manifest
        .snippets
        .as_ref()
        .map(ExtensionSnippets::paths)
        .into_iter()
        .flatten()
    {
        let snippet_path = extension_path.join(relative_snippet_path);
        let snippets_content = fs.load_bytes(&snippet_path).await?;
        let snippets_file = serde_json_lenient::from_slice::<VsSnippetsFile>(&snippets_content)
            .with_context(|| anyhow!("解析代码片段文件失败 {snippet_path:?}"))?;
        let snippet_errors = file_to_snippets(snippets_file, &snippet_path)
            .flat_map(Result::err)
            .collect::<Vec<_>>();
        let error_count = snippet_errors.len();

        anyhow::ensure!(
            error_count == 0,
            "无法解析文件 {snippet_path:?} 中的 {error_count} 个代码片段{suffix}：\n\n{snippet_errors}",
            suffix = if error_count == 1 { "" } else { "s" },
            snippet_errors = snippet_errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    Ok(())
}