use gpui::{IntoElement, ParentElement};
use ui::{List, ListBulletItem, prelude::*};

/// Zed AI 套餐的集中定义
pub struct PlanDefinitions;

impl PlanDefinitions {
    /// 免费套餐
    pub fn free_plan(&self) -> impl IntoElement {
        List::new()
            .child(ListBulletItem::new("2000 次可接受的编辑预测"))
            .child(ListBulletItem::new(
                "使用你自己的 AI API 密钥，无限制提问",
            ))
            .child(ListBulletItem::new("无限制使用外部智能体"))
    }

    /// 专业版试用套餐
    pub fn pro_trial(&self, period: bool) -> impl IntoElement {
        List::new()
            .child(ListBulletItem::new("Zed 智能体内含 20 美元额度的令牌"))
            .child(ListBulletItem::new("无限制编辑预测"))
            .when(period, |this| {
                this.child(ListBulletItem::new(
                    "免费试用 14 天，无需信用卡",
                ))
            })
    }

    /// 专业版套餐
    pub fn pro_plan(&self) -> impl IntoElement {
        List::new()
            .child(ListBulletItem::new("Zed 智能体内含 5 美元额度的令牌"))
            .child(ListBulletItem::new("超出 5 美元后按使用量计费"))
            .child(ListBulletItem::new("无限制编辑预测"))
    }

    /// 企业版套餐
    pub fn business_plan(&self) -> impl IntoElement {
        List::new()
            .child(ListBulletItem::new("无限制编辑预测"))
            .child(ListBulletItem::new("按使用量计费"))
    }

    /// 学生版套餐
    pub fn student_plan(&self) -> impl IntoElement {
        List::new()
            .child(ListBulletItem::new("无限制编辑预测"))
            .child(ListBulletItem::new("Zed 智能体内含 10 美元额度的令牌"))
            .child(ListBulletItem::new(
                "可选购额度包用于额外使用",
            ))
    }
}