use std::{num::NonZeroU32, path::Path};

use collections::{HashMap, HashSet};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::Error as _};
use settings_macros::{MergeFrom, with_fallible_options};
use std::sync::Arc;

use crate::{DocumentFoldingRanges, DocumentSymbols, ExtendingVec, SemanticTokens, merge_from};

/// 在某个时间点修饰键的状态
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct ModifiersContent {
    /// control 键
    #[serde(default)]
    pub control: bool,
    /// alt 键
    /// 有时也称为 'meta' 键
    #[serde(default)]
    pub alt: bool,
    /// shift 键
    #[serde(default)]
    pub shift: bool,
    /// 在 macOS 上为 command 键，
    /// 在 Windows 上为 Windows 键，
    /// 在 Linux 上为 super 键
    #[serde(default)]
    pub platform: bool,
    /// 功能键
    #[serde(default)]
    pub function: bool,
}

#[with_fallible_options]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AllLanguageSettingsContent {
    /// 编辑预测设置。
    pub edit_predictions: Option<EditPredictionSettingsContent>,
    /// 默认语言设置。
    #[serde(flatten)]
    pub defaults: LanguageSettingsContent,
    /// 各个语言的设置。
    #[serde(default)]
    pub languages: LanguageToSettingsMap,
    /// 将文件扩展名和文件名与语言关联起来的设置。
    pub file_types: Option<HashMap<Arc<str>, ExtendingVec<String>>>,
}

impl merge_from::MergeFrom for AllLanguageSettingsContent {
    fn merge_from(&mut self, other: &Self) {
        self.file_types.merge_from(&other.file_types);
        self.edit_predictions.merge_from(&other.edit_predictions);

        // 用户的全局设置会覆盖默认全局设置以及所有默认的语言特定设置。
        //
        self.defaults.merge_from(&other.defaults);
        for language_settings in self.languages.0.values_mut() {
            language_settings.merge_from(&other.defaults);
        }

        // 用户的语言特定设置会覆盖默认的语言特定设置。
        for (language_name, user_language_settings) in &other.languages.0 {
            if let Some(existing) = self.languages.0.get_mut(language_name) {
                existing.merge_from(&user_language_settings);
            } else {
                let mut new_settings = self.defaults.clone();
                new_settings.merge_from(&user_language_settings);

                self.languages.0.insert(language_name.clone(), new_settings);
            }
        }
    }
}

/// 提供编辑预测的提供商。
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Serialize, JsonSchema, MergeFrom)]
#[serde(rename_all = "snake_case")]
pub enum EditPredictionProvider {
    None,
    #[default]
    Copilot,
    Zed,
    Codestral,
    Ollama,
    OpenAiCompatibleApi,
    Mercury,
    Experimental(&'static str),
}

const EXPERIMENTAL_ZETA2_EDIT_PREDICTION_PROVIDER_NAME: &str = "zeta2";

impl<'de> Deserialize<'de> for EditPredictionProvider {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum Content {
            None,
            Copilot,
            Zed,
            Codestral,
            Ollama,
            OpenAiCompatibleApi,
            Mercury,
            Experimental(String),
        }

        Ok(match Content::deserialize(deserializer)? {
            Content::None => EditPredictionProvider::None,
            Content::Copilot => EditPredictionProvider::Copilot,
            Content::Zed => EditPredictionProvider::Zed,
            Content::Codestral => EditPredictionProvider::Codestral,
            Content::Ollama => EditPredictionProvider::Ollama,
            Content::OpenAiCompatibleApi => EditPredictionProvider::OpenAiCompatibleApi,
            Content::Mercury => EditPredictionProvider::Mercury,
            Content::Experimental(name)
                if name == EXPERIMENTAL_ZETA2_EDIT_PREDICTION_PROVIDER_NAME =>
            {
                EditPredictionProvider::Zed
            }
            Content::Experimental(name) => {
                return Err(D::Error::custom(format!(
                    "未知的实验性编辑预测提供商: {}",
                    name
                )));
            }
        })
    }
}

impl EditPredictionProvider {
    pub fn is_zed(&self) -> bool {
        match self {
            EditPredictionProvider::Zed => true,
            EditPredictionProvider::None
            | EditPredictionProvider::Copilot
            | EditPredictionProvider::Codestral
            | EditPredictionProvider::Ollama
            | EditPredictionProvider::OpenAiCompatibleApi
            | EditPredictionProvider::Mercury
            | EditPredictionProvider::Experimental(_) => false,
        }
    }

    pub fn display_name(&self) -> Option<&'static str> {
        match self {
            EditPredictionProvider::Zed => Some("Zed AI"),
            EditPredictionProvider::Copilot => Some("GitHub Copilot"),
            EditPredictionProvider::Codestral => Some("Codestral"),
            EditPredictionProvider::Mercury => Some("Mercury"),
            EditPredictionProvider::Experimental(_) | EditPredictionProvider::None => None,
            EditPredictionProvider::Ollama => Some("Ollama"),
            EditPredictionProvider::OpenAiCompatibleApi => Some("OpenAI-Compatible API"),
        }
    }
}

