use super::*;
use crate::{
    AgentTool, EditFileMode, EditFileTool, EditFileToolInput, GrepTool, GrepToolInput,
    ListDirectoryTool, ListDirectoryToolInput, ReadFileTool, ReadFileToolInput,
};
use Role::*;
use client::{Client, RefreshLlmTokenListener, UserStore};
use eval_utils::{EvalOutput, EvalOutputProcessor, OutcomeKind};
use fs::FakeFs;
use futures::{FutureExt, future::LocalBoxFuture};
use gpui::{AppContext, TestAppContext};
use http_client::StatusCode;
use indoc::{formatdoc, indoc};
use language_model::{
    LanguageModelRegistry, LanguageModelToolResult, LanguageModelToolResultContent,
    LanguageModelToolUse, LanguageModelToolUseId, SelectedModel,
};
use project::Project;
use prompt_store::{ProjectContext, WorktreeContext};
use rand::prelude::*;
use reqwest_client::ReqwestClient;
use serde_json::json;
use std::{
    fmt::{self, Display},
    path::Path,
    str::FromStr,
    time::Duration,
};
use util::path;

#[derive(Default, Clone, Debug)]
struct EditAgentOutputProcessor {
    mismatched_tag_threshold: f32,
    cumulative_tags: usize,
    cumulative_mismatched_tags: usize,
    eval_outputs: Vec<EvalOutput<EditEvalMetadata>>,
}

fn mismatched_tag_threshold(mismatched_tag_threshold: f32) -> EditAgentOutputProcessor {
    EditAgentOutputProcessor {
        mismatched_tag_threshold,
        cumulative_tags: 0,
        cumulative_mismatched_tags: 0,
        eval_outputs: Vec::new(),
    }
}

#[derive(Clone, Debug)]
struct EditEvalMetadata {
    tags: usize,
    mismatched_tags: usize,
}

impl EvalOutputProcessor for EditAgentOutputProcessor {
    type Metadata = EditEvalMetadata;

    fn process(&mut self, output: &EvalOutput<Self::Metadata>) {
        if matches!(output.outcome, OutcomeKind::Passed | OutcomeKind::Failed) {
            self.cumulative_mismatched_tags += output.metadata.mismatched_tags;
            self.cumulative_tags += output.metadata.tags;
            self.eval_outputs.push(output.clone());
        }
    }

    fn assert(&mut self) {
        let mismatched_tag_ratio =
            self.cumulative_mismatched_tags as f32 / self.cumulative_tags as f32;
        if mismatched_tag_ratio > self.mismatched_tag_threshold {
            for eval_output in &self.eval_outputs {
                println!("{}", eval_output.data);
            }
            panic!(
                "标签不匹配数量过多：{:?}",
                self.cumulative_mismatched_tags
            );
        }
    }
}

