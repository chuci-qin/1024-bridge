use anyhow::Result;
use shared::Config;
use std::env;

pub type S2ESubmitterConfig = Config;

pub fn load_config() -> Result<S2ESubmitterConfig> {
    env::set_var("SERVICE__NAME", "s2e-submitter");

    let mut config = Config::load()?;
    config.service.name = "s2e-submitter".to_string();

    if config.api.port == 8080 {
        config.api.port = 8084;
    }

    Ok(config)
}
