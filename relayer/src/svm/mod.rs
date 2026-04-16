//! SVM 链交互模块
//!
//! - poller：轮询 SVM 链上的 StakeEvent 事件（从交易日志中解析）
//! - submitter：在 SVM 链上提交 confirm_event 指令

pub mod poller;
pub mod submitter;