#[test]
#[cfg_attr(not(feature = "unit-eval"), ignore)]
fn eval_extract_handle_command_output() {
    // Test how well agent generates multiple edit hunks.
    //
    // Model                       | Pass rate
    // ----------------------------|----------
    // claude-3.7-sonnet           |  0.99 (2025-06-14)
    // claude-sonnet-4             |  0.97 (2025-06-14)
    // gemini-2.5-pro-06-05        |  0.98 (2025-06-16)
    // gemini-2.5-flash            |  0.11 (2025-05-22)

    let input_file_path = "root/blame.rs";
    let input_file_content = include_str!("evals/fixtures/extract_handle_command_output/before.rs");
    let possible_diffs = vec![
        include_str!("evals/fixtures/extract_handle_command_output/possible-01.diff"),
        include_str!("evals/fixtures/extract_handle_command_output/possible-02.diff"),
        include_str!("evals/fixtures/extract_handle_command_output/possible-03.diff"),
        include_str!("evals/fixtures/extract_handle_command_output/possible-04.diff"),
        include_str!("evals/fixtures/extract_handle_command_output/possible-05.diff"),
        include_str!("evals/fixtures/extract_handle_command_output/possible-06.diff"),
        include_str!("evals/fixtures/extract_handle_command_output/possible-07.diff"),
    ];
    let edit_description = "从 `run_git_blame` 中提取 `handle_command_output` 方法。";
    eval_utils::eval(100, 0.95, mismatched_tag_threshold(0.05), move || {
    run_eval(EvalInput::from_conversation(
        vec![
            message(
                User,
                [text(formatdoc! {"
                        读取 `{input_file_path}` 文件，并在 `run_git_blame` 的最后一段代码中
                        提取一个处理命令失败的方法，命名为 `handle_command_output`，
                        仅接收 std::process::Output 作为唯一参数。
                        不要为该方法添加文档注释，也不要添加任何额外注释。

                        将该方法添加到 `run_git_blame` 旁边，并直接从 `run_git_blame` 中原样复制代码。
                    "})],
            ),
            message(
                Assistant,
                [tool_use(
                    "tool_1",
                    ReadFileTool::NAME,
                    ReadFileToolInput {
                        path: input_file_path.into(),
                        start_line: None,
                        end_line: None,
                    },
                )],
            ),
            message(
                User,
                [tool_result(
                    "tool_1",
                    ReadFileTool::NAME,
                    input_file_content,
                )],
            ),
            message(
                Assistant,
                [tool_use(
                    "tool_2",
                    EditFileTool::NAME,
                    EditFileToolInput {
                        display_description: edit_description.into(),
                        path: input_file_path.into(),
                        mode: EditFileMode::Edit,
                    },
                )],
            ),
        ],
        Some(input_file_content.into()),
        EvalAssertion::assert_diff_any(possible_diffs.clone()),
    ))
});
}

#[test]
#[cfg_attr(not(feature = "unit-eval"), ignore)]
fn eval_delete_run_git_blame() {
    // 模型名称                      | 通过率
    // ----------------------------|----------
    // claude-3.7-sonnet           | 1.0  (2025-06-14)
    // claude-sonnet-4             | 0.96 (2025-06-14)
    // gemini-2.5-pro-06-05        | 1.0  (2025-06-16)
    // gemini-2.5-flash            |

    let input_file_path = "root/blame.rs";
    let input_file_content = include_str!("evals/fixtures/delete_run_git_blame/before.rs");
    let output_file_content = include_str!("evals/fixtures/delete_run_git_blame/after.rs");
    let edit_description = "删除 `run_git_blame` 函数。";

    eval_utils::eval(100, 0.95, mismatched_tag_threshold(0.05), move || {
        run_eval(EvalInput::from_conversation(
            vec![
                message(
                    User,
                    [text(formatdoc! {"
                            读取 `{input_file_path}` 文件并删除 `run_git_blame`。
                            只删除这个函数，不要删除它的调用处。
                        "})],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_1",
                        ReadFileTool::NAME,
                        ReadFileToolInput {
                            path: input_file_path.into(),
                            start_line: None,
                            end_line: None,
                        },
                    )],
                ),
                message(
                    User,
                    [tool_result(
                        "tool_1",
                        ReadFileTool::NAME,
                        input_file_content,
                    )],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_2",
                        EditFileTool::NAME,
                        EditFileToolInput {
                            display_description: edit_description.into(),
                            path: input_file_path.into(),
                            mode: EditFileMode::Edit,
                        },
                    )],
                ),
            ],
            Some(input_file_content.into()),
            EvalAssertion::assert_eq(output_file_content),
        ))
    });
}

#[test]
#[cfg_attr(not(feature = "unit-eval"), ignore)]
fn eval_translate_doc_comments() {
    //  模型名称                          | 通过率
    // ============================================
    //
    //  claude-3.7-sonnet              |  1.0  (2025-06-14)
    //  claude-sonnet-4                |  1.0  (2025-06-14)
    //  gemini-2.5-pro-preview-03-25   |  1.0  (2025-05-22)
    //  gemini-2.5-flash-preview-04-17 |

    let input_file_path = "root/canvas.rs";
    let input_file_content = include_str!("evals/fixtures/translate_doc_comments/before.rs");
    let edit_description = "将所有文档注释翻译成意大利语";

    eval_utils::eval(200, 1., mismatched_tag_threshold(0.05), move || {
        run_eval(EvalInput::from_conversation(
            vec![
                message(
                    User,
                    [text(formatdoc! {"
                            读取 {input_file_path} 文件并进行编辑（不要覆盖原文件），
                            将所有文档注释翻译成意大利语。
                        "})],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_1",
                        ReadFileTool::NAME,
                        ReadFileToolInput {
                            path: input_file_path.into(),
                            start_line: None,
                            end_line: None,
                        },
                    )],
                ),
                message(
                    User,
                    [tool_result(
                        "tool_1",
                        ReadFileTool::NAME,
                        input_file_content,
                    )],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_2",
                        EditFileTool::NAME,
                        EditFileToolInput {
                            display_description: edit_description.into(),
                            path: input_file_path.into(),
                            mode: EditFileMode::Edit,
                        },
                    )],
                ),
            ],
            Some(input_file_content.into()),
            EvalAssertion::judge_diff("文档注释已翻译成意大利语"),
        ))
    });
}

#[test]
#[cfg_attr(not(feature = "unit-eval"), ignore)]
fn eval_use_wasi_sdk_in_compile_parser_to_wasm() {
    //  模型名称                          | 通过率
    // ============================================
    //
    //  claude-3.7-sonnet              |  0.96 (2025-06-14)
    //  claude-sonnet-4                |  0.11 (2025-06-14)
    //  gemini-2.5-pro-preview-latest  |  0.99 (2025-06-16)
    //  gemini-2.5-flash-preview-04-17 |

    let input_file_path = "root/lib.rs";
    let input_file_content =
        include_str!("evals/fixtures/use_wasi_sdk_in_compile_parser_to_wasm/before.rs");
    let edit_description = "更新 compile_parser_to_wasm，使用 wasi-sdk 替代 emscripten";

    eval_utils::eval(100, 0.95, mismatched_tag_threshold(0.05), move || {
        run_eval(EvalInput::from_conversation(
            vec![
                message(
                    User,
                    [text(formatdoc! {"
                            读取 `{input_file_path}` 文件，修改 `compile_parser_to_wasm` 函数，
                            使用 `wasi-sdk` 替代 emscripten。
                            使用 `ureq` 为当前平台和架构下载 SDK。
                            将压缩包解压到缓存目录中 `tree-sitter` 目录下、`lib` 的同级位置。
                            使用压缩包内的 `bin/clang`（Windows 上为 `bin/clang.exe`）
                            将解析器编译为 WebAssembly。
                            如果该可执行文件已存在，则不要重新下载 SDK。

                            使用这些 Clang 编译参数：-fPIC -shared -Os -Wl,--export=tree_sitter_{{language_name}}

                            可用的 wasi-sdk 资源包：
                            - wasi-sdk-25.0-x86_64-macos.tar.gz
                            - wasi-sdk-25.0-arm64-macos.tar.gz
                            - wasi-sdk-25.0-x86_64-linux.tar.gz
                            - wasi-sdk-25.0-arm64-linux.tar.gz
                            - wasi-sdk-25.0-x86_64-linux.tar.gz
                            - wasi-sdk-25.0-arm64-linux.tar.gz
                            - wasi-sdk-25.0-x86_64-windows.tar.gz
                        "})],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_1",
                        ReadFileTool::NAME,
                        ReadFileToolInput {
                            path: input_file_path.into(),
                            start_line: Some(971),
                            end_line: Some(1050),
                        },
                    )],
                ),
                message(
                    User,
                    [tool_result(
                        "tool_1",
                        ReadFileTool::NAME,
                        lines(input_file_content, 971..1050),
                    )],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_2",
                        ReadFileTool::NAME,
                        ReadFileToolInput {
                            path: input_file_path.into(),
                            start_line: Some(1050),
                            end_line: Some(1100),
                        },
                    )],
                ),
                message(
                    User,
                    [tool_result(
                        "tool_2",
                        ReadFileTool::NAME,
                        lines(input_file_content, 1050..1100),
                    )],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_3",
                        ReadFileTool::NAME,
                        ReadFileToolInput {
                            path: input_file_path.into(),
                            start_line: Some(1100),
                            end_line: Some(1150),
                        },
                    )],
                ),
                message(
                    User,
                    [tool_result(
                        "tool_3",
                        ReadFileTool::NAME,
                        lines(input_file_content, 1100..1150),
                    )],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_4",
                        EditFileTool::NAME,
                        EditFileToolInput {
                            display_description: edit_description.into(),
                            path: input_file_path.into(),
                            mode: EditFileMode::Edit,
                        },
                    )],
                ),
            ],
            Some(input_file_content.into()),
            EvalAssertion::judge_diff(indoc! {"
                    - compile_parser_to_wasm 方法已修改为使用 wasi-sdk
                    - 使用 ureq 为当前平台和架构下载 SDK
                "}),
        ))
    });
}

