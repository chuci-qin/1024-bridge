//! SVM 链交互模块（三段式流水线）
//!
//! - poller：签名枚举（`enumerate_new_signatures`）及事件提取
//!   （`fetch_and_extract_events`）的核心逻辑
//! - sig_queue：基于空文件的签名队列管理（`sigs/` 活跃 + `sigs_dead/` DLQ）
//! - submitter：在 SVM 链上提交 confirm_event 指令

pub mod poller;
pub mod sig_queue;
pub mod submitter;