/// 编辑预测设置的内容。
#[with_fallible_options]
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, MergeFrom, PartialEq)]
pub struct EditPredictionSettingsContent {
    /// 确定要使用的编辑预测提供商。
    pub provider: Option<EditPredictionProvider>,
    /// 表示应禁用编辑预测的文件的 glob 列表。
    /// 此列表添加到一个预设的、合理的默认 glob 集合之上。
    /// 您添加的任何额外项都会与它们合并。
    pub disabled_globs: Option<Vec<String>>,
    /// 在缓冲区中显示编辑预测的模式。
    /// 需要提供商支持。
    pub mode: Option<EditPredictionsMode>,
    /// GitHub Copilot 特定设置。
    pub copilot: Option<CopilotSettingsContent>,
    /// Codestral 特定设置。
    pub codestral: Option<CodestralSettingsContent>,
    /// Ollama 特定设置。
    pub ollama: Option<OllamaEditPredictionSettingsContent>,
    /// 使用自定义 OpenAI 兼容服务器进行编辑预测的特定设置。
    pub open_ai_compatible_api: Option<CustomEditPredictionProviderSettingsContent>,
    /// 存储手动捕获的编辑预测示例的目录。
    pub examples_dir: Option<Arc<Path>>,
    /// 控制 Zed 在使用其编辑预测功能时是否可以收集训练数据。
    /// 仅对检测为开源项目的文件进行数据捕获。
    ///
    /// - `"default"`：使用先前通过状态栏切换设置的偏好，
    ///   如果未存储任何偏好，则为 false。
    /// - `"yes"`：允许对开源项目中的文件收集数据。
    /// - `"no"`：永不收集数据。
    pub allow_data_collection: Option<EditPredictionDataCollectionChoice>,
}

#[with_fallible_options]
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, MergeFrom, PartialEq)]
pub struct CustomEditPredictionProviderSettingsContent {
    /// 用于补全的 API URL。
    ///
    /// 默认值: ""
    pub api_url: Option<String>,
    /// 用于补全的提示格式。设置为 `""` 将根据模型名称自动推断格式。
    ///
    /// 默认值: ""
    pub prompt_format: Option<EditPredictionPromptFormat>,
    /// 模型名称。
    ///
    /// 默认值: ""
    pub model: Option<String>,
    /// 最大生成 token 数。
    ///
    /// 默认值: 256
    pub max_output_tokens: Option<u32>,
}

#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum EditPredictionPromptFormat {
    #[default]
    Infer,
    Zeta,
    Zeta2,
    CodeLlama,
    StarCoder,
    DeepseekCoder,
    Qwen,
    CodeGemma,
    Codestral,
    Glm,
}

#[with_fallible_options]
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, MergeFrom, PartialEq)]
pub struct CopilotSettingsContent {
    /// 用于 Copilot 的 HTTP/HTTPS 代理。
    ///
    /// 默认值: 无
    pub proxy: Option<String>,
    /// 禁用代理的证书验证（不推荐）。
    ///
    /// 默认值: false
    pub proxy_no_verify: Option<bool>,
    /// Copilot 的企业 URI。
    ///
    /// 默认值: 无
    pub enterprise_uri: Option<String>,
    /// 是否启用 Copilot 的“下一个编辑建议”功能。
    ///
    /// 默认值: true
    pub enable_next_edit_suggestions: Option<bool>,
}

#[with_fallible_options]
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, MergeFrom, PartialEq)]
pub struct CodestralSettingsContent {
    /// 用于补全的模型。
    ///
    /// 默认值: "codestral-latest"
    pub model: Option<String>,
    /// 最大生成 token 数。
    ///
    /// 默认值: 150
    pub max_tokens: Option<u32>,
    /// 用于补全的 API URL。
    ///
    /// 默认值: "https://codestral.mistral.ai"
    pub api_url: Option<String>,
}

/// 用于编辑预测的 Ollama 模型名称。
#[with_fallible_options]
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, MergeFrom, PartialEq, Eq)]
#[serde(transparent)]
pub struct OllamaModelName(pub String);

impl AsRef<str> for OllamaModelName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for OllamaModelName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<OllamaModelName> for String {
    fn from(value: OllamaModelName) -> Self {
        value.0
    }
}

#[with_fallible_options]
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, MergeFrom, PartialEq)]
pub struct OllamaEditPredictionSettingsContent {
    /// 用于补全的模型。
    ///
    /// 默认值: 无
    pub model: Option<OllamaModelName>,
    /// FIM 模型的最大生成 token 数。
    ///
    /// 默认值: 256
    pub max_output_tokens: Option<u32>,
    /// 用于补全的 API URL。
    ///
    /// 默认值: "http://localhost:11434"
    pub api_url: Option<String>,