#[test]
#[cfg_attr(not(feature = "unit-eval"), ignore)]
fn eval_disable_cursor_blinking() {
    //  模型名称                          | 通过率
    // ============================================
    //
    //  claude-3.7-sonnet              |  0.59 (2025-07-14)
    //  claude-sonnet-4                |  0.81 (2025-07-14)
    //  gemini-2.5-pro                 |  0.95 (2025-07-14)
    //  gemini-2.5-flash-preview-04-17 |  0.78 (2025-07-14)

    let input_file_path = "root/editor.rs";
    let input_file_content = include_str!("evals/fixtures/disable_cursor_blinking/before.rs");
    let edit_description = "注释掉对 `BlinkManager::enable` 的调用";
    let possible_diffs = vec![
        include_str!("evals/fixtures/disable_cursor_blinking/possible-01.diff"),
        include_str!("evals/fixtures/disable_cursor_blinking/possible-02.diff"),
        include_str!("evals/fixtures/disable_cursor_blinking/possible-03.diff"),
        include_str!("evals/fixtures/disable_cursor_blinking/possible-04.diff"),
    ];
    eval_utils::eval(100, 0.51, mismatched_tag_threshold(0.05), move || {
        run_eval(EvalInput::from_conversation(
            vec![
                message(User, [text("我们研究一下光标闪烁是如何工作的。")]),
                message(
                    Assistant,
                    [tool_use(
                        "tool_1",
                        GrepTool::NAME,
                        GrepToolInput {
                            regex: "blink".into(),
                            include_pattern: None,
                            offset: 0,
                            case_sensitive: false,
                        },
                    )],
                ),
                message(
                    User,
                    [tool_result(
                        "tool_1",
                        GrepTool::NAME,
                        [
                            lines(input_file_content, 100..400),
                            lines(input_file_content, 800..1300),
                            lines(input_file_content, 1600..2000),
                            lines(input_file_content, 5000..5500),
                            lines(input_file_content, 8000..9000),
                            lines(input_file_content, 18455..18470),
                            lines(input_file_content, 20000..20500),
                            lines(input_file_content, 21000..21300),
                        ]
                        .join("找到匹配项：\n\n"),
                    )],
                ),
                message(
                    User,
                    [text(indoc! {"
                            注释掉所有与 BlinkManager 交互的代码行。
                            保留外层的 update 代码块，但将内部的所有内容（包括 if 语句）都注释掉。
                            不要添加额外的注释。
                        "})],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_4",
                        EditFileTool::NAME,
                        EditFileToolInput {
                            display_description: edit_description.into(),
                            path: input_file_path.into(),
                            mode: EditFileMode::Edit,
                        },
                    )],
                ),
            ],
            Some(input_file_content.into()),
            EvalAssertion::assert_diff_any(possible_diffs.clone()),
        ))
    });
}

#[test]
#[cfg_attr(not(feature = "unit-eval"), ignore)]
fn eval_from_pixels_constructor() {
    // Results for 2025-06-13
    //
    // The outcome of this evaluation depends heavily on the LINE_HINT_TOLERANCE
    // value. Higher values improve the pass rate but may sometimes cause
    // edits to be misapplied. In the context of this eval, this means
    // the agent might add from_pixels tests in incorrect locations
    // (e.g., at the beginning of the file), yet the evaluation may still
    // rate it highly.
    //
    //  Model                          | Date        | Pass rate
    // =========================================================
    //  claude-4.0-sonnet              | 2025-06-14  | 0.99
    //  claude-3.7-sonnet              | 2025-06-14  | 0.88
    //  gemini-2.5-pro-preview-06-05   | 2025-06-16  | 0.98

    let input_file_path = "root/canvas.rs";
    let input_file_content = include_str!("evals/fixtures/from_pixels_constructor/before.rs");
    let edit_description = "实现 from_pixels 构造函数并添加测试。";

    eval_utils::eval(100, 0.95, mismatched_tag_threshold(0.25), move || {
        run_eval(EvalInput::from_conversation(
            vec![
                message(
                    User,
                    [text(indoc! {"
                            在 Canvas 中新增一个 from_pixels 构造函数，
                            并在同一文件中为其添加测试。
                        "})],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_1",
                        ReadFileTool::NAME,
                        ReadFileToolInput {
                            path: input_file_path.into(),
                            start_line: None,
                            end_line: None,
                        },
                    )],
                ),
                message(
                    User,
                    [tool_result(
                        "tool_1",
                        ReadFileTool::NAME,
                        input_file_content,
                    )],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_2",
                        GrepTool::NAME,
                        GrepToolInput {
                            regex: "mod\\s+tests".into(),
                            include_pattern: Some("font-kit/src/canvas.rs".into()),
                            offset: 0,
                            case_sensitive: false,
                        },
                    )],
                ),
                message(
                    User,
                    [tool_result("tool_2", GrepTool::NAME, "未找到匹配项")],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_3",
                        GrepTool::NAME,
                        GrepToolInput {
                            regex: "mod\\s+tests".into(),
                            include_pattern: Some("font-kit/src/**/*.rs".into()),
                            offset: 0,
                            case_sensitive: false,
                        },
                    )],
                ),
                message(
                    User,
                    [tool_result("tool_3", GrepTool::NAME, "未找到匹配项")],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_4",
                        GrepTool::NAME,
                        GrepToolInput {
                            regex: "#\\[test\\]".into(),
                            include_pattern: Some("font-kit/src/**/*.rs".into()),
                            offset: 0,
                            case_sensitive: false,
                        },
                    )],
                ),
                message(
                    User,
                    [tool_result(
                        "tool_4",
                        GrepTool::NAME,
                        indoc! {"
                                Found 6 matches:

                                ## Matches in font-kit/src/loaders/core_text.rs

                                ### mod test › L926-936
                                ```
                                mod test {
                                    use super::Font;
                                    use crate::properties::{Stretch, Weight};

                                    #[cfg(feature = \"source\")]
                                    use crate::source::SystemSource;

                                    static TEST_FONT_POSTSCRIPT_NAME: &'static str = \"ArialMT\";

                                    #[cfg(feature = \"source\")]
                                    #[test]
                                ```

                                55 lines remaining in ancestor node. Read the file to see all.

                                ### mod test › L947-951
                                ```
                                    }

                                    #[test]
                                    fn test_core_text_to_css_font_weight() {
                                        // Exact matches
                                ```

                                ### mod test › L959-963
                                ```
                                    }

                                    #[test]
                                    fn test_core_text_to_css_font_stretch() {
                                        // Exact matches
                                ```

                                ## Matches in font-kit/src/loaders/freetype.rs

                                ### mod test › L1238-1248
                                ```
                                mod test {
                                    use crate::loaders::freetype::Font;

                                    static PCF_FONT_PATH: &str = \"resources/tests/times-roman-pcf/timR12.pcf\";
                                    static PCF_FONT_POSTSCRIPT_NAME: &str = \"Times-Roman\";

                                    #[test]
                                    fn get_pcf_postscript_name() {
                                        let font = Font::from_path(PCF_FONT_PATH, 0).unwrap();
                                        assert_eq!(font.postscript_name().unwrap(), PCF_FONT_POSTSCRIPT_NAME);
                                    }
                                ```

                                1 lines remaining in ancestor node. Read the file to see all.

                                ## Matches in font-kit/src/sources/core_text.rs

                                ### mod test › L265-275
                                ```
                                mod test {
                                    use crate::properties::{Stretch, Weight};

                                    #[test]
                                    fn test_css_to_core_text_font_weight() {
                                        // Exact matches
                                        assert_eq!(super::css_to_core_text_font_weight(Weight(100.0)), -0.7);
                                        assert_eq!(super::css_to_core_text_font_weight(Weight(400.0)), 0.0);
                                        assert_eq!(super::css_to_core_text_font_weight(Weight(700.0)), 0.4);
                                        assert_eq!(super::css_to_core_text_font_weight(Weight(900.0)), 0.8);

                                ```

                                27 lines remaining in ancestor node. Read the file to see all.

                                ### mod test › L278-282
                                ```
                                    }

                                    #[test]
                                    fn test_css_to_core_text_font_stretch() {
                                        // Exact matches
                                ```
                            "},
                    )],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_5",
                        EditFileTool::NAME,
                        EditFileToolInput {
                            display_description: edit_description.into(),
                            path: input_file_path.into(),
                            mode: EditFileMode::Edit,
                        },
                    )],
                ),
            ],
            Some(input_file_content.into()),
            EvalAssertion::judge_diff(indoc! {"
                        - 差异内容包含新增的 from_pixels 构造函数
                        - 差异内容包含为 from_pixels 构造函数新增的测试
                    "}),
        ))
    });
}

