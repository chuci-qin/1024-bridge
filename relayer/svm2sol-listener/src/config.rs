use anyhow::Result;
use shared::Config;
use std::env;

pub type ListenerConfig = Config;

pub fn load_config() -> Result<ListenerConfig> {
    env::set_var("SERVICE__NAME", "svm2sol-listener");
    let mut config = Config::load()?;
    config.service.name = "svm2sol-listener".to_string();
    Ok(config)
}
