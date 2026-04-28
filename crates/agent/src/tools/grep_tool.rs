use crate::{AgentTool, ToolCallEventStream, ToolInput};
use agent_client_protocol::schema as acp;
use anyhow::Result;
use futures::{FutureExt as _, StreamExt};
use gpui::{App, Entity, SharedString, Task};
use language::{OffsetRangeExt, ParseStatus, Point};
use project::{
    Project, SearchResults, WorktreeSettings,
    search::{SearchQuery, SearchResult},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::Settings;
use std::{cmp, fmt::Write, sync::Arc};
use util::RangeExt;
use util::markdown::MarkdownInlineCode;
use util::paths::PathMatcher;

/// Searches the contents of files in the project with a regular expression
///
/// - Prefer this tool to path search when searching for symbols in the project, because you won't need to guess what path it's in.
/// - Supports full regex syntax (eg. "log.*Error", "function\\s+\\w+", etc.)
/// - Pass an `include_pattern` if you know how to narrow your search on the files system
/// - Never use this tool to search for paths. Only search file contents with this tool.
/// - Use this tool when you need to find files containing specific patterns
/// - Results are paginated with 20 matches per page. Use the optional 'offset' parameter to request subsequent pages.
/// - DO NOT use HTML entities solely to escape characters in the tool parameters.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GrepToolInput {
    /// A regex pattern to search for in the entire project. Note that the regex will be parsed by the Rust `regex` crate.
    ///
    /// Do NOT specify a path here! This will only be matched against the code **content**.
    pub regex: String,
    /// A glob pattern for the paths of files to include in the search.
    /// Supports standard glob patterns like "**/*.rs" or "frontend/src/**/*.ts".
    /// If omitted, all files in the project will be searched.
    ///
    /// The glob pattern is matched against the full path including the project root directory.
    ///
    /// <example>
    /// If the project has the following root directories:
    ///
    /// - /a/b/backend
    /// - /c/d/frontend
    ///
    /// Use "backend/**/*.rs" to search only Rust files in the backend root directory.
    /// Use "frontend/src/**/*.ts" to search TypeScript files only in the frontend root directory (sub-directory "src").
    /// Use "**/*.rs" to search Rust files across all root directories.
    /// </example>
    pub include_pattern: Option<String>,
    /// Optional starting position for paginated results (0-based).
    /// When not provided, starts from the beginning.
    #[serde(default)]
    pub offset: u32,
    /// Whether the regex is case-sensitive. Defaults to false (case-insensitive).
    #[serde(default)]
    pub case_sensitive: bool,
}

impl GrepToolInput {
    /// Which page of search results this is.
    pub fn page(&self) -> u32 {
        1 + (self.offset / RESULTS_PER_PAGE)
    }
}

const RESULTS_PER_PAGE: u32 = 20;

pub struct GrepTool {
    project: Entity<Project>,
}

impl GrepTool {
    pub fn new(project: Entity<Project>) -> Self {
        Self { project }
    }
}

impl AgentTool for GrepTool {
    type Input = GrepToolInput;
    type Output = String;

    const NAME: &'static str = "grep";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Search
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        match input {
            Ok(input) => {
                let page = input.page();
                let regex_str = MarkdownInlineCode(&input.regex);
                let case_info = if input.case_sensitive {
                    "（区分大小写）"
                } else {
                    ""
                };

                if page > 1 {
                    format!("获取正则表达式 {regex_str}{case_info} 的第 {page} 页搜索结果")
                } else {
                    format!("使用正则表达式 {regex_str}{case_info} 搜索文件")
                }
            }
            Err(_) => "使用正则表达式搜索".into(),
        }
        .into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        // 上下文展示行数
        const CONTEXT_LINES: u32 = 2;
        // 最大祖先节点展示行数
        const MAX_ANCESTOR_LINES: u32 = 10;