    /// 用于补全的提示格式。设置为 `""` 将根据模型名称自动推断格式。
    ///
    /// 默认值: ""
    pub prompt_format: Option<EditPredictionPromptFormat>,
}

/// 控制 Zed 在使用其编辑预测功能时是否收集训练数据。
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Eq,
    PartialEq,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum EditPredictionDataCollectionChoice {
    /// 使用先前通过状态栏切换设置的偏好，如果未存储任何偏好则为 false。
    #[default]
    Default,
    /// 允许 Zed 从开源项目收集训练数据。
    Yes,
    /// 绝不允许收集训练数据。
    No,
}

/// 编辑预测的显示模式。
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Eq,
    PartialEq,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum EditPredictionsMode {
    /// 如果提供商支持，在按住修饰键（例如 alt）时内联显示。
    /// 否则使用 eager preview 模式。
    #[serde(alias = "auto")]
    Subtle,
    /// 当没有语言服务器补全可用时内联显示。
    #[default]
    #[serde(alias = "eager_preview")]
    Eager,
}

/// 控制编辑器中的自动缩进行为。
#[derive(
    Copy,
    Clone,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum AutoIndentMode {
    /// 输入时根据语法上下文调整缩进。
    /// 使用 tree-sitter 分析代码结构并相应地缩进。
    SyntaxAware,
    /// 创建新行时保留当前行的缩进，但不根据语法上下文调整。
    PreserveIndent,
    /// 无自动缩进。新行从第 0 列开始。
    None,
}

/// 控制编辑器中的软换行行为。
#[derive(
    Copy,
    Clone,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum SoftWrap {
    /// 通常偏好单行，除非遇到超长行。
    None,
    /// 已弃用：请改为使用 None。保留此项是为了避免破坏现有用户的配置。
    /// 通常偏好单行，除非遇到超长行。
    PreferLine,
    /// 对超出编辑器宽度的行进行软换行。
    EditorWidth,
    /// 在首选行长或编辑器宽度（取较小者）处软换行。
    #[serde(alias = "preferred_line_length")]
    Bounded,
}

