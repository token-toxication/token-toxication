use std::{net::SocketAddr, path::PathBuf};

use clap::Args;

#[derive(Debug, Clone, Args)]
pub struct Config {
    #[arg(long, env = "TT_BIND_ADDR", default_value = "0.0.0.0:3000")]
    pub bind_addr: SocketAddr,

    #[arg(
        long,
        env = "TT_DATABASE_PATH",
        default_value = "data/token-toxication.sqlite3"
    )]
    pub database_path: PathBuf,

    #[arg(long, env = "TT_STATIC_DIR", default_value = "apps/admin/dist")]
    pub static_dir: PathBuf,

    #[arg(long, env = "TT_ADMIN_USERNAME", default_value = "admin")]
    pub admin_username: String,

    #[arg(
        long,
        env = "TT_ADMIN_PASSWORD",
        default_value = "change-this-password"
    )]
    pub admin_password: String,

    #[arg(long, env = "TT_API_KEY_PREFIX", default_value = "tokentoxication-")]
    pub api_key_prefix: String,

    #[arg(
        long,
        env = "TT_ALLOW_DEFAULT_ADMIN_PASSWORD",
        default_value_t = false,
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    pub allow_default_admin_password: bool,
}

impl Config {
    pub fn warn_if_default_admin_password(&self) {
        if self.admin_password == "change-this-password" && !self.allow_default_admin_password {
            tracing::warn!(
                "using default admin password; set TT_ADMIN_PASSWORD before exposing this service"
            );
        }
    }
}