        let project = self.project.clone();
        cx.spawn(async move |cx|  {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("接收工具输入失败：{e}"))?;

            let results = cx.update(|cx| {
                let path_style = project.read(cx).path_style(cx);

                let include_matcher = PathMatcher::new(
                    input
                        .include_pattern
                        .as_ref()
                        .into_iter()
                        .collect::<Vec<_>>(),
                    path_style,
                )
                .map_err(|error| format!("包含的通配符模式无效：{error}"))?;

                // 排除全局 file_scan_exclusions 和 private_files 设置
                let exclude_matcher = {
                    let global_settings = WorktreeSettings::get_global(cx);
                    let exclude_patterns = global_settings
                        .file_scan_exclusions
                        .sources()
                        .chain(global_settings.private_files.sources());

                    PathMatcher::new(exclude_patterns, path_style)
                        .map_err(|error| format!("排除的模式无效：{error}"))?
                };

                let query = SearchQuery::regex(
                    &input.regex,
                    false,
                    input.case_sensitive,
                    false,
                    false,
                    include_matcher,
                    exclude_matcher,
                    true, // 始终将文件包含模式与以项目根目录开头的完整项目路径进行匹配
                    None,
                )
                .map_err(|error| error.to_string())?;

                Ok::<_, String>(
                    project.update(cx, |project, cx| project.search(query, cx)),
                )
            })?;

            let project = project.downgrade();
            // 在结果迭代期间保持搜索处于活跃状态，丢弃此任务即为取消机制；故意不分离该任务
            let SearchResults {rx, _task_handle}  = results;
            futures::pin_mut!(rx);

            let mut output = String::new();
            let mut skips_remaining = input.offset;
            let mut matches_found = 0;
            let mut has_more_matches = false;

            'outer: loop {
                let search_result = futures::select! {
                    result = rx.next().fuse() => result,
                    _ = event_stream.cancelled_by_user().fuse() => {
                        return Err("搜索已被用户取消".to_string());
                    }
                };

                let (buffer, ranges) = match search_result {
                    Some(SearchResult::Buffer { buffer, ranges }) => (buffer, ranges),
                    Some(SearchResult::LimitReached) => {
                        has_more_matches = true;
                        break;
                    }
                    Some(SearchResult::WaitingForScan) => continue,
                    None => break,
                };
                if ranges.is_empty() {
                    continue;
                }

                let (Some(path), mut parse_status) = buffer.read_with(cx, |buffer, cx| {
                    (buffer.file().map(|file| file.full_path(cx)), buffer.parse_status())
                }) else {
                    continue;
                };

                // 基于工作树设置检查此文件是否应被排除
                if let Ok(Some(project_path)) = project.read_with(cx, |project, cx| {
                    project.find_project_path(&path, cx)
                }) {
                    if cx.update(|cx| {
                        let worktree_settings = WorktreeSettings::get(Some((&project_path).into()), cx);
                        worktree_settings.is_path_excluded(&project_path.path)
                            || worktree_settings.is_path_private(&project_path.path)
                    }) {
                        continue;
                    }
                }

                while *parse_status.borrow() != ParseStatus::Idle {
                    parse_status.changed().await.map_err(|e| e.to_string())?;
                }

                let snapshot = buffer.read_with(cx, |buffer, _cx| buffer.snapshot());

                let mut ranges = ranges
                    .into_iter()
                    .map(|range| {
                        let matched = range.to_point(&snapshot);
                        let matched_end_line_len = snapshot.line_len(matched.end.row);
                        let full_lines = Point::new(matched.start.row, 0)..Point::new(matched.end.row, matched_end_line_len);
                        let symbols = snapshot.symbols_containing(matched.start, None);

                        if let Some(ancestor_node) = snapshot.syntax_ancestor(full_lines.clone()) {
                            let full_ancestor_range = ancestor_node.byte_range().to_point(&snapshot);
                            let end_row = full_ancestor_range.end.row.min(full_ancestor_range.start.row + MAX_ANCESTOR_LINES);
                            let end_col = snapshot.line_len(end_row);
                            let capped_ancestor_range = Point::new(full_ancestor_range.start.row, 0)..Point::new(end_row, end_col);

                            if capped_ancestor_range.contains_inclusive(&full_lines) {
                                return (capped_ancestor_range, Some(full_ancestor_range), symbols)
                            }
                        }

                        let mut matched = matched;
                        matched.start.column = 0;
                        matched.start.row =
                            matched.start.row.saturating_sub(CONTEXT_LINES);
                        matched.end.row = cmp::min(
                            snapshot.max_point().row,
                            matched.end.row + CONTEXT_LINES,
                        );
                        matched.end.column = snapshot.line_len(matched.end.row);

                        (matched, None, symbols)
                    })
                    .peekable();

                let mut file_header_written = false;

                while let Some((mut range, ancestor_range, parent_symbols)) = ranges.next(){
                    if skips_remaining > 0 {
                        skips_remaining -= 1;
                        continue;
                    }

                    // 已找到完整一页的匹配项，且又发现一个新匹配项
                    if matches_found >= RESULTS_PER_PAGE {
                        has_more_matches = true;
                        break 'outer;
                    }

                    while let Some((next_range, _, _)) = ranges.peek() {
                        if range.end.row >= next_range.start.row {
                            range.end = next_range.end;
                            ranges.next();
                        } else {
                            break;
                        }
                    }

                    if !file_header_written {
                        writeln!(output, "\n## 在 {} 中的匹配项", path.display())
                            .ok();
                        file_header_written = true;
                    }

                    let end_row = range.end.row;
                    output.push_str("\n### ");

                    for symbol in parent_symbols {
                        write!(output, "{} › ", symbol.text)
                            .ok();
                    }

                    if range.start.row == end_row {
                        writeln!(output, "第{}行", range.start.row + 1)
                            .ok();
                    } else {
                        writeln!(output, "第{}-{}行", range.start.row + 1, end_row + 1)
                            .ok();
                    }

                    output.push_str("```\n");
                    output.extend(snapshot.text_for_range(range));
                    output.push_str("\n```\n");

                    if let Some(ancestor_range) = ancestor_range
                        && end_row < ancestor_range.end.row {
                            let remaining_lines = ancestor_range.end.row - end_row;
                            writeln!(output, "\n父节点剩余 {} 行。读取文件查看全部内容。", remaining_lines)
                                .ok();
                        }

                    matches_found += 1;
                }
            }

