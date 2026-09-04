use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Github,
    Gitlab,
    Bitbucket,
}

impl std::fmt::Display for Provider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Bitbucket => "bitbucket",
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_username: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StateConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_request_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub github: ProviderConfig,
    pub gitlab: ProviderConfig,
    pub bitbucket: ProviderConfig,
    pub state: StateConfig,
}

impl Config {
    pub fn provider(&self, provider: Provider) -> &ProviderConfig {
        match provider {
            Provider::Github => &self.github,
            Provider::Gitlab => &self.gitlab,
            Provider::Bitbucket => &self.bitbucket,
        }
    }

    pub fn provider_mut(&mut self, provider: Provider) -> &mut ProviderConfig {
        match provider {
            Provider::Github => &mut self.github,
            Provider::Gitlab => &mut self.gitlab,
            Provider::Bitbucket => &mut self.bitbucket,
        }
    }
}

pub fn config_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("RV_CONFIG") {
        return Ok(PathBuf::from(path));
    }

    dirs::home_dir()
        .map(|home| home.join(".rv.json"))
        .context("could not determine the home directory")
}

pub fn load_config() -> Result<Config> {
    load_config_from(&config_path()?)
}

pub fn save_config(config: &Config) -> Result<()> {
    save_config_to(&config_path()?, config)
}

fn load_config_from(path: &Path) -> Result<Config> {
    if !path.exists() {
        let config = Config::default();
        save_config_to(path, &config)?;
        return Ok(config);
    }

    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read config at {}", path.display()))?;
    serde_json::from_str(&contents).with_context(|| format!("invalid config at {}", path.display()))
}

fn save_config_to(path: &Path, config: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let contents = format!("{}\n", serde_json::to_string_pretty(config)?);
    write_private(path, contents.as_bytes())
        .with_context(|| format!("failed to write config at {}", path.display()))
}

#[cfg(unix)]
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)
}

#[cfg(not(unix))]
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_typescript_config_format() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        fs::write(
            &path,
            r#"{
              "github": {"token":"gh-token","botUsername":"rv-bot"},
              "gitlab": {},
              "bitbucket": {},
              "state": {"pullRequestUrl":"https://github.com/tembo/rv/pull/1"}
            }"#,
        )
        .unwrap();

        let config = load_config_from(&path).unwrap();
        assert_eq!(config.github.token.as_deref(), Some("gh-token"));
        assert_eq!(config.github.bot_username.as_deref(), Some("rv-bot"));
        assert_eq!(
            config.state.pull_request_url.as_deref(),
            Some("https://github.com/tembo/rv/pull/1")
        );
    }

    #[test]
    fn creates_a_private_default_config() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        let config = load_config_from(&path).unwrap();

        assert_eq!(config, Config::default());
        assert!(path.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn invalid_config_is_reported_instead_of_silently_discarded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        fs::write(&path, "not JSON").unwrap();

        assert!(load_config_from(&path).is_err());
    }
}
