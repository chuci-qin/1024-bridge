use anyhow::Result;
use shared::Config;
use std::env;

pub type S2EListenerConfig = Config;

pub fn load_config() -> Result<S2EListenerConfig> {
    env::set_var("SERVICE__NAME", "s2e-listener");

    let mut config = Config::load()?;
    config.service.name = "s2e-listener".to_string();

    if config.api.port == 8080 {
        config.api.port = 8081;
    }

    Ok(config)
}