            if matches_found == 0 {
                Ok("未找到匹配项".into())
            } else if has_more_matches {
                Ok(format!(
                    "显示匹配项 {}-{}（找到更多匹配项；使用偏移量：{} 查看下一页）：\n{output}",
                    input.offset + 1,
                    input.offset + matches_found,
                    input.offset + RESULTS_PER_PAGE,
                ))
            } else {
                Ok(format!("找到 {matches_found} 处匹配：\n{output}"))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::ToolCallEventStream;

    use super::*;
    use gpui::{TestAppContext, UpdateGlobal};
    use project::{FakeFs, Project};
    use serde_json::json;
    use settings::SettingsStore;
    use unindent::Unindent;
    use util::path;

    #[gpui::test]
    async fn test_grep_tool_with_include_pattern(cx: &mut TestAppContext) {
    init_test(cx);
    cx.executor().allow_parking();

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/root"),
        serde_json::json!({
            "src": {
                "main.rs": "fn main() {\n    println!(\"Hello, world!\");\n}",
                "utils": {
                    "helper.rs": "fn helper() {\n    println!(\"I'm a helper!\");\n}",
                },
            },
            "tests": {
                "test_main.rs": "fn test_main() {\n    assert!(true);\n}",
            }
        }),
    )
    .await;

    let project = Project::test(fs.clone(), [path!("/root").as_ref()], cx).await;

    // 测试使用包含模式匹配项目根目录下的 Rust 文件
    let input = GrepToolInput {
        regex: "println".to_string(),
        include_pattern: Some("root/**/*.rs".to_string()),
        offset: 0,
        case_sensitive: false,
    };

    let result = run_grep_tool(input, project.clone(), cx).await;
    assert!(result.contains("main.rs"), "应在 main.rs 中找到匹配项");
    assert!(
        result.contains("helper.rs"),
        "应在 helper.rs 中找到匹配项"
    );
    assert!(
        !result.contains("test_main.rs"),
        "不应包含 test_main.rs，即使它是 .rs 文件（因为不匹配该模式）"
    );

    // 测试仅包含 src 目录的包含模式
    let input = GrepToolInput {
        regex: "fn".to_string(),
        include_pattern: Some("root/**/src/**".to_string()),
        offset: 0,
        case_sensitive: false,
    };

    let result = run_grep_tool(input, project.clone(), cx).await;
    assert!(
        result.contains("main.rs"),
        "应在 src/main.rs 中找到匹配项"
    );
    assert!(
        result.contains("helper.rs"),
        "应在 src/utils/helper.rs 中找到匹配项"
    );
    assert!(
        !result.contains("test_main.rs"),
        "不应包含 test_main.rs，因为它不在 src 目录中"
    );

    // 测试空包含模式（默认匹配所有文件）
    let input = GrepToolInput {
        regex: "fn".to_string(),
        include_pattern: None,
        offset: 0,
        case_sensitive: false,
    };

    let result = run_grep_tool(input, project.clone(), cx).await;
    assert!(result.contains("main.rs"), "应在 main.rs 中找到匹配项");
    assert!(
        result.contains("helper.rs"),
        "应在 helper.rs 中找到匹配项"
    );
    assert!(
        result.contains("test_main.rs"),
        "应包含 test_main.rs"
    );
    }