/// 特定语言的设置。
#[with_fallible_options]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct LanguageSettingsContent {
    /// 一个制表符应占多少列。
    ///
    /// 默认值: 4
    #[schemars(range(min = 1, max = 128))]
    pub tab_size: Option<NonZeroU32>,
    /// 是否使用制表符进行缩进，而不是多个空格。
    ///
    /// 默认值: false
    pub hard_tabs: Option<bool>,
    /// 如何对长行文本进行软换行。
    ///
    /// 默认值: none
    pub soft_wrap: Option<SoftWrap>,
    /// 在启用了软换行的缓冲区中，在何列位置进行软换行。
    ///
    /// 默认值: 80
    pub preferred_line_length: Option<u32>,
    /// 是否在编辑器中显示换行指示线。设置为 true 时，
    /// 如果 softwrap 设置为 'preferred_line_length'，将在 'preferred_line_length' 值处显示一条指示线，
    /// 并将显示由 'wrap_guides' 设置指定的任何附加指示线。
    ///
    /// 默认值: true
    pub show_wrap_guides: Option<bool>,
    /// 在编辑器中显示换行指示线的列数。
    ///
    /// 默认值: []
    pub wrap_guides: Option<Vec<usize>>,
    /// 缩进线相关设置。
    pub indent_guides: Option<IndentGuideSettingsContent>,
    /// 保存前是否对缓冲区执行格式化。
    ///
    /// 默认值: on
    pub format_on_save: Option<FormatOnSave>,
    /// 保存缓冲区前是否删除行尾的空白字符。
    ///
    /// 默认值: true
    pub remove_trailing_whitespace_on_save: Option<bool>,
    /// 保存缓冲区时是否确保末尾有一个单换行符。
    ///
    /// 默认值: true
    pub ensure_final_newline_on_save: Option<bool>,
    /// 在新建文件以及格式化和保存操作中如何处理行尾符。
    ///
    /// - `detect`：检测现有行尾符，否则使用平台默认值（Unix 为 `lf`，Windows 为 `crlf`）。
    /// - `prefer_lf`：对新建文件和无现有行尾符的文件优先使用 LF。
    /// - `prefer_crlf`：对新建文件和无现有行尾符的文件优先使用 CRLF。
    /// - `enforce_lf`：在格式化和保存时强制使用 LF。
    /// - `enforce_crlf`：在格式化和保存时强制使用 CRLF。
    ///
    /// EditorConfig 的 `end_of_line` 属性会覆盖此设置，行为类似于 `enforce_lf` 或 `enforce_crlf`。
    ///
    /// 默认值: detect
    pub line_ending: Option<LineEndingSetting>,
    /// 如何执行缓冲区格式化。
    ///
    /// 默认值: auto
    pub formatter: Option<FormatterList>,
    /// Zed 的 Prettier 集成设置。
    /// 允许启用/禁用 Prettier 格式化并配置默认 Prettier，当找不到项目级别的 Prettier 安装时使用。
    ///
    /// 默认值: off
    pub prettier: Option<PrettierSettingsContent>,
    /// 是否自动闭合 JSX 标签。
    pub jsx_tag_auto_close: Option<JsxTagAutoCloseSettingsContent>,
    /// 是否使用语言服务器提供代码智能功能。
    ///
    /// 默认值: true
    pub enable_language_server: Option<bool>,
    /// 此语言要使用（或禁用）的语言服务器列表。
    ///
    /// 该数组应包含语言服务器 ID，以及以下特殊标记：
    /// - `"!<language_server_id>"` - 带有 `!` 前缀的语言服务器 ID 将被禁用。
    /// - `"..."` - 一个占位符，表示为此语言注册的**其余**语言服务器。
    ///
    /// 默认值: ["..."]
    pub language_servers: Option<Vec<String>>,
    /// 控制如何使用来自语言服务器的语义标记进行语法高亮。
    ///
    /// 选项：
    /// - "off": 不请求语言服务器的语义标记。
    /// - "combined": 将 LSP 语义标记与 tree-sitter 高亮一起使用。
    /// - "full": 仅使用 LSP 语义标记，替换 tree-sitter 高亮。
    ///
    /// 默认值: "off"
    pub semantic_tokens: Option<SemanticTokens>,
    /// 控制是否使用语言服务器提供的折叠范围替代 tree-sitter 和基于缩进的折叠。
    ///
    /// 选项：
    /// - "off": 使用 tree-sitter 和基于缩进的折叠（默认）。
    /// - "on": 尽可能使用 LSP 折叠，当服务器未返回结果时回退到 tree-sitter 和基于缩进的折叠。
    ///
    /// 默认值: "off"
    pub document_folding_ranges: Option<DocumentFoldingRanges>,
    /// 控制用于大纲和面包屑导航的文档符号的来源。
    ///
    /// 选项：
    /// - "off": 使用 tree-sitter 查询计算文档符号（默认）。
    /// - "on": 使用语言服务器的 `textDocument/documentSymbol` LSP 响应。启用时，不再使用 tree-sitter 获取文档符号。
    ///
    /// 默认值: "off"
    pub document_symbols: Option<DocumentSymbols>,
    /// 控制 `editor::Rewrap` 操作在此语言中允许的位置。
    ///
    /// 注意：此设置在 Vim 模式下无效，因为 Vim 模式下允许在任何地方 Rewrap。
    ///
    /// 默认值: "in_comments"
    pub allow_rewrap: Option<RewrapBehavior>,
    /// 控制是否立即显示编辑预测（true）还是通过手动触发 `editor::ShowEditPrediction` 显示（false）。
    ///
    /// 默认值: true
    pub show_edit_predictions: Option<bool>,
    /// 控制在给定的语言作用域中是否显示编辑预测。
    ///
    /// 示例: ["string", "comment"]
    ///
    /// 默认值: []
    pub edit_predictions_disabled_in: Option<Vec<String>>,
    /// 是否在编辑器中显示制表符和空格。
    pub show_whitespaces: Option<ShowWhitespaceSetting>,
    /// 当 show_whitespaces 启用时，用于渲染空白字符的可见字符。
    ///
    /// 默认值: 空格为 "•"，制表符为 "→"。
    pub whitespace_map: Option<WhitespaceMapContent>,
    /// 当上一行是注释时，是否在新行开头继续注释。
    ///
    /// 默认值: true
    pub extend_comment_on_newline: Option<bool>,
    /// 按回车键时是否继续 markdown 列表。
    ///
    /// 默认值: true
    pub extend_list_on_newline: Option<bool>,
    /// 在列表标记之后按 tab 键时是否缩进列表项。
    ///
    /// 默认值: true
    pub indent_list_on_tab: Option<bool>,
    /// 行内提示相关设置。
    pub inlay_hints: Option<InlayHintSettingsContent>,
    /// 是否自动为您输入闭合字符。例如，当您输入 '(' 时，Zed 将在正确位置自动添加一个 ')'。
    ///
    /// 默认值: true
    pub use_autoclose: Option<bool>,
    /// 是否自动用字符包围选中的文本。例如，当您选择文本并输入 '(' 时，Zed 将自动用 () 包围文本。
    ///
    /// 默认值: true
    pub use_auto_surround: Option<bool>,
    /// 控制编辑器如何处理自动闭合的字符。
    /// 当设置为 `false`（默认）时，跳过和自动移除闭合字符仅适用于自动插入的字符。
    /// 否则（`true`），无论闭合字符如何插入，始终跳过和自动移除它们。
    ///
    /// 默认值: false
    pub always_treat_brackets_as_autoclosed: Option<bool>,
    /// 是否在每次输入触发器符号后使用额外的 LSP 查询来格式化（和修正）代码，由 LSP 服务器能力定义。
    ///
    /// 默认值: true
    pub use_on_type_format: Option<bool>,
    /// 在格式化之前保存时要运行的代码操作。
    /// 如果格式化关闭，这些操作不会运行。
    ///
    /// 默认值: {} （对于 Go 为 {"source.organizeImports": true}）。
    pub code_actions_on_format: Option<HashMap<String, bool>>,
    /// 如果语言服务器支持，是否对关联的范围执行链接编辑。
    /// 例如，在编辑开头 <html> 标签时，结尾 </html> 标签的内容也将被同时编辑。
    ///
    /// 默认值: true
    pub linked_edits: Option<bool>,
    /// 控制输入时的自动缩进行为。
    ///
    /// - "syntax_aware": 根据语法上下文调整缩进（默认）
    /// - "preserve_indent": 在新行上保留当前行的缩进
    /// - "none": 无自动缩进
    ///
    /// 默认值: syntax_aware
    pub auto_indent: Option<AutoIndentMode>,
    /// 是否应根据上下文调整粘贴内容的缩进。
    ///
    /// 默认值: true
    pub auto_indent_on_paste: Option<bool>,
    /// 此语言的任务配置。
    ///
    /// 默认值: {}
    pub tasks: Option<LanguageTaskSettingsContent>,
    /// 在编辑器中输入时是否自动弹出补全菜单，无需显式请求。
    ///
    /// 默认值: true
    pub show_completions_on_input: Option<bool>,
    /// 是否在补全菜单中显示项目的内联和旁侧文档。
    ///
    /// 默认值: true
    pub show_completion_documentation: Option<bool>,
    /// 控制此语言的补全处理方式。
    pub completions: Option<CompletionSettingsContent>,
    /// 此语言的首选调试器。
    ///
    /// 默认值: []
    pub debuggers: Option<Vec<String>>,
    /// 是否在编辑器中启用单词差异高亮。
    ///
    /// 启用后，修改行内发生变化的单词会被高亮显示，以显示具体更改。
    ///
    /// 默认值: true
    pub word_diff_enabled: Option<bool>,
    /// 是否使用 tree-sitter 括号查询来检测并为编辑器中的括号着色。
    ///
    /// 默认值: false
    pub colorize_brackets: Option<bool>,
}

