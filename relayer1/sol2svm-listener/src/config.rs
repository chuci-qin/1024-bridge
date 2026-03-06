use anyhow::Result;
use shared::Config;
use std::env;

pub type ListenerConfig = Config;

pub fn load_config() -> Result<ListenerConfig> {
    env::set_var("SERVICE__NAME", "sol2svm-listener");
    let mut config = Config::load()?;
    config.service.name = "sol2svm-listener".to_string();
    Ok(config)
}