    #[gpui::test]
    async fn test_grep_tool_with_case_sensitivity(cx: &mut TestAppContext) {
    init_test(cx);
    cx.executor().allow_parking();

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/root"),
        serde_json::json!({
            "case_test.txt": "This file has UPPERCASE and lowercase text.\nUPPERCASE patterns should match only with case_sensitive: true",
        }),
    )
    .await;

    let project = Project::test(fs.clone(), [path!("/root").as_ref()], cx).await;

    // 测试不区分大小写搜索（默认）
    let input = GrepToolInput {
        regex: "uppercase".to_string(),
        include_pattern: Some("**/*.txt".to_string()),
        offset: 0,
        case_sensitive: false,
    };

    let result = run_grep_tool(input, project.clone(), cx).await;
    assert!(
        result.contains("UPPERCASE"),
        "不区分大小写搜索应匹配大写内容"
    );

    // 测试区分大小写搜索
    let input = GrepToolInput {
        regex: "uppercase".to_string(),
        include_pattern: Some("**/*.txt".to_string()),
        offset: 0,
        case_sensitive: true,
    };

    let result = run_grep_tool(input, project.clone(), cx).await;
    assert!(
        !result.contains("UPPERCASE"),
        "区分大小写搜索不应匹配大写内容"
    );

    // 测试区分大小写搜索
    let input = GrepToolInput {
        regex: "LOWERCASE".to_string(),
        include_pattern: Some("**/*.txt".to_string()),
        offset: 0,
        case_sensitive: true,
    };

    let result = run_grep_tool(input, project.clone(), cx).await;

    assert!(
        !result.contains("lowercase"),
        "区分大小写搜索不应匹配小写内容"
    );

    // 测试针对小写模式的区分大小写搜索
    let input = GrepToolInput {
        regex: "lowercase".to_string(),
        include_pattern: Some("**/*.txt".to_string()),
        offset: 0,
        case_sensitive: true,
    };