#[test]
#[cfg_attr(not(feature = "unit-eval"), ignore)]
fn eval_zode() {
    //  模型                          | 通过率
    // ============================================
    //
    //  claude-3.7-sonnet              |  1.0 (2025-06-14)
    //  claude-sonnet-4                |  1.0 (2025-06-14)
    //  gemini-2.5-pro-preview-03-25   |  1.0 (2025-05-22)
    //  gemini-2.5-flash-preview-04-17 |  1.0 (2025-05-22)

    let input_file_path = "root/zode.py";
    let input_content = None;
    let edit_description = "创建主 Zode 命令行脚本";

    eval_utils::eval(50, 1., mismatched_tag_threshold(0.05), move || {
        run_eval(EvalInput::from_conversation(
            vec![
                message(User, [text(include_str!("evals/fixtures/zode/prompt.md"))]),
                message(
                    Assistant,
                    [
                        tool_use(
                            "tool_1",
                            ReadFileTool::NAME,
                            ReadFileToolInput {
                                path: "root/eval/react.py".into(),
                                start_line: None,
                                end_line: None,
                            },
                        ),
                        tool_use(
                            "tool_2",
                            ReadFileTool::NAME,
                            ReadFileToolInput {
                                path: "root/eval/react_test.py".into(),
                                start_line: None,
                                end_line: None,
                            },
                        ),
                    ],
                ),
                message(
                    User,
                    [
                        tool_result(
                            "tool_1",
                            ReadFileTool::NAME,
                            include_str!("evals/fixtures/zode/react.py"),
                        ),
                        tool_result(
                            "tool_2",
                            ReadFileTool::NAME,
                            include_str!("evals/fixtures/zode/react_test.py"),
                        ),
                    ],
                ),
                message(
                    Assistant,
                    [
                        text(
                            "现在我已明确需要构建的内容，将创建主 Python 脚本：",
                        ),
                        tool_use(
                            "tool_3",
                            EditFileTool::NAME,
                            EditFileToolInput {
                                display_description: edit_description.into(),
                                path: input_file_path.into(),
                                mode: EditFileMode::Create,
                            },
                        ),
                    ],
                ),
            ],
            input_content.clone(),
            EvalAssertion::new(async move |sample, _, _cx| {
                let invalid_starts = [' ', '`', '\n'];
                let mut message = String::new();
                for start in invalid_starts {
                    if sample.text_after.starts_with(start) {
                        message.push_str(&format!("样本以 {:?} 开头\n", start));
                        break;
                    }
                }
                // 移除末尾换行符
                message.pop();

                if message.is_empty() {
                    Ok(EvalAssertionOutcome {
                        score: 100,
                        message: None,
                    })
                } else {
                    Ok(EvalAssertionOutcome {
                        score: 0,
                        message: Some(message),
                    })
                }
            }),
        ))
    });
}

