use std::{
    env,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub admin_token: String,
    pub metric_retention: usize,
    pub log_retention: usize,
    pub degraded_after: Duration,
    pub offline_after: Duration,
    pub housekeeping_interval: Duration,
    pub default_environment: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            bind_addr: SocketAddr::new(
                env::var("RUSTPULSE_BIND_HOST")
                    .ok()
                    .and_then(|value| value.parse::<IpAddr>().ok())
                    .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                read_env("RUSTPULSE_PORT", 8080),
            ),
            admin_token: env::var("RUSTPULSE_ADMIN_TOKEN")
                .unwrap_or_else(|_| "rustpulse-dev-token".to_string()),
            metric_retention: read_env("RUSTPULSE_METRIC_RETENTION", 20_000),
            log_retention: read_env("RUSTPULSE_LOG_RETENTION", 10_000),
            degraded_after: Duration::from_secs(read_env("RUSTPULSE_DEGRADED_AFTER_SECS", 15)),
            offline_after: Duration::from_secs(read_env("RUSTPULSE_OFFLINE_AFTER_SECS", 30)),
            housekeeping_interval: Duration::from_secs(read_env("RUSTPULSE_HOUSEKEEPING_SECS", 5)),
            default_environment: env::var("RUSTPULSE_DEFAULT_ENV")
                .unwrap_or_else(|_| "dev".to_string()),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::from_env()
    }
}

fn read_env<T>(key: &str, fallback: T) -> T
where
    T: std::str::FromStr,
{
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<T>().ok())
        .unwrap_or(fallback)
}