/// 控制编辑器中空白字符的显示方式。
#[derive(
    Copy,
    Clone,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ShowWhitespaceSetting {
    /// 仅对选中的文本绘制空白字符。
    Selection,
    /// 不绘制任何制表符或空格。
    None,
    /// 绘制所有不可见符号。
    All,
    /// 仅绘制边界处的空白字符。
    ///
    /// 要成为边界空白字符，需满足以下任一条件：
    /// - 它是一个制表符
    /// - 它紧邻边缘（开头或结尾）
    /// - 它紧邻一个空白字符（左侧或右侧）
    Boundary,
    /// 仅在非空白字符之后绘制空白字符。
    Trailing,
}

#[with_fallible_options]
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, MergeFrom, PartialEq)]
pub struct WhitespaceMapContent {
    pub space: Option<char>,
    pub tab: Option<char>,
}

/// `editor::Rewrap` 的行为。
#[derive(
    Debug,
    PartialEq,
    Clone,
    Copy,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum RewrapBehavior {
    /// 仅在注释内 Rewrap。
    #[default]
    InComments,
    /// 仅在当前选区内 Rewrap。
    InSelections,
    /// 允许在任何地方 Rewrap。
    Anywhere,
}

#[with_fallible_options]
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct JsxTagAutoCloseSettingsContent {
    /// 启用或禁用 JSX 标签的自动闭合。
    pub enabled: Option<bool>,
}

/// 行内提示设置。
#[with_fallible_options]
#[derive(Clone, Default, Debug, Serialize, Deserialize, JsonSchema, MergeFrom, PartialEq, Eq)]
pub struct InlayHintSettingsContent {
    /// 全局开关，用于启用或禁用提示。
    ///
    /// 默认值: false
    pub enabled: Option<bool>,
    /// 调试时是否显示内联值的全局开关。
    ///
    /// 默认值: true
    pub show_value_hints: Option<bool>,
    /// 是否显示类型提示。
    ///
    /// 默认值: true
    pub show_type_hints: Option<bool>,
    /// 是否显示参数提示。
    ///
    /// 默认值: true
    pub show_parameter_hints: Option<bool>,
    /// 是否显示其他提示。
    ///
    /// 默认值: true
    pub show_other_hints: Option<bool>,
    /// 是否为行内提示显示背景。
    ///
    /// 如果设置为 `true`，背景将使用当前主题中的 `hint.background` 颜色。
    ///
    /// 默认值: false
    pub show_background: Option<bool>,
    /// 是否在缓冲区编辑后防抖更新行内提示。
    ///
    /// 设置为 0 以禁用防抖。
    ///
    /// 默认值: 700
    pub edit_debounce_ms: Option<u64>,
    /// 是否在缓冲区滚动后防抖更新行内提示。
    ///
    /// 设置为 0 以禁用防抖。
    ///
    /// 默认值: 50
    pub scroll_debounce_ms: Option<u64>,
    /// 当用户按下指定的修饰键时切换行内提示（隐藏或显示）。
    /// 如果仅按下指定修饰键的子集，则不切换。
    /// 如果未指定修饰键，相当于 `null`。
    ///
    /// 默认值: null
    pub toggle_on_modifiers_press: Option<ModifiersContent>,
}