    let result = run_grep_tool(input, project.clone(), cx).await;
    assert!(
        result.contains("lowercase"),
        "区分大小写搜索应匹配小写文本"
    );
    }

    /// Helper function to set up a syntax test environment
    async fn setup_syntax_test(cx: &mut TestAppContext) -> Entity<Project> {
        use unindent::Unindent;
        init_test(cx);
        cx.executor().allow_parking();

        let fs = FakeFs::new(cx.executor());

        // Create test file with syntax structures
        fs.insert_tree(
            path!("/root"),
            serde_json::json!({
                "test_syntax.rs": r#"
                    fn top_level_function() {
                        println!("This is at the top level");
                    }

                    mod feature_module {
                        pub mod nested_module {
                            pub fn nested_function(
                                first_arg: String,
                                second_arg: i32,
                            ) {
                                println!("Function in nested module");
                                println!("{first_arg}");
                                println!("{second_arg}");
                            }
                        }
                    }

                    struct MyStruct {
                        field1: String,
                        field2: i32,
                    }

                    impl MyStruct {
                        fn method_with_block() {
                            let condition = true;
                            if condition {
                                println!("Inside if block");
                            }
                        }

                        fn long_function() {
                            println!("Line 1");
                            println!("Line 2");
                            println!("Line 3");
                            println!("Line 4");
                            println!("Line 5");
                            println!("Line 6");
                            println!("Line 7");
                            println!("Line 8");
                            println!("Line 9");
                            println!("Line 10");
                            println!("Line 11");
                            println!("Line 12");
                        }
                    }

                    trait Processor {
                        fn process(&self, input: &str) -> String;
                    }

                    impl Processor for MyStruct {
                        fn process(&self, input: &str) -> String {
                            format!("Processed: {}", input)
                        }
                    }
                "#.unindent().trim(),
            }),
        )
        .await;

        let project = Project::test(fs.clone(), [path!("/root").as_ref()], cx).await;

        project.update(cx, |project, _cx| {
            project.languages().add(language::rust_lang())
        });

        project
    }

    #[gpui::test]
    async fn test_grep_top_level_function(cx: &mut TestAppContext) {
        let project = setup_syntax_test(cx).await;

        // Test: Line at the top level of the file
        let input = GrepToolInput {
            regex: "This is at the top level".to_string(),
            include_pattern: Some("**/*.rs".to_string()),
            offset: 0,
            case_sensitive: false,
        };

        let result = run_grep_tool(input, project.clone(), cx).await;
        let expected = r#"
            Found 1 matches:

            ## Matches in root/test_syntax.rs

            ### fn top_level_function › L1-3
            ```
            fn top_level_function() {
                println!("This is at the top level");
            }
            ```
            "#
        .unindent();
        assert_eq!(result, expected);
    }

    #[gpui::test]
    async fn test_grep_function_body(cx: &mut TestAppContext) {
        let project = setup_syntax_test(cx).await;

        // Test: Line inside a function body
        let input = GrepToolInput {
            regex: "Function in nested module".to_string(),
            include_pattern: Some("**/*.rs".to_string()),
            offset: 0,
            case_sensitive: false,
        };

        let result = run_grep_tool(input, project.clone(), cx).await;
        let expected = r#"
            Found 1 matches:

            ## Matches in root/test_syntax.rs

            ### mod feature_module › pub mod nested_module › pub fn nested_function › L10-14
            ```
                    ) {
                        println!("Function in nested module");
                        println!("{first_arg}");
                        println!("{second_arg}");
                    }
            ```
            "#
        .unindent();
        assert_eq!(result, expected);
    }

    #[gpui::test]
    async fn test_grep_function_args_and_body(cx: &mut TestAppContext) {
        let project = setup_syntax_test(cx).await;

        // Test: Line with a function argument
        let input = GrepToolInput {
            regex: "second_arg".to_string(),
            include_pattern: Some("**/*.rs".to_string()),
            offset: 0,
            case_sensitive: false,
        };

        let result = run_grep_tool(input, project.clone(), cx).await;
        let expected = r#"
            Found 1 matches:

            ## Matches in root/test_syntax.rs

            ### mod feature_module › pub mod nested_module › pub fn nested_function › L7-14
            ```
                    pub fn nested_function(
                        first_arg: String,
                        second_arg: i32,
                    ) {
                        println!("Function in nested module");
                        println!("{first_arg}");
                        println!("{second_arg}");
                    }
            ```
            "#
        .unindent();
        assert_eq!(result, expected);
    }

    #[gpui::test]
    async fn test_grep_if_block(cx: &mut TestAppContext) {
        use unindent::Unindent;
        let project = setup_syntax_test(cx).await;

        // Test: Line inside an if block
        let input = GrepToolInput {
            regex: "Inside if block".to_string(),
            include_pattern: Some("**/*.rs".to_string()),
            offset: 0,
            case_sensitive: false,
        };

        let result = run_grep_tool(input, project.clone(), cx).await;
        let expected = r#"
            Found 1 matches:

            ## Matches in root/test_syntax.rs

            ### impl MyStruct › fn method_with_block › L26-28
            ```
                    if condition {
                        println!("Inside if block");
                    }
            ```
            "#
        .unindent();
        assert_eq!(result, expected);
    }

    #[gpui::test]
    async fn test_grep_long_function_top(cx: &mut TestAppContext) {
        use unindent::Unindent;
        let project = setup_syntax_test(cx).await;

        // Test: Line in the middle of a long function - should show message about remaining lines
        let input = GrepToolInput {
            regex: "Line 5".to_string(),
            include_pattern: Some("**/*.rs".to_string()),
            offset: 0,
            case_sensitive: false,
        };

        let result = run_grep_tool(input, project.clone(), cx).await;
        let expected = r#"
            Found 1 matches:

            ## Matches in root/test_syntax.rs

            ### impl MyStruct › fn long_function › L31-41
            ```
                fn long_function() {
                    println!("Line 1");
                    println!("Line 2");
                    println!("Line 3");
                    println!("Line 4");
                    println!("Line 5");
                    println!("Line 6");
                    println!("Line 7");
                    println!("Line 8");
                    println!("Line 9");
                    println!("Line 10");
            ```

            3 lines remaining in ancestor node. Read the file to see all.
            "#
        .unindent();
        assert_eq!(result, expected);
    }

    #[gpui::test]
    async fn test_grep_long_function_bottom(cx: &mut TestAppContext) {
        use unindent::Unindent;
        let project = setup_syntax_test(cx).await;

        // Test: Line in the long function
        let input = GrepToolInput {
            regex: "Line 12".to_string(),
            include_pattern: Some("**/*.rs".to_string()),
            offset: 0,
            case_sensitive: false,
        };

        let result = run_grep_tool(input, project.clone(), cx).await;
        let expected = r#"
            Found 1 matches:

            ## Matches in root/test_syntax.rs

            ### impl MyStruct › fn long_function › L41-45
            ```
                    println!("Line 10");
                    println!("Line 11");
                    println!("Line 12");
                }
            }
            ```
            "#
        .unindent();
        assert_eq!(result, expected);
    }

    async fn run_grep_tool(
        input: GrepToolInput,
        project: Entity<Project>,
        cx: &mut TestAppContext,
    ) -> String {
        let tool = Arc::new(GrepTool { project });
        let task = cx.update(|cx| {
            tool.run(
                ToolInput::resolved(input),
                ToolCallEventStream::test().0,
                cx,
            )
        });

        match task.await {
            Ok(result) => {
                if cfg!(windows) {
                    result.replace("root\\", "root/")
                } else {
                    result
                }
            }
            Err(e) => panic!("Failed to run grep tool: {}", e),
        }
    }

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
        });
    }

    #[gpui::test]
    async fn test_grep_security_boundaries(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());

        fs.insert_tree(
            path!("/"),
            json!({
                "project_root": {
                    "allowed_file.rs": "fn main() { println!(\"This file is in the project\"); }",
                    ".mysecrets": "SECRET_KEY=abc123\nfn secret() { /* private */ }",
                    ".secretdir": {
                        "config": "fn special_configuration() { /* excluded */ }"
                    },
                    ".mymetadata": "fn custom_metadata() { /* excluded */ }",
                    "subdir": {
                        "normal_file.rs": "fn normal_file_content() { /* Normal */ }",
                        "special.privatekey": "fn private_key_content() { /* private */ }",
                        "data.mysensitive": "fn sensitive_data() { /* private */ }"
                    }
                },
                "outside_project": {
                    "sensitive_file.rs": "fn outside_function() { /* This file is outside the project */ }"
                }
            }),
        )
        .await;

        cx.update(|cx| {
            use gpui::UpdateGlobal;
            use settings::SettingsStore;
            SettingsStore::update_global(cx, |store, cx| {
                store.update_user_settings(cx, |settings| {
                    settings.project.worktree.file_scan_exclusions = Some(vec![
                        "**/.secretdir".to_string(),
                        "**/.mymetadata".to_string(),
                    ]);
                    settings.project.worktree.private_files = Some(
                        vec![
                            "**/.mysecrets".to_string(),
                            "**/*.privatekey".to_string(),
                            "**/*.mysensitive".to_string(),
                        ]
                        .into(),
                    );
                });
            });
        });

        let project = Project::test(fs.clone(), [path!("/project_root").as_ref()], cx).await;

        // Searching for files outside the project worktree should return no results
        let result = run_grep_tool(
            GrepToolInput {
                regex: "outside_function".to_string(),
                include_pattern: None,
                offset: 0,
                case_sensitive: false,
            },
            project.clone(),
            cx,
        )
        .await;
        let paths = extract_paths_from_results(&result);
        assert!(
            paths.is_empty(),
            "grep 工具不应找到项目工作树之外的文件"
        );

        // Searching within the project should succeed
        let result = run_grep_tool(
            GrepToolInput {
                regex: "main".to_string(),
                include_pattern: None,
                offset: 0,
                case_sensitive: false,
            },
            project.clone(),
            cx,
        )
        .await;
        let paths = extract_paths_from_results(&result);
        assert!(
            paths.iter().any(|p| p.contains("allowed_file.rs")),
            "grep 工具应能搜索工作树内的文件"
        );

        // Searching files that match file_scan_exclusions should return no results
        let result = run_grep_tool(
            GrepToolInput {
                regex: "special_configuration".to_string(),
                include_pattern: None,
                offset: 0,
                case_sensitive: false,
            },
            project.clone(),
            cx,
        )
        .await;
        let paths = extract_paths_from_results(&result);
        assert!(
            paths.is_empty(),
            "grep 工具不应搜索 .secretdir 目录中的文件（file_scan_exclusions）"
        );

        let result = run_grep_tool(
            GrepToolInput {
                regex: "custom_metadata".to_string(),
                include_pattern: None,
                offset: 0,
                case_sensitive: false,
            },
            project.clone(),
            cx,
        )
        .await;
        let paths = extract_paths_from_results(&result);
        assert!(
            paths.is_empty(),
            "grep 工具不应搜索 .mymetadata 文件（file_scan_exclusions）"
        );

        // Searching private files should return no results
        let result = run_grep_tool(
            GrepToolInput {
                regex: "SECRET_KEY".to_string(),
                include_pattern: None,
                offset: 0,
                case_sensitive: false,
            },
            project.clone(),
            cx,
        )
        .await;
        let paths = extract_paths_from_results(&result);
        assert!(
            paths.is_empty(),
            "grep 工具不应搜索 .mysecrets 文件（private_files）"
        );

        let result = run_grep_tool(
            GrepToolInput {
                regex: "private_key_content".to_string(),
                include_pattern: None,
                offset: 0,
                case_sensitive: false,
            },
            project.clone(),
            cx,
        )
        .await;
        let paths = extract_paths_from_results(&result);

        assert!(
            paths.is_empty(),
            "grep 工具不应搜索 .privatekey 文件（private_files）"
        );

        let result = run_grep_tool(
            GrepToolInput {
                regex: "sensitive_data".to_string(),
                include_pattern: None,
                offset: 0,
                case_sensitive: false,
            },
            project.clone(),
            cx,
        )
        .await;
        let paths = extract_paths_from_results(&result);
        assert!(
            paths.is_empty(),
            "grep 工具不应搜索 .mysensitive 文件（private_files）"
        );

        // Searching a normal file should still work, even with private_files configured
        let result = run_grep_tool(
            GrepToolInput {
                regex: "normal_file_content".to_string(),
                include_pattern: None,
                offset: 0,
                case_sensitive: false,
            },
            project.clone(),
            cx,
        )
        .await;
        let paths = extract_paths_from_results(&result);
        assert!(
            paths.iter().any(|p| p.contains("normal_file.rs")),
            "应能搜索普通文件"
        );

        // Path traversal attempts with .. in include_pattern should not escape project
        let result = run_grep_tool(
            GrepToolInput {
                regex: "outside_function".to_string(),
                include_pattern: Some("../outside_project/**/*.rs".to_string()),
                offset: 0,
                case_sensitive: false,
            },
            project.clone(),
            cx,
        )
        .await;
        let paths = extract_paths_from_results(&result);
        assert!(
            paths.is_empty(),
            "grep 工具不应允许通过相对路径突破项目边界"
        );
    }

    #[gpui::test]
    async fn test_grep_with_multiple_worktree_settings(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());

        // Create first worktree with its own private files
        fs.insert_tree(
            path!("/worktree1"),
            json!({
                ".zed": {
                    "settings.json": r#"{
                        "file_scan_exclusions": ["**/fixture.*"],
                        "private_files": ["**/secret.rs"]
                    }"#
                },
                "src": {
                    "main.rs": "fn main() { let secret_key = \"hidden\"; }",
                    "secret.rs": "const API_KEY: &str = \"secret_value\";",
                    "utils.rs": "pub fn get_config() -> String { \"config\".to_string() }"
                },
                "tests": {
                    "test.rs": "fn test_secret() { assert!(true); }",
                    "fixture.sql": "SELECT * FROM secret_table;"
                }
            }),
        )
        .await;

        // Create second worktree with different private files
        fs.insert_tree(
            path!("/worktree2"),
            json!({
                ".zed": {
                    "settings.json": r#"{
                        "file_scan_exclusions": ["**/internal.*"],
                        "private_files": ["**/private.js", "**/data.json"]
                    }"#
                },
                "lib": {
                    "public.js": "export function getSecret() { return 'public'; }",
                    "private.js": "const SECRET_KEY = \"private_value\";",
                    "data.json": "{\"secret_data\": \"hidden\"}"
                },
                "docs": {
                    "README.md": "# Documentation with secret info",
                    "internal.md": "Internal secret documentation"
                }
            }),
        )
        .await;

        // Set global settings
        cx.update(|cx| {
            SettingsStore::update_global(cx, |store, cx| {
                store.update_user_settings(cx, |settings| {
                    settings.project.worktree.file_scan_exclusions =
                        Some(vec!["**/.git".to_string(), "**/node_modules".to_string()]);
                    settings.project.worktree.private_files =
                        Some(vec!["**/.env".to_string()].into());
                });
            });
        });

        let project = Project::test(
            fs.clone(),
            [path!("/worktree1").as_ref(), path!("/worktree2").as_ref()],
            cx,
        )
        .await;

        // Wait for worktrees to be fully scanned
        cx.executor().run_until_parked();

        // Search for "secret" - should exclude files based on worktree-specific settings
        let result = run_grep_tool(
            GrepToolInput {
                regex: "secret".to_string(),
                include_pattern: None,
                offset: 0,
                case_sensitive: false,
            },
            project.clone(),
            cx,
        )
        .await;
        let paths = extract_paths_from_results(&result);

        // Should find matches in non-private files
        assert!(
    paths.iter().any(|p| p.contains("main.rs")),
    "应在 worktree1/src/main.rs 中找到 'secret'"
);
assert!(
    paths.iter().any(|p| p.contains("test.rs")),
    "应在 worktree1/tests/test.rs 中找到 'secret'"
);
assert!(
    paths.iter().any(|p| p.contains("public.js")),
    "应在 worktree2/lib/public.js 中找到 'secret'"
);
assert!(
    paths.iter().any(|p| p.contains("README.md")),
    "应在 worktree2/docs/README.md 中找到 'secret'"
);

