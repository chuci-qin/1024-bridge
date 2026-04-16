//! EVM 链交互模块
//!
//! - poller：轮询 EVM 链上的 StakeEvent 日志
//! - submitter：在 EVM 链上提交 confirmEvent 交易

pub mod poller;
pub mod submitter;
