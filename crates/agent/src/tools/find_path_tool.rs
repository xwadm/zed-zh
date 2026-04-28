use crate::{AgentTool, ToolCallEventStream, ToolInput};
use agent_client_protocol::schema as acp;
use anyhow::{Result, anyhow};
use futures::FutureExt as _;
use gpui::{App, AppContext, Entity, SharedString, Task};
use language_model::LanguageModelToolResultContent;
use project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::{cmp, path::PathBuf, sync::Arc};
use util::paths::PathMatcher;

/// 适用于任意代码库大小的快速文件路径模式匹配工具
///
/// - 支持 glob 模式，如 "**/*.js" 或 "src/**/*.ts"
/// - 返回按字母顺序排序的匹配文件路径
/// - 搜索符号时优先使用 grep 工具，除非你有明确的路径信息
/// - 需要按名称模式查找文件时使用此工具
/// - 结果分页显示，每页 50 条，使用可选的 offset 参数请求后续页面
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct FindPathToolInput {
    /// 用于匹配项目中所有路径的 glob 表达式
    ///
    /// <example>
    /// 如果项目包含以下根目录：
    ///
    /// - directory1/a/something.txt
    /// - directory2/a/things.txt
    /// - directory3/a/other.txt
    ///
    /// 提供 "*thing*.txt" 即可获取前两个路径
    /// </example>
    pub glob: String,
    /// 分页结果的起始位置（从 0 开始），未提供时从头开始
    #[serde(default)]
    pub offset: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FindPathToolOutput {
    Success {
        offset: usize,
        current_matches_page: Vec<PathBuf>,
        all_matches_len: usize,
    },
    Error {
        error: String,
    },
}

impl From<FindPathToolOutput> for LanguageModelToolResultContent {
    fn from(output: FindPathToolOutput) -> Self {
        match output {
            FindPathToolOutput::Success {
                offset,
                current_matches_page,
                all_matches_len,
            } => {
                if current_matches_page.is_empty() {
                    "未找到匹配项".into()
                } else {
                    let mut llm_output = format!("共找到 {} 个匹配项。", all_matches_len);
                    if all_matches_len > RESULTS_PER_PAGE {
                        write!(
                            &mut llm_output,
                            "\n显示结果 {}-{}（提供 'offset' 参数查看更多结果）：",
                            offset + 1,
                            offset + current_matches_page.len()
                        )
                        .ok();
                    }

                    for mat in current_matches_page {
                        write!(&mut llm_output, "\n{}", mat.display()).ok();
                    }

                    llm_output.into()
                }
            }
            FindPathToolOutput::Error { error } => error.into(),
        }
    }
}

/// 每页结果数量
const RESULTS_PER_PAGE: usize = 50;

pub struct FindPathTool {
    project: Entity<Project>,
}

impl FindPathTool {
    /// 创建路径查找工具实例
    pub fn new(project: Entity<Project>) -> Self {
        Self { project }
    }
}

impl AgentTool for FindPathTool {
    type Input = FindPathToolInput;
    type Output = FindPathToolOutput;

    const NAME: &'static str = "find_path";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Search
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        let mut title = "查找路径".to_string();
        if let Ok(input) = input {
            title.push_str(&format!(" 匹配 “`{}`”", input.glob));
        }
        title.into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        let project = self.project.clone();
        cx.spawn(async move |cx| {
            let input = input.recv().await.map_err(|e| FindPathToolOutput::Error {
                error: format!("接收工具输入失败：{e}"),
            })?;

            let search_paths_task = cx.update(|cx| search_paths(&input.glob, project, cx));

            let matches = futures::select! {
                result = search_paths_task.fuse() => result.map_err(|e| FindPathToolOutput::Error { error: e.to_string() })?,
                _ = event_stream.cancelled_by_user().fuse() => {
                    return Err(FindPathToolOutput::Error { error: "路径搜索已被用户取消".to_string() });
                }
            };
            let paginated_matches: &[PathBuf] = &matches[cmp::min(input.offset, matches.len())
                ..cmp::min(input.offset + RESULTS_PER_PAGE, matches.len())];

            event_stream.update_fields(
                acp::ToolCallUpdateFields::new()
                    .title(if paginated_matches.is_empty() {
                        "无匹配项".into()
                    } else if paginated_matches.len() == 1 {
                        "1 个匹配项".into()
                    } else {
                        format!("{} 个匹配项", paginated_matches.len())
                    })
                    .content(
                        paginated_matches
                            .iter()
                            .map(|path| {
                                acp::ToolCallContent::Content(acp::Content::new(
                                    acp::ContentBlock::ResourceLink(acp::ResourceLink::new(
                                        path.to_string_lossy(),
                                        format!("file://{}", path.display()),
                                    )),
                                ))
                            })
                            .collect::<Vec<_>>(),
                    ),
            );

            Ok(FindPathToolOutput::Success {
                offset: input.offset,
                current_matches_page: paginated_matches.to_vec(),
                all_matches_len: matches.len(),
            })
        })
    }
}

/// 根据 glob 模式搜索项目中的文件路径
fn search_paths(glob: &str, project: Entity<Project>, cx: &mut App) -> Task<Result<Vec<PathBuf>>> {
    let path_style = project.read(cx).path_style(cx);
    let path_matcher = match PathMatcher::new(
        [
            // 模型有时会尝试搜索空字符串，此时返回项目中所有路径
            if glob.is_empty() { "*" } else { glob },
        ],
        path_style,
    ) {
        Ok(matcher) => matcher,
        Err(err) => return Task::ready(Err(anyhow!("无效的 glob 表达式：{err}"))),
    };
    let snapshots: Vec<_> = project
        .read(cx)
        .worktrees(cx)
        .map(|worktree| worktree.read(cx).snapshot())
        .collect();

    cx.background_spawn(async move {
        let mut results = Vec::new();
        for snapshot in snapshots {
            for entry in snapshot.entries(false, 0) {
                if path_matcher.is_match(&snapshot.root_name().join(&entry.path)) {
                    results.push(snapshot.absolutize(&entry.path));
                }
            }
        }

        Ok(results)
    })
}

#[cfg(test)]
mod test {
    use super::*;
    use gpui::TestAppContext;
    use project::{FakeFs, Project};
    use settings::SettingsStore;
    use util::path;

    #[gpui::test]
    async fn test_find_path_tool(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            "/root",
            serde_json::json!({
                "apple": {
                    "banana": {
                        "carrot": "1",
                    },
                    "bandana": {
                        "carbonara": "2",
                    },
                    "endive": "3"
                }
            }),
        )
        .await;
        let project = Project::test(fs.clone(), [path!("/root").as_ref()], cx).await;

        let matches = cx
            .update(|cx| search_paths("root/**/car*", project.clone(), cx))
            .await
            .unwrap();
        assert_eq!(
            matches,
            &[
                PathBuf::from(path!("/root/apple/banana/carrot")),
                PathBuf::from(path!("/root/apple/bandana/carbonara"))
            ]
        );

        let matches = cx
            .update(|cx| search_paths("**/car*", project.clone(), cx))
            .await
            .unwrap();
        assert_eq!(
            matches,
            &[
                PathBuf::from(path!("/root/apple/banana/carrot")),
                PathBuf::from(path!("/root/apple/bandana/carbonara"))
            ]
        );
    }

    /// 初始化测试环境
    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
        });
    }
}