/// 行内提示的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlayHintKind {
    /// 类型行内提示。
    Type,
    /// 参数行内提示。
    Parameter,
}

impl InlayHintKind {
    /// 根据给定的名称返回 [`InlayHintKind`]。
    ///
    /// 如果 `name` 与任何预期的字符串表示都不匹配，则返回 `None`。
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "type" => Some(InlayHintKind::Type),
            "parameter" => Some(InlayHintKind::Parameter),
            _ => None,
        }
    }

    /// 返回此 [`InlayHintKind`] 的名称。
    pub fn name(&self) -> &'static str {
        match self {
            InlayHintKind::Type => "type",
            InlayHintKind::Parameter => "parameter",
        }
    }
}

/// 控制此语言的补全处理方式。
#[with_fallible_options]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, MergeFrom, Default)]
#[serde(rename_all = "snake_case")]
pub struct CompletionSettingsContent {
    /// 控制如何完成单词。
    /// 对于大型文档，可能不会获取所有单词用于补全。
    ///
    /// 默认值: `fallback`
    pub words: Option<WordsCompletionMode>,
    /// 自动显示基于单词的补全所需的最小字符数。
    /// 在此数量之前，仍然可以通过相应的编辑器命令手动触发基于单词的补全。
    ///
    /// 默认值: 3
    pub words_min_length: Option<u32>,
    /// 是否获取 LSP 补全。
    ///
    /// 默认值: true
    pub lsp: Option<bool>,
    /// 在获取 LSP 补全时，确定等待特定服务器响应多长时间。
    /// 设置为 0 表示无限等待。
    ///
    /// 默认值: 0
    pub lsp_fetch_timeout_ms: Option<u64>,
    /// 控制如何插入 LSP 补全。
    ///
    /// 默认值: "replace_suffix"
    pub lsp_insert_mode: Option<LspInsertMode>,
}

#[derive(
    Copy,
    Clone,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum LspInsertMode {
    /// 替换光标前的文本，使用 LSP 规范中描述的 `insert` 范围。
    Insert,
    /// 替换光标前后的文本，使用 LSP 规范中描述的 `replace` 范围。
    Replace,
    /// 如果要替换的文本是补全文本的子序列，则行为类似于 `"replace"`，否则类似于 `"insert"`。
    ReplaceSubsequence,
    /// 如果光标后的文本是补全文本的后缀，则行为类似于 `"replace"`，否则类似于 `"insert"`。
    ReplaceSuffix,
}

/// 控制如何完成文档中的单词。
#[derive(
    Copy,
    Clone,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum WordsCompletionMode {
    /// 始终获取文档中的单词以进行补全，与 LSP 补全一同显示。
    Enabled,
    /// 仅在 LSP 响应出错或超时时，使用文档中的单词显示补全。
    Fallback,
    /// 从不获取或补全文档中的单词。
    /// （仍然可以通过单独的操作查询基于单词的补全）
    Disabled,
}

/// 允许启用/禁用 Prettier 格式化，并配置默认 Prettier，当没有找到项目级 Prettier 安装时使用。
/// Prettier 格式化默认禁用。
#[with_fallible_options]
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct PrettierSettingsContent {
    /// 对给定语言启用或禁用 Prettier 格式化。
    pub allowed: Option<bool>,

    /// 强制 Prettier 集成在格式化该语言文件时使用特定的解析器名称。
    pub parser: Option<String>,

    /// 强制 Prettier 集成在格式化该语言文件时使用特定的插件。
    /// 默认 Prettier 将安装这些插件。
    pub plugins: Option<HashSet<String>>,

    /// 默认 Prettier 选项，格式与 package.json 中 Prettier 部分的格式相同。
    /// 如果项目通过其 package.json 安装了 Prettier，这些选项将被忽略。
    #[serde(flatten)]
    pub options: Option<HashMap<String, serde_json::Value>>,
}

/// TODO: 这应该只是一个布尔值
/// 控制保存文件时的格式化行为。
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "lowercase")]
pub enum FormatOnSave {
    /// 保存时应格式化文件。
    On,
    /// 保存时不格式化文件。
    Off,
}

/// 控制保存缓冲区时如何规范换行符。
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum LineEndingSetting {
    /// 保留文件的现有行尾符。新建文件使用平台默认行尾符。
    #[strum(serialize = "Detect")]
    Detect,
    /// 对新建文件和没有现行行尾符规范的文件使用 LF，同时保留现有的 LF 或 CRLF 文件。
    #[strum(serialize = "Prefer LF")]
    PreferLf,
    /// 对新建文件和没有现行行尾符规范的文件使用 CRLF，同时保留现有的 LF 或 CRLF 文件。
    #[strum(serialize = "Prefer CRLF")]
    PreferCrlf,
    /// 在格式化和保存时将行尾符规范化为 LF（`\n`）。
    #[strum(serialize = "Enforce LF")]
    EnforceLf,
    /// 在格式化和保存时将行尾符规范化为 CRLF（`\r\n`）。
    #[strum(serialize = "Enforce CRLF")]
    EnforceCrlf,
}

/// 控制格式化代码时应使用哪些格式化器。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema, MergeFrom)]
#[serde(untagged)]
pub enum FormatterList {
    Single(Formatter),
    Vec(Vec<Formatter>),
}

impl Default for FormatterList {
    fn default() -> Self {
        Self::Single(Formatter::default())
    }
}

impl AsRef<[Formatter]> for FormatterList {
    fn as_ref(&self) -> &[Formatter] {
        match &self {
            Self::Single(single) => std::slice::from_ref(single),
            Self::Vec(v) => v,
        }
    }
}

/// 控制格式化代码时应使用哪个格式化器。如果有多个格式化器，将按声明顺序执行。
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema, MergeFrom)]
#[serde(rename_all = "snake_case")]
pub enum Formatter {
    /// 使用 Zed 的 Prettier 集成（如果适用）格式化文件，或回退到通过语言服务器格式化。
    #[default]
    Auto,
    /// 不格式化代码。
    None,
    /// 使用 Zed 的 Prettier 集成格式化代码。
    Prettier,
    /// 使用外部命令格式化代码。
    External {
        /// 要运行的外部程序。
        command: String,
        /// 要传递给程序的参数。
        arguments: Option<Vec<String>>,
    },
    /// 使用语言服务器执行的代码操作格式化文件。
    CodeAction(String),
    /// 使用语言服务器格式化代码。
    #[serde(untagged)]
    LanguageServer(LanguageServerFormatterSpecifier),
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema, MergeFrom)]
#[serde(
    rename_all = "snake_case",
    // 允许将语言服务器指定为 "language_server" 或 {"language_server": {"name": ...}}
    from = "LanguageServerVariantContent",
    into = "LanguageServerVariantContent"
)]
pub enum LanguageServerFormatterSpecifier {
    Specific {
        name: String,
    },
    #[default]
    Current,
}

impl From<LanguageServerVariantContent> for LanguageServerFormatterSpecifier {
    fn from(value: LanguageServerVariantContent) -> Self {
        match value {
            LanguageServerVariantContent::Specific {
                language_server: LanguageServerSpecifierContent { name: Some(name) },
            } => Self::Specific { name },
            _ => Self::Current,
        }
    }
}

impl From<LanguageServerFormatterSpecifier> for LanguageServerVariantContent {
    fn from(value: LanguageServerFormatterSpecifier) -> Self {
        match value {
            LanguageServerFormatterSpecifier::Specific { name } => Self::Specific {
                language_server: LanguageServerSpecifierContent { name: Some(name) },
            },
            LanguageServerFormatterSpecifier::Current => {
                Self::Current(CurrentLanguageServerContent::LanguageServer)
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema, MergeFrom)]
#[serde(rename_all = "snake_case", untagged)]
enum LanguageServerVariantContent {
    /// 使用特定的语言服务器格式化代码。
    Specific {
        language_server: LanguageServerSpecifierContent,
    },
    /// 使用当前的语言服务器格式化代码。
    Current(CurrentLanguageServerContent),
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema, MergeFrom)]
#[serde(rename_all = "snake_case")]
enum CurrentLanguageServerContent {
    #[default]
    LanguageServer,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema, MergeFrom)]
#[serde(rename_all = "snake_case")]
struct LanguageServerSpecifierContent {
    /// 用于格式化的语言服务器名称
    name: Option<String>,
}

/// 缩进线设置。
#[with_fallible_options]
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct IndentGuideSettingsContent {
    /// 是否在编辑器中显示缩进线。
    ///
    /// 默认值: true
    pub enabled: Option<bool>,
    /// 缩进线的宽度（像素），介于 1 到 10 之间。
    ///
    /// 默认值: 1
    pub line_width: Option<u32>,
    /// 活动缩进线的宽度（像素），介于 1 到 10 之间。
    ///
    /// 默认值: 1
    pub active_line_width: Option<u32>,
    /// 确定缩进线的着色方式。
    ///
    /// 默认值: Fixed
    pub coloring: Option<IndentGuideColoring>,
    /// 确定缩进线背景的着色方式。
    ///
    /// 默认值: Disabled
    pub background_coloring: Option<IndentGuideBackgroundColoring>,
}

