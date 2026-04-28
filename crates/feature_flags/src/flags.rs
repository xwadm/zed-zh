use crate::{EnumFeatureFlag, FeatureFlag, PresenceFlag, register_feature_flag};

pub struct NotebookFeatureFlag;

impl FeatureFlag for NotebookFeatureFlag {
    const NAME: &'static str = "笔记本功能";
    type Value = PresenceFlag;
}
register_feature_flag!(NotebookFeatureFlag);

pub struct PanicFeatureFlag;

impl FeatureFlag for PanicFeatureFlag {
    const NAME: &'static str = "崩溃诊断";
    type Value = PresenceFlag;
}
register_feature_flag!(PanicFeatureFlag);

/// 用于授权访问 ACP 测试版功能的功能标志。
///
/// 我们会将这个功能标志复用于新的测试版，因此如果当前没有使用它，请不要删除。
pub struct AcpBetaFeatureFlag;

impl FeatureFlag for AcpBetaFeatureFlag {
    const NAME: &'static str = "ACP测试版";
    type Value = PresenceFlag;
}
register_feature_flag!(AcpBetaFeatureFlag);

pub struct AgentSharingFeatureFlag;

impl FeatureFlag for AgentSharingFeatureFlag {
    const NAME: &'static str = "代理共享";
    type Value = PresenceFlag;
}
register_feature_flag!(AgentSharingFeatureFlag);

pub struct DiffReviewFeatureFlag;

impl FeatureFlag for DiffReviewFeatureFlag {
    const NAME: &'static str = "差异审查";
    type Value = PresenceFlag;

    fn enabled_for_staff() -> bool {
        false
    }
}
register_feature_flag!(DiffReviewFeatureFlag);

pub struct StreamingEditFileToolFeatureFlag;

impl FeatureFlag for StreamingEditFileToolFeatureFlag {
    const NAME: &'static str = "流式编辑文件工具";
    type Value = PresenceFlag;

    fn enabled_for_staff() -> bool {
        true
    }
}
register_feature_flag!(StreamingEditFileToolFeatureFlag);

pub struct UpdatePlanToolFeatureFlag;

impl FeatureFlag for UpdatePlanToolFeatureFlag {
    const NAME: &'static str = "更新计划工具";
    type Value = PresenceFlag;

    fn enabled_for_staff() -> bool {
        false
    }
}
register_feature_flag!(UpdatePlanToolFeatureFlag);

pub struct ProjectPanelUndoRedoFeatureFlag;

impl FeatureFlag for ProjectPanelUndoRedoFeatureFlag {
    const NAME: &'static str = "项目面板撤销重做";
    type Value = PresenceFlag;

    fn enabled_for_staff() -> bool {
        true
    }
}
register_feature_flag!(ProjectPanelUndoRedoFeatureFlag);

/// 控制代理线程工作树芯片在侧边栏中的标签显示方式。
#[derive(Clone, Copy, PartialEq, Eq, Debug, EnumFeatureFlag)]
pub enum AgentThreadWorktreeLabel {
    #[default]
    Both,
    Worktree,
    Branch,
}

pub struct AgentThreadWorktreeLabelFlag;

impl FeatureFlag for AgentThreadWorktreeLabelFlag {
    const NAME: &'static str = "代理线程工作树标签";
    type Value = AgentThreadWorktreeLabel;

    fn enabled_for_staff() -> bool {
        false
    }
}
register_feature_flag!(AgentThreadWorktreeLabelFlag);