//! 自定义错误类型
//!
//! 定义 relayer 可能遇到的各类错误。
//! 目前大部分代码直接使用 anyhow::Result 进行错误传播，
//! 这些错误类型作为更精确的错误分类预留。

use thiserror::Error;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum RelayerError {
    /// 链 ID 在注册表中找不到，且没有设置 RPC 环境变量覆盖
    #[error("链 {0} 不在注册表中，且未设置 RPC 覆盖")]
    UnknownChain(u64),

    /// 本机 relayer 公钥不在桥合约的白名单中
    #[error("本机 relayer 密钥不在桥合约的 relayer 白名单中")]
    NotWhitelisted,

    /// 从链上未发现任何 peer 配置
    #[error("未从链上配置中发现任何 peer")]
    NoPeers,

    /// 链上账户数据反序列化失败
    #[error("链上账户反序列化失败: {0}")]
    Deserialize(String),

    /// RPC 调用失败
    #[error("RPC 错误: {0}")]
    Rpc(String),

    /// ABI 编码/解码失败
    #[error("ABI 编码错误: {0}")]
    Abi(String),
}
