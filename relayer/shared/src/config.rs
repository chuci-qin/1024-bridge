use crate::error::{RelayerError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_retry_delays() -> Vec<u64> {
    vec![0, 30000, 60000, 120000, 300000]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub service: ServiceConfig,
    pub source_chain: ChainConfig,
    pub target_chain: ChainConfig,
    #[serde(default)]
    pub relayer: RelayerConfig,
    #[serde(default)]
    pub queue: QueueConfig,
    #[serde(default)]
    pub gas: GasConfig,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub name: String,
    pub version: String,
    pub worker_pool_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainConfig {
    pub name: String,
    pub chain_id: u64,
    pub rpc_url: String,
    pub contract_address: String,
    pub confirmation_blocks: Option<u64>,
    pub commitment: Option<String>, // For SVM: "finalized", "confirmed", etc.
    #[serde(default)]
    pub usdc_mint: Option<String>, // USDC mint address for SVM, contract address for EVM
    #[serde(default)]
    pub ws_url: Option<String>, // WebSocket URL; auto-derived from rpc_url if not set
}

impl ChainConfig {
    /// Returns the WebSocket URL, deriving from `rpc_url` if `ws_url` is not set.
    pub fn ws_url(&self) -> String {
        if let Some(ref ws) = self.ws_url {
            return ws.clone();
        }
        if self.rpc_url.starts_with("https://") {
            self.rpc_url.replacen("https://", "wss://", 1)
        } else if self.rpc_url.starts_with("http://") {
            self.rpc_url.replacen("http://", "ws://", 1)
        } else {
            format!("wss://{}", self.rpc_url)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RelayerConfig {
    // 密钥配置
    #[serde(default)]
    pub svm_wallet_path: Option<PathBuf>,
    #[serde(default)]
    pub evm_private_key: Option<String>,
    #[serde(default)]
    pub ecdsa_private_key: Option<String>,
    #[serde(default)]
    pub ed25519_private_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueConfig {
    #[serde(default = "default_queue_path")]
    pub path: PathBuf,
    #[serde(default = "default_max_size")]
    pub max_size: usize,
    #[serde(default = "default_retry_limit")]
    pub retry_limit: u32,
    #[serde(default = "default_retry_delays")]
    pub retry_delays: Vec<u64>, // milliseconds
}

fn default_queue_path() -> PathBuf {
    PathBuf::from(".relayer/queue")
}

fn default_max_size() -> usize {
    1000
}

fn default_retry_limit() -> u32 {
    5
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            path: default_queue_path(),
            max_size: 1000,
            retry_limit: 5,
            retry_delays: default_retry_delays(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasConfig {
    #[serde(default = "default_min_svm_balance")]
    pub min_svm_balance: f64,  // SOL
    #[serde(default = "default_min_evm_balance")]
    pub min_evm_balance: f64,  // ETH
    #[serde(default = "default_balance_check_interval")]
    pub balance_check_interval: u64, // milliseconds
}

fn default_min_svm_balance() -> f64 {
    5.0
}

fn default_min_evm_balance() -> f64 {
    0.1
}

fn default_balance_check_interval() -> u64 {
    300000
}

impl Default for GasConfig {
    fn default() -> Self {
        Self {
            min_svm_balance: 5.0,
            min_evm_balance: 0.1,
            balance_check_interval: 300000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_api_port")]
    pub port: u16,
    #[serde(default)]
    pub cors_enabled: bool,
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

fn default_api_port() -> u16 {
    8080
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            cors_enabled: false,
            cors_origins: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String, // "json" or "pretty"
    #[serde(default)]
    pub log_file: Option<String>, // file path for log output, e.g. "./logs/s2e.log"
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "text".to_string()
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: "text".to_string(),
            log_file: None,
        }
    }
}

impl Config {
    /// 从环境变量和配置文件加载配置
    pub fn load() -> Result<Self> {
        // 加载 .env 文件
        dotenvy::dotenv().ok();
        
        // 暂时移除数组字段，稍后手动处理
        let cors_origins_str = std::env::var("API__CORS_ORIGINS").ok();
        let retry_delays_str = std::env::var("QUEUE__RETRY_DELAYS").ok();
        
        if cors_origins_str.is_some() {
            std::env::remove_var("API__CORS_ORIGINS");
        }
        if retry_delays_str.is_some() {
            std::env::remove_var("QUEUE__RETRY_DELAYS");
        }

        let config = config::Config::builder()
            .add_source(
                config::Environment::default()
                    .separator("__")
                    .try_parsing(true)
                    .ignore_empty(true)
            )
            .build()
            .map_err(|e| RelayerError::Config(format!("Failed to build config: {}", e)))?;

        let mut config: Config = config
            .try_deserialize()
            .map_err(|e| RelayerError::Config(format!("Failed to deserialize config: {}.\n\nMake sure all required fields are set in your .env file:\n- SOURCE_CHAIN__NAME\n- SOURCE_CHAIN__CHAIN_ID\n- SOURCE_CHAIN__RPC_URL\n- SOURCE_CHAIN__CONTRACT_ADDRESS\n- TARGET_CHAIN__NAME\n- TARGET_CHAIN__CHAIN_ID\n- TARGET_CHAIN__RPC_URL\n- TARGET_CHAIN__CONTRACT_ADDRESS", e)))?;
        
        // 手动解析数组字段
        if let Some(origins_str) = cors_origins_str {
            if !origins_str.is_empty() {
                config.api.cors_origins = origins_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .collect();
            }
        }
        
        if let Some(delays_str) = retry_delays_str {
            if !delays_str.is_empty() {
                config.queue.retry_delays = delays_str
                    .split(',')
                    .filter_map(|s| s.trim().parse::<u64>().ok())
                    .collect();
                if config.queue.retry_delays.is_empty() {
                    config.queue.retry_delays = default_retry_delays();
                }
            }
        }
        
        Ok(config)
    }

    /// 验证配置
    pub fn validate(&self) -> Result<()> {
        // 验证 RPC URLs
        if self.source_chain.rpc_url.is_empty() {
            return Err(RelayerError::Config("Source chain RPC URL is empty".to_string()));
        }
        if self.target_chain.rpc_url.is_empty() {
            return Err(RelayerError::Config("Target chain RPC URL is empty".to_string()));
        }

        // 验证合约地址
        if self.source_chain.contract_address.is_empty() {
            return Err(RelayerError::Config("Source contract address is empty".to_string()));
        }
        if self.target_chain.contract_address.is_empty() {
            return Err(RelayerError::Config("Target contract address is empty".to_string()));
        }

        // 验证密钥配置 (根据服务类型检查)
        // 这里可以添加更多验证逻辑

        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            service: ServiceConfig {
                name: "relayer".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                worker_pool_size: 5,
            },
            source_chain: ChainConfig {
                name: "Source Chain".to_string(),
                chain_id: 0,
                rpc_url: String::new(),
                contract_address: String::new(),
                confirmation_blocks: Some(12),
                commitment: None,
                usdc_mint: None,
                ws_url: None,
            },
            target_chain: ChainConfig {
                name: "Target Chain".to_string(),
                chain_id: 0,
                rpc_url: String::new(),
                contract_address: String::new(),
                confirmation_blocks: Some(12),
                commitment: None,
                usdc_mint: None,
                ws_url: None,
            },
            relayer: RelayerConfig {
                svm_wallet_path: None,
                evm_private_key: None,
                ecdsa_private_key: None,
                ed25519_private_key: None,
            },
            queue: QueueConfig {
                path: PathBuf::from(".relayer/queue"),
                max_size: 1000,
                retry_limit: 5,
                retry_delays: vec![0, 30000, 60000, 120000, 300000],
            },
            gas: GasConfig {
                min_svm_balance: 5.0,
                min_evm_balance: 0.1,
                balance_check_interval: 300000,
            },
            api: ApiConfig {
                port: 8080,
                cors_enabled: true,
                cors_origins: vec!["*".to_string()],
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                format: "json".to_string(),
                log_file: None,
            },
        }
    }
}