/// 特定语言的任务设置。
#[with_fallible_options]
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize, JsonSchema, MergeFrom)]
pub struct LanguageTaskSettingsContent {
    /// 为特定语言设置的额外任务变量。
    pub variables: Option<HashMap<String, String>>,
    pub enabled: Option<bool>,
    /// 优先使用 LSP 任务而非 Zed 语言扩展任务。
    /// 如果由于错误/超时或正常执行未返回 LSP 任务，则将使用 Zed 语言扩展任务代替。
    ///
    /// 其他 Zed 任务仍会显示：
    /// * 来自任一任务配置文件的 Zed 任务
    /// * 来自历史记录的 Zed 任务（例如之前运行过的一次性任务）
    pub prefer_lsp: Option<bool>,
}

/// 从语言名称到设置的映射。
#[with_fallible_options]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct LanguageToSettingsMap(pub HashMap<String, LanguageSettingsContent>);

/// 确定缩进线的着色方式。
#[derive(
    Default,
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum IndentGuideColoring {
    /// 不为缩进线渲染任何线条。
    Disabled,
    /// 对所有缩进级别使用相同的颜色。
    #[default]
    Fixed,
    /// 对每个缩进级别使用不同的颜色。
    IndentAware,
}

/// 确定缩进线背景的着色方式。
#[derive(
    Default,
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum IndentGuideBackgroundColoring {
    /// 不为缩进线渲染任何背景。
    #[default]
    Disabled,
    /// 对每个缩进级别使用不同的颜色。
    IndentAware,
}

#[cfg(test)]
mod test {

    use crate::{ParseStatus, fallible_options};

    use super::*;

    #[test]
    fn test_formatter_deserialization() {
        let raw_auto = "{\"formatter\": \"auto\"}";
        let settings: LanguageSettingsContent = serde_json::from_str(raw_auto).unwrap();
        assert_eq!(
            settings.formatter,
            Some(FormatterList::Single(Formatter::Auto))
        );
        let raw_none = "{\"formatter\": \"none\"}";
        let settings: LanguageSettingsContent = serde_json::from_str(raw_none).unwrap();
        assert_eq!(
            settings.formatter,
            Some(FormatterList::Single(Formatter::None))
        );
        let raw = "{\"formatter\": \"language_server\"}";
        let settings: LanguageSettingsContent = serde_json::from_str(raw).unwrap();
        assert_eq!(
            settings.formatter,
            Some(FormatterList::Single(Formatter::LanguageServer(
                LanguageServerFormatterSpecifier::Current
            )))
        );

        let raw = "{\"formatter\": [{\"language_server\": {\"name\": null}}]}";
        let settings: LanguageSettingsContent = serde_json::from_str(raw).unwrap();
        assert_eq!(
            settings.formatter,
            Some(FormatterList::Vec(vec![Formatter::LanguageServer(
                LanguageServerFormatterSpecifier::Current
            )]))
        );
        let raw = "{\"formatter\": [{\"language_server\": {\"name\": null}}, \"language_server\", \"prettier\"]}";
        let settings: LanguageSettingsContent = serde_json::from_str(raw).unwrap();
        assert_eq!(
            settings.formatter,
            Some(FormatterList::Vec(vec![
                Formatter::LanguageServer(LanguageServerFormatterSpecifier::Current),
                Formatter::LanguageServer(LanguageServerFormatterSpecifier::Current),
                Formatter::Prettier
            ]))
        );

        let raw = "{\"formatter\": [{\"language_server\": {\"name\": \"ruff\"}}, \"prettier\"]}";
        let settings: LanguageSettingsContent = serde_json::from_str(raw).unwrap();
        assert_eq!(
            settings.formatter,
            Some(FormatterList::Vec(vec![
                Formatter::LanguageServer(LanguageServerFormatterSpecifier::Specific {
                    name: "ruff".to_string()
                }),
                Formatter::Prettier
            ]))
        );

        assert_eq!(
            serde_json::to_string(&LanguageServerFormatterSpecifier::Current).unwrap(),
            "\"language_server\"",
        );
    }

    #[test]
    fn test_formatter_deserialization_invalid() {
        let raw_auto = "{\"formatter\": {}}";
        let (_, result) = fallible_options::parse_json::<LanguageSettingsContent>(raw_auto);
        assert!(matches!(result, ParseStatus::Failed { .. }));
    }

    #[test]
    fn test_prettier_options() {
        let raw_prettier = r#"{"allowed": false, "tabWidth": 4, "semi": false}"#;
        let result = serde_json::from_str::<PrettierSettingsContent>(raw_prettier)
            .expect("解析 prettier 选项失败");
        assert!(
            result
                .options
                .as_ref()
                .expect("选项被扁平化存储")
                .contains_key("semi")
        );
        assert!(
            result
                .options
                .as_ref()
                .expect("选项被扁平化存储")
                .contains_key("tabWidth")
        );
    }
}