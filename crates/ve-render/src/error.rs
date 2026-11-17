//! 错误类型。

use thiserror::Error;

/// 合成失败。
#[derive(Debug, Error)]
pub enum RenderError {
    /// 找不到可用的显卡（连软件显卡也没有）。
    #[error("找不到可用的显卡")]
    NoAdapter,
    /// 创建显卡设备或读回结果失败。
    #[error("显卡出错：{0}")]
    Device(String),
    /// 图层数据和声明的尺寸对不上，或输出尺寸无效。
    #[error("图层无效：{0}")]
    BadLayer(String),
}
