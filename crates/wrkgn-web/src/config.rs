use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug)]
pub struct CacheConfig {
    pub browser: u64,
    pub edge_public: u64,
    pub edge_filtered: u64,
    pub browser_search: u64,
    pub edge_search: u64,
    pub short: u64,
    pub browser_redirect: u64,
    pub edge_redirect: u64,
    pub asset: u64,
    pub stale_revalidate: u64,
    pub stale_error: u64,
}
impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            browser: 60,
            edge_public: 604800,
            edge_filtered: 86400,
            browser_search: 15,
            edge_search: 3600,
            short: 30,
            browser_redirect: 86400,
            edge_redirect: 604800,
            asset: 31536000,
            stale_revalidate: 86400,
            stale_error: 604800,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub snapshot: PathBuf,
    pub public_origin: String,
    pub connections: usize,
    pub render_limit: usize,
    pub cache: CacheConfig,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:3000".parse().unwrap(),
            snapshot: "local-data/public-export".into(),
            public_origin: "https://wrkgn.com".into(),
            connections: 4,
            render_limit: 8,
            cache: CacheConfig::default(),
        }
    }
}
pub const HELP: &str = "Usage: wrkgn-web [--bind 127.0.0.1:3000] [--snapshot PATH] [--public-origin https://wrkgn.com]\n       wrkgn-web check <snapshot-root-or-public.sqlite>\nOptions also use WRKGN_WEB_<UPPERCASE_NAME> environment variables (hyphens become underscores).\n--connections 4 --render-limit 8\nCache seconds: --browser-ttl 60 --edge-public-ttl 604800 --edge-filtered-ttl 86400\n--browser-search-ttl 15 --edge-search-ttl 3600 --short-ttl 30\n--browser-redirect-ttl 86400 --edge-redirect-ttl 604800 --asset-ttl 31536000\n--stale-revalidate 86400 --stale-error 604800\nBuild identifier: compile-time WRKGN_BUILD_ID, default dev. SIGHUP reloads the selected snapshot.";
impl Config {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut config = Self::default();
        let options = [
            "bind",
            "snapshot",
            "public-origin",
            "connections",
            "render-limit",
            "browser-ttl",
            "edge-public-ttl",
            "edge-filtered-ttl",
            "browser-search-ttl",
            "edge-search-ttl",
            "short-ttl",
            "browser-redirect-ttl",
            "edge-redirect-ttl",
            "asset-ttl",
            "stale-revalidate",
            "stale-error",
        ];
        for name in options {
            if let Ok(value) = std::env::var(format!(
                "WRKGN_WEB_{}",
                name.replace('-', "_").to_uppercase()
            )) {
                config.set(name, &value)?;
            }
        }
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let name = arg
                .strip_prefix("--")
                .ok_or_else(|| format!("unexpected argument {arg}"))?;
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {arg}"))?;
            config.set(name, value)?;
        }
        let origin = url::Url::parse(&config.public_origin).map_err(|_| "invalid public origin")?;
        if !matches!(origin.scheme(), "http" | "https")
            || origin.host_str().is_none()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || !origin.username().is_empty()
            || origin.password().is_some()
        {
            return Err("public origin must be an http(s) origin without a path, credentials, query, or fragment".into());
        }
        config.public_origin = origin.origin().ascii_serialization();
        if config.connections == 0
            || config.connections > 32
            || config.render_limit == 0
            || config.render_limit > 1024
        {
            return Err("connections must be 1..32 and render-limit 1..1024".into());
        }
        Ok(config)
    }
    fn set(&mut self, name: &str, value: &str) -> Result<(), String> {
        let number = || {
            value
                .parse::<u64>()
                .map_err(|_| format!("invalid seconds/count for --{name}"))
        };
        match name {
            "bind" => self.bind = value.parse().map_err(|_| "invalid bind address")?,
            "snapshot" => self.snapshot = value.into(),
            "public-origin" => self.public_origin = value.into(),
            "connections" => {
                self.connections =
                    usize::try_from(number()?).map_err(|_| "connections too large")?
            }
            "render-limit" => {
                self.render_limit =
                    usize::try_from(number()?).map_err(|_| "render-limit too large")?
            }
            "browser-ttl" => self.cache.browser = number()?,
            "edge-public-ttl" => self.cache.edge_public = number()?,
            "edge-filtered-ttl" => self.cache.edge_filtered = number()?,
            "browser-search-ttl" => self.cache.browser_search = number()?,
            "edge-search-ttl" => self.cache.edge_search = number()?,
            "short-ttl" => self.cache.short = number()?,
            "browser-redirect-ttl" => self.cache.browser_redirect = number()?,
            "edge-redirect-ttl" => self.cache.edge_redirect = number()?,
            "asset-ttl" => self.cache.asset = number()?,
            "stale-revalidate" => self.cache.stale_revalidate = number()?,
            "stale-error" => self.cache.stale_error = number()?,
            _ => return Err(format!("unknown option --{name}")),
        }
        Ok(())
    }
}
