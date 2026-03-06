use anyhow::Result;
use shared::Config;
use std::env;

pub type SubmitterConfig = Config;

pub fn load_config() -> Result<SubmitterConfig> {
    env::set_var("SERVICE__NAME", "svm2sol-submitter");
    let mut config = Config::load()?;
    config.service.name = "svm2sol-submitter".to_string();
    if config.api.port == 8080 {
        config.api.port = 8086;
    }
    Ok(config)
}