// 不应在基于工作树设置标记为私有/排除的文件中找到匹配项
assert!(
    !paths.iter().any(|p| p.contains("secret.rs")),
    "不应搜索 worktree1/src/secret.rs（本地 private_files 配置）"
);
assert!(
    !paths.iter().any(|p| p.contains("fixture.sql")),
    "不应搜索 worktree1/tests/fixture.sql（本地 file_scan_exclusions 配置）"
);
assert!(
    !paths.iter().any(|p| p.contains("private.js")),
    "不应搜索 worktree2/lib/private.js（本地 private_files 配置）"
);
assert!(
    !paths.iter().any(|p| p.contains("data.json")),
    "不应搜索 worktree2/lib/data.json（本地 private_files 配置）"
);
assert!(
    !paths.iter().any(|p| p.contains("internal.md")),
    "不应搜索 worktree2/docs/internal.md（本地 file_scan_exclusions 配置）"
);
        // Test with `include_pattern` specific to one worktree
        let result = run_grep_tool(
            GrepToolInput {
                regex: "secret".to_string(),
                include_pattern: Some("worktree1/**/*.rs".to_string()),
                offset: 0,
                case_sensitive: false,
            },
            project.clone(),
            cx,
        )
        .await;

        let paths = extract_paths_from_results(&result);

        // Should only find matches in worktree1 *.rs files (excluding private ones)
        assert!(
            paths.iter().any(|p| p.contains("main.rs")),
            "应在 worktree1/src/main.rs 中找到匹配项"
        );
        assert!(
            paths.iter().any(|p| p.contains("test.rs")),
            "应在 worktree1/tests/test.rs 中找到匹配项"
        );
        assert!(
            !paths.iter().any(|p| p.contains("secret.rs")),
            "不应在已排除的 worktree1/src/secret.rs 中找到匹配项"
        );
        assert!(
            paths.iter().all(|p| !p.contains("worktree2")),
            "不应在 worktree2 中找到任何匹配项"
        );
    }

    // Helper function to extract file paths from grep results
    fn extract_paths_from_results(results: &str) -> Vec<String> {
        results
            .lines()
            .filter(|line| line.starts_with("## 在 "))
            .map(|line| {
                line.strip_prefix("## 在 ")
                    .unwrap()
                    .strip_suffix(" 中的匹配项")
                    .unwrap()
                    .trim()
                    .to_string()
            })
            .collect()
    }
}