#[test]
#[cfg_attr(not(feature = "unit-eval"), ignore)]
fn eval_add_overwrite_test() {
    //  模型                          | 通过率
    // ============================================
    //
    //  claude-3.7-sonnet              |  0.65 (2025-06-14)
    //  claude-sonnet-4                |  0.07 (2025-06-14)
    //  gemini-2.5-pro-preview-03-25   |  0.35 (2025-05-22)
    //  gemini-2.5-flash-preview-04-17 |

    let input_file_path = "root/action_log.rs";
    let input_file_content = include_str!("evals/fixtures/add_overwrite_test/before.rs");
    let edit_description = "在 action_log.rs 中添加一个用于覆盖文件的新测试";

    eval_utils::eval(200, 0.5, mismatched_tag_threshold(0.05), move || {
        run_eval(EvalInput::from_conversation(
            vec![
                message(
                    User,
                    [text(indoc! {"
                            在 action_log.rs 中新增一个测试，用于验证文件覆盖场景。
                            即：文件已存在，但我们调用 buffer_created 方法，就像该文件是新建的一样。
                            参考文件中其他所有测试的编写风格。
                        "})],
                ),
                message(
                    Assistant,
                    [tool_use(
                        "tool_1",
                        ReadFileTool::NAME,
                        ReadFileToolInput {
                            path: input_file_path.into(),
                            start_line: None,
                            end_line: None,
                        },
                    )],
                ),
                message(
                    User,
                    [tool_result(
                        "tool_1",
                        ReadFileTool::NAME,
                        indoc! {"
                                pub struct ActionLog [L13-20]
                                 tracked_buffers [L15]
                                 edited_since_project_diagnostics_check [L17]
                                 project [L19]
                                impl ActionLog [L22-498]
                                 pub fn new [L24-30]
                                 pub fn project [L32-34]
                                 pub fn checked_project_diagnostics [L37-39]
                                 pub fn has_edited_files_since_project_diagnostics_check [L42-44]
                                 fn track_buffer_internal [L46-101]
                                 fn handle_buffer_event [L103-116]
                                 fn handle_buffer_edited [L118-123]
                                 fn handle_buffer_file_changed [L125-158]
                                 async fn maintain_diff [L160-264]
                                 pub fn buffer_read [L267-269]
                                 pub fn buffer_created [L272-276]
                                 pub fn buffer_edited [L279-287]
                                 pub fn will_delete_buffer [L289-304]
                                 pub fn keep_edits_in_range [L306-364]
                                 pub fn reject_edits_in_ranges [L366-459]
                                 pub fn keep_all_edits [L461-473]
                                 pub fn changed_buffers [L476-482]
                                 pub fn stale_buffers [L485-497]
                                fn apply_non_conflicting_edits [L500-561]
                                fn diff_snapshots [L563-585]
                                fn point_to_row_edit [L587-614]
                                enum ChangeAuthor [L617-620]
                                 User [L618]
                                 Agent [L619]
                                enum TrackedBufferStatus [L623-627]
                                 Created [L624]
                                 Modified [L625]
                                 Deleted [L626]
                                struct TrackedBuffer [L629-641]
                                 buffer [L630]
                                 base_text [L631]
                                 unreviewed_changes [L632]
                                 status [L633]
                                 version [L634]
                                 diff [L635]
                                 snapshot [L636]
                                 diff_update [L637]
                                 _open_lsp_handle [L638]
                                 _maintain_diff [L639]
                                 _subscription [L640]
                                impl TrackedBuffer [L643-657]
                                 fn has_changes [L644-650]
                                 fn schedule_diff_update [L652-656]
                                pub struct ChangedBuffer [L659-661]
                                 pub diff [L660]
                                mod tests [L664-1574]
                                 fn init_logger [L678-682]
                                 fn init_test [L684-691]
                                 async fn test_keep_edits [L694-769]
                                 async fn test_deletions [L772-854]
                                 async fn test_overlapping_user_edits [L857-951]
                                 async fn test_creating_files [L954-1010]
                                 async fn test_deleting_files [L1013-1120]
                                 async fn test_reject_edits [L1123-1255]
                                 async fn test_reject_multiple_edits [L1258-1331]
                                 async fn test_reject_deleted_file [L1334-1388]
                                 async fn test_reject_created_file [L1391-1443]
                                 async fn test_random_diffs [L1446-1535]
                                  fn quiesce [L1510-1534]
                                 struct HunkStatus [L1538-1542]
                                  range [L1539]
                                  diff_status [L1540]
                                  old_text [L1541]
                                 fn unreviewed_hunks [L1544-1573]

                                显示符号 1-69（总符号数：69）

                                可以使用此大纲中的行号，再次调用此工具并指定 start_line 和 end_line 字段，
                                以查看大纲中符号的具体实现。
                            "},
                    )],
                ),
                message(
                    Assistant,
                    [
                        text(
                            "让我们查看现有的文件创建相关测试用例，理解编写模式：",
                        ),
                        tool_use(
                            "tool_2",
                            ReadFileTool::NAME,
                            ReadFileToolInput {
                                path: input_file_path.into(),
                                start_line: Some(953),
                                end_line: Some(1010),
                            },
                        ),
                    ],
                ),
                message(
                    User,
                    [tool_result(
                        "tool_2",
                        ReadFileTool::NAME,
                        lines(input_file_content, 953..1010),
                    )],
                ),
                message(
                    Assistant,
                    [
                        text(
                            "现在让我们查看另一个相关测试，理解其结构：",
                        ),
                        tool_use(
                            "tool_3",
                            ReadFileTool::NAME,
                            ReadFileToolInput {
                                path: input_file_path.into(),
                                start_line: Some(1012),
                                end_line: Some(1120),
                            },
                        ),
                    ],
                ),
                message(
                    User,
                    [tool_result(
                        "tool_3",
                        ReadFileTool::NAME,
                        lines(input_file_content, 1012..1120),
                    )],
                ),
                message(
                    Assistant,
                    [
                        text("现在让我们查看 buffer_created 的实现方式："),
                        tool_use(
                            "tool_4",
                            ReadFileTool::NAME,
                            ReadFileToolInput {
                                path: input_file_path.into(),
                                start_line: Some(271),
                                end_line: Some(276),
                            },
                        ),
                    ],
                ),
                message(
                    User,
                    [tool_result(
                        "tool_4",
                        ReadFileTool::NAME,
                        lines(input_file_content, 271..276),
                    )],
                ),
                message(
                    Assistant,
                    [
                        text(indoc! {"
                                根据我对现有测试和代码的理解，现在将创建一个文件覆盖的新测试。在该测试中，我们将：

                                1. 创建一个文件
                                2. 为其打开缓冲区
                                3. 在操作日志中跟踪该缓冲区
                                4. 修改缓冲区内容
                                5. 再次调用 buffer_created，就像该文件是新建的一样
                                6. 验证变更被正确跟踪

                                开始编写测试：
                            "}),
                        tool_use(
                            "tool_5",
                            EditFileTool::NAME,
                            EditFileToolInput {
                                display_description: edit_description.into(),
                                path: input_file_path.into(),
                                mode: EditFileMode::Edit,
                            },
                        ),
                    ],
                ),
            ],
            Some(input_file_content.into()),
            EvalAssertion::judge_diff(
                "已创建文件覆盖的新测试，未修改任何原有测试",
            ),
        ))
    });
}

#[test]
#[cfg_attr(not(feature = "unit-eval"), ignore)]
fn eval_create_empty_file() {
    // 检查编辑智能体能否创建空文件，且不将自身思考内容写入文件。
    // 该问题并非空文件特有，但使用空文件更容易复现。
    //
    //  模型                          | 通过率
    // ============================================
    //
    //  claude-3.7-sonnet              |  1.00 (2025-06-14)
    //  claude-sonnet-4                |  1.00 (2025-06-14)
    //  gemini-2.5-pro-preview-03-25   |  1.00 (2025-05-21)
    //  gemini-2.5-flash-preview-04-17 |  1.00 (2025-05-21)

    let input_file_content = None;
    let expected_output_content = String::new();

    eval_utils::eval(100, 0.99, mismatched_tag_threshold(0.05), move || {
        run_eval(EvalInput::from_conversation(
            vec![
                message(User, [text("创建第二个空的待办事项文件")]),
                message(
                    Assistant,
                    [
                        text(formatdoc! {"
                            我将帮你创建第二个空的待办事项文件。
                            首先，我检查项目结构，确认是否已存在待办文件，以便为第二个文件确定合适的名称和位置。
                            "}),
                        tool_use(
                            "toolu_01GAF8TtsgpjKxCr8fgQLDgR",
                            ListDirectoryTool::NAME,
                            ListDirectoryToolInput {
                                path: "root".to_string(),
                            },
                        ),
                    ],
                ),
                message(
                    User,
                    [tool_result(
                        "toolu_01GAF8TtsgpjKxCr8fgQLDgR",
                        ListDirectoryTool::NAME,
                        "root/TODO\nroot/TODO2\nroot/new.txt\n",
                    )],
                ),
                message(
                    Assistant,
                    [
                        text(formatdoc! {"
                            我看到 root 目录下已存在 TODO 文件。现在在同一目录创建第二个空待办文件，命名为 TODO3：
                        "}),
                        tool_use(
                            "toolu_01Tb3iQ9griqSYMmVuykQPWU",
                            EditFileTool::NAME,
                            EditFileToolInput {
                                display_description: "创建空的 TODO3 文件".to_string(),
                                mode: EditFileMode::Create,
                                path: "root/TODO3".into(),
                            },
                        ),
                    ],
                ),
            ],
            input_file_content.clone(),
            // 错误行为是写入类似如下内容
            // "我将按照要求创建空的 TODO3 文件。"
            EvalAssertion::assert_eq(expected_output_content.clone()),
        ))
    });
}

fn message(
    role: Role,
    contents: impl IntoIterator<Item = MessageContent>,
) -> LanguageModelRequestMessage {
    LanguageModelRequestMessage {
        role,
        content: contents.into_iter().collect(),
        cache: false,
        reasoning_details: None,
    }
}

fn text(text: impl Into<String>) -> MessageContent {
    MessageContent::Text(text.into())
}

fn lines(input: &str, range: Range<usize>) -> String {
    input
        .lines()
        .skip(range.start)
        .take(range.len())
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_use(
    id: impl Into<Arc<str>>,
    name: impl Into<Arc<str>>,
    input: impl Serialize,
) -> MessageContent {
    MessageContent::ToolUse(LanguageModelToolUse {
        id: LanguageModelToolUseId::from(id.into()),
        name: name.into(),
        raw_input: serde_json::to_string_pretty(&input).unwrap(),
        input: serde_json::to_value(input).unwrap(),
        is_input_complete: true,
        thought_signature: None,
    })
}

fn tool_result(
    id: impl Into<Arc<str>>,
    name: impl Into<Arc<str>>,
    result: impl Into<Arc<str>>,
) -> MessageContent {
    MessageContent::ToolResult(LanguageModelToolResult {
        tool_use_id: LanguageModelToolUseId::from(id.into()),
        tool_name: name.into(),
        is_error: false,
        content: LanguageModelToolResultContent::Text(result.into()),
        output: None,
    })
}

#[derive(Clone)]
struct EvalInput {
    conversation: Vec<LanguageModelRequestMessage>,
    edit_file_input: EditFileToolInput,
    input_content: Option<String>,
    assertion: EvalAssertion,
}

impl EvalInput {
    fn from_conversation(
        conversation: Vec<LanguageModelRequestMessage>,
        input_content: Option<String>,
        assertion: EvalAssertion,
    ) -> Self {
        let msg = conversation.last().expect("Conversation must not be empty");
        if msg.role != Role::Assistant {
            panic!("Conversation must end with an assistant message");
        }
        let tool_use = msg
            .content
            .iter()
            .flat_map(|content| match content {
                MessageContent::ToolUse(tool_use) if tool_use.name == EditFileTool::NAME.into() => {
                    Some(tool_use)
                }
                _ => None,
            })
            .next()
            .expect("Conversation must end with an edit_file tool use")
            .clone();

        let edit_file_input: EditFileToolInput = serde_json::from_value(tool_use.input).unwrap();

        EvalInput {
            conversation,
            edit_file_input,
            input_content,
            assertion,
        }
    }
}

#[derive(Clone)]
struct EvalSample {
    text_before: String,
    text_after: String,
    edit_output: EditAgentOutput,
    diff: String,
}

trait AssertionFn: 'static + Send + Sync {
    fn assert<'a>(
        &'a self,
        sample: &'a EvalSample,
        judge_model: Arc<dyn LanguageModel>,
        cx: &'a mut TestAppContext,
    ) -> LocalBoxFuture<'a, Result<EvalAssertionOutcome>>;
}

impl<F> AssertionFn for F
where
    F: 'static
        + Send
        + Sync
        + AsyncFn(
            &EvalSample,
            Arc<dyn LanguageModel>,
            &mut TestAppContext,
        ) -> Result<EvalAssertionOutcome>,
{
    fn assert<'a>(
        &'a self,
        sample: &'a EvalSample,
        judge_model: Arc<dyn LanguageModel>,
        cx: &'a mut TestAppContext,
    ) -> LocalBoxFuture<'a, Result<EvalAssertionOutcome>> {
        (self)(sample, judge_model, cx).boxed_local()
    }
}

#[derive(Clone)]
struct EvalAssertion(Arc<dyn AssertionFn>);

impl EvalAssertion {
    fn new<F>(f: F) -> Self
    where
        F: 'static
            + Send
            + Sync
            + AsyncFn(
                &EvalSample,
                Arc<dyn LanguageModel>,
                &mut TestAppContext,
            ) -> Result<EvalAssertionOutcome>,
    {
        EvalAssertion(Arc::new(f))
    }

    fn assert_eq(expected: impl Into<String>) -> Self {
        let expected = expected.into();
        Self::new(async move |sample, _judge, _cx| {
            Ok(EvalAssertionOutcome {
                score: if strip_empty_lines(&sample.text_after) == strip_empty_lines(&expected) {
                    100
                } else {
                    0
                },
                message: None,
            })
        })
    }

    fn assert_diff_any(expected_diffs: Vec<impl Into<String>>) -> Self {
        let expected_diffs: Vec<String> = expected_diffs.into_iter().map(Into::into).collect();
        Self::new(async move |sample, _judge, _cx| {
            let matches = expected_diffs.iter().any(|possible_diff| {
                let expected =
                    language::apply_diff_patch(&sample.text_before, possible_diff).unwrap();
                strip_empty_lines(&expected) == strip_empty_lines(&sample.text_after)
            });

            Ok(EvalAssertionOutcome {
                score: if matches { 100 } else { 0 },
                message: None,
            })
        })
    }

    fn judge_diff(assertions: &'static str) -> Self {
        Self::new(async move |sample, judge, cx| {
            let prompt = DiffJudgeTemplate {
                diff: sample.diff.clone(),
                assertions,
            }
            .render(&Templates::new())
            .unwrap();

            let request = LanguageModelRequest {
                messages: vec![LanguageModelRequestMessage {
                    role: Role::User,
                    content: vec![prompt.into()],
                    cache: false,
                    reasoning_details: None,
                }],
                thinking_allowed: true,
                ..Default::default()
            };
            let mut response = retry_on_rate_limit(async || {
                Ok(judge
                    .stream_completion_text(request.clone(), &cx.to_async())
                    .await?)
            })
            .await?;
            let mut output = String::new();
            while let Some(chunk) = response.stream.next().await {
                let chunk = chunk?;
                output.push_str(&chunk);
            }

            // Parse the score from the response
            let re = regex::Regex::new(r"<score>(\d+)</score>").unwrap();
            if let Some(captures) = re.captures(&output)
                && let Some(score_match) = captures.get(1)
            {
                let score = score_match.as_str().parse().unwrap_or(0);
                return Ok(EvalAssertionOutcome {
                    score,
                    message: Some(output),
                });
            }

            anyhow::bail!("响应中未找到分数。原始输出：{output}");
        })
    }

    async fn run(
        &self,
        input: &EvalSample,
        judge_model: Arc<dyn LanguageModel>,
        cx: &mut TestAppContext,
    ) -> Result<EvalAssertionOutcome> {
        self.0.assert(input, judge_model, cx).await
    }
}

fn run_eval(eval: EvalInput) -> eval_utils::EvalOutput<EditEvalMetadata> {
    let dispatcher = gpui::TestDispatcher::new(rand::random());
    let mut cx = TestAppContext::build(dispatcher, None);
    let foreground_executor = cx.foreground_executor().clone();
    let result = foreground_executor.block_test(async {
        let test = EditAgentTest::new(&mut cx).await;
        test.eval(eval, &mut cx).await
    });
    cx.quit();
    match result {
        Ok(output) => eval_utils::EvalOutput {
            data: output.to_string(),
            outcome: if output.assertion.score < 80 {
                eval_utils::OutcomeKind::Failed
            } else {
                eval_utils::OutcomeKind::Passed
            },
            metadata: EditEvalMetadata {
                tags: output.sample.edit_output.parser_metrics.tags,
                mismatched_tags: output.sample.edit_output.parser_metrics.mismatched_tags,
            },
        },
        Err(e) => eval_utils::EvalOutput {
            data: format!("{e:?}"),
            outcome: eval_utils::OutcomeKind::Error,
            metadata: EditEvalMetadata {
                tags: 0,
                mismatched_tags: 0,
            },
        },
    }
}

#[derive(Clone)]
struct EditEvalOutput {
    sample: EvalSample,
    assertion: EvalAssertionOutcome,
}

impl Display for EditEvalOutput {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "分数：{:?}", self.assertion.score)?;
        if let Some(message) = self.assertion.message.as_ref() {
            writeln!(f, "信息：{}", message)?;
        }

        writeln!(f, "差异：\n{}", self.sample.diff)?;

        writeln!(
            f,
            "解析器指标：\n{:#?}",
            self.sample.edit_output.parser_metrics
        )?;
        writeln!(f, "原始编辑内容：\n{}", self.sample.edit_output.raw_edits)?;
        Ok(())
    }
}

struct EditAgentTest {
    agent: EditAgent,
    project: Entity<Project>,
    judge_model: Arc<dyn LanguageModel>,
}

impl EditAgentTest {
    async fn new(cx: &mut TestAppContext) -> Self {
        cx.executor().allow_parking();

        let fs = FakeFs::new(cx.executor());
        cx.update(|cx| {
            settings::init(cx);
            gpui_tokio::init(cx);
            let http_client = Arc::new(ReqwestClient::user_agent("agent tests").unwrap());
            cx.set_http_client(http_client);
            let client = Client::production(cx);
            let user_store = cx.new(|cx| UserStore::new(client.clone(), cx));
            settings::init(cx);
            language_model::init(cx);
            RefreshLlmTokenListener::register(client.clone(), user_store.clone(), cx);
            language_models::init(user_store, client.clone(), cx);
        });

        fs.insert_tree("/root", json!({})).await;
        let project = Project::test(fs.clone(), [path!("/root").as_ref()], cx).await;
        let agent_model = SelectedModel::from_str(
            &std::env::var("ZED_AGENT_MODEL").unwrap_or("anthropic/claude-sonnet-4-latest".into()),
        )
        .unwrap();
        let judge_model = SelectedModel::from_str(
            &std::env::var("ZED_JUDGE_MODEL").unwrap_or("anthropic/claude-sonnet-4-latest".into()),
        )
        .unwrap();

        let authenticate_provider_tasks = cx.update(|cx| {
            LanguageModelRegistry::global(cx).update(cx, |registry, cx| {
                registry
                    .providers()
                    .iter()
                    .map(|p| p.authenticate(cx))
                    .collect::<Vec<_>>()
            })
        });
        let (agent_model, judge_model) = cx
            .update(|cx| {
                cx.spawn(async move |cx| {
                    futures::future::join_all(authenticate_provider_tasks).await;
                    let agent_model = Self::load_model(&agent_model, cx).await;
                    let judge_model = Self::load_model(&judge_model, cx).await;
                    (agent_model.unwrap(), judge_model.unwrap())
                })
            })
            .await;
        let action_log = cx.new(|_| ActionLog::new(project.clone()));

        let edit_format = EditFormat::from_env(agent_model.clone()).unwrap();

        Self {
            agent: EditAgent::new(
                agent_model,
                project.clone(),
                action_log,
                Templates::new(),
                edit_format,
                true,
                true,
            ),
            project,
            judge_model,
        }
    }

    async fn load_model(
        selected_model: &SelectedModel,
        cx: &mut AsyncApp,
    ) -> Result<Arc<dyn LanguageModel>> {
        cx.update(|cx| {
            let registry = LanguageModelRegistry::read_global(cx);
            let provider = registry
                .provider(&selected_model.provider)
                .expect("未找到服务提供商");
            provider.authenticate(cx)
        })
        .await?;
        Ok(cx.update(|cx| {
            let models = LanguageModelRegistry::read_global(cx);
            let model = models
                .available_models(cx)
                .find(|model| {
                    model.provider_id() == selected_model.provider
                        && model.id() == selected_model.model
                })
                .unwrap_or_else(|| panic!("未找到模型：{}", selected_model.model.0));
            model
        }))
    }

    async fn eval(&self, mut eval: EvalInput, cx: &mut TestAppContext) -> Result<EditEvalOutput> {
        // 确保对话中的最后一条消息已缓存
        eval.conversation.last_mut().unwrap().cache = true;

        let path = self
            .project
            .read_with(cx, |project, cx| {
                project.find_project_path(eval.edit_file_input.path, cx)
            })
            .unwrap();
        let buffer = self
            .project
            .update(cx, |project, cx| project.open_buffer(path, cx))
            .await
            .unwrap();

        let tools = crate::built_in_tools().collect::<Vec<_>>();

        let system_prompt = {
            let worktrees = vec![WorktreeContext {
                root_name: "root".to_string(),
                abs_path: Path::new("/path/to/root").into(),
                rules_file: None,
            }];
            let project_context = ProjectContext::new(worktrees, Vec::default());
            let tool_names = tools
                .iter()
                .map(|tool| tool.name.clone().into())
                .collect::<Vec<_>>();
            let template = crate::SystemPromptTemplate {
                project: &project_context,
                available_tools: tool_names,
                model_name: None,
            };
            let templates = Templates::new();
            template.render(&templates).unwrap()
        };

        let has_system_prompt = eval
            .conversation
            .first()
            .is_some_and(|msg| msg.role == Role::System);
        let messages = if has_system_prompt {
            eval.conversation
        } else {
            [LanguageModelRequestMessage {
                role: Role::System,
                content: vec![MessageContent::Text(system_prompt)],
                cache: true,
                reasoning_details: None,
            }]
            .into_iter()
            .chain(eval.conversation)
            .collect::<Vec<_>>()
        };

        let conversation = LanguageModelRequest {
            messages,
            tools,
            thinking_allowed: true,
            ..Default::default()
        };

        let edit_output = if matches!(eval.edit_file_input.mode, EditFileMode::Edit) {
            if let Some(input_content) = eval.input_content.as_deref() {
                buffer.update(cx, |buffer, cx| buffer.set_text(input_content, cx));
            }
            retry_on_rate_limit(async || {
                self.agent
                    .edit(
                        buffer.clone(),
                        eval.edit_file_input.display_description.clone(),
                        &conversation,
                        &mut cx.to_async(),
                    )
                    .0
                    .await
            })
            .await?
        } else {
            retry_on_rate_limit(async || {
                self.agent
                    .overwrite(
                        buffer.clone(),
                        eval.edit_file_input.display_description.clone(),
                        &conversation,
                        &mut cx.to_async(),
                    )
                    .0
                    .await
            })
            .await?
        };

        let buffer_text = buffer.read_with(cx, |buffer, _| buffer.text());
        let sample = EvalSample {
            edit_output,
            diff: language::unified_diff(
                eval.input_content.as_deref().unwrap_or_default(),
                &buffer_text,
            ),
            text_before: eval.input_content.unwrap_or_default(),
            text_after: buffer_text,
        };
        let assertion = eval
            .assertion
            .run(&sample, self.judge_model.clone(), cx)
            .await?;

        Ok(EditEvalOutput { assertion, sample })
    }
}

async fn retry_on_rate_limit<R>(mut request: impl AsyncFnMut() -> Result<R>) -> Result<R> {
    const MAX_RETRIES: usize = 20;
    let mut attempt = 0;

    loop {
        attempt += 1;
        let response = request().await;

        if attempt >= MAX_RETRIES {
            return response;
        }

        let retry_delay = match &response {
            Ok(_) => None,
            Err(err) => match err.downcast_ref::<LanguageModelCompletionError>() {
                Some(err) => match &err {
                    LanguageModelCompletionError::RateLimitExceeded { retry_after, .. }
                    | LanguageModelCompletionError::ServerOverloaded { retry_after, .. } => {
                        Some(retry_after.unwrap_or(Duration::from_secs(5)))
                    }
                    LanguageModelCompletionError::UpstreamProviderError {
                        status,
                        retry_after,
                        ..
                    } => {
                        // Only retry for specific status codes
                        let should_retry = matches!(
                            *status,
                            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
                        ) || status.as_u16() == 529;

                        if should_retry {
                            // Use server-provided retry_after if available, otherwise use default
                            Some(retry_after.unwrap_or(Duration::from_secs(5)))
                        } else {
                            None
                        }
                    }
                    LanguageModelCompletionError::ApiReadResponseError { .. }
                    | LanguageModelCompletionError::ApiInternalServerError { .. }
                    | LanguageModelCompletionError::HttpSend { .. } => {
                        // Exponential backoff for transient I/O and internal server errors
                        Some(Duration::from_secs(2_u64.pow((attempt - 1) as u32).min(30)))
                    }
                    _ => None,
                },
                _ => None,
            },
        };

        if let Some(retry_after) = retry_delay {
            let jitter = retry_after.mul_f64(rand::rng().random_range(0.0..1.0));
            eprintln!("尝试 #{attempt}: {retry_after:?} 后重试，附加抖动 {jitter:?}");
            // This code does not use the gpui::executor
            #[allow(clippy::disallowed_methods)]
            smol::Timer::after(retry_after + jitter).await;
        } else {
            return response;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct EvalAssertionOutcome {
    score: usize,
    message: Option<String>,
}

#[derive(Serialize)]
pub struct DiffJudgeTemplate {
    diff: String,
    assertions: &'static str,
}

impl Template for DiffJudgeTemplate {
    const TEMPLATE_NAME: &'static str = "diff_judge.hbs";
}

fn strip_empty_lines(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
