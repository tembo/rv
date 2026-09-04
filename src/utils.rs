use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use url::Url;

use crate::config::Provider;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPullRequestUrl {
    pub provider: Provider,
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl ParsedPullRequestUrl {
    pub fn project_path(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

pub fn parse_pull_request_url(value: &str) -> Result<ParsedPullRequestUrl> {
    let url = Url::parse(value).context("invalid pull request URL")?;
    let host = url
        .host_str()
        .context("pull request URL does not include a host")?;
    let parts: Vec<_> = url
        .path_segments()
        .context("pull request URL does not include a path")?
        .filter(|part| !part.is_empty())
        .collect();

    if host.eq_ignore_ascii_case("github.com") {
        if let [owner, repo, "pull", number] = parts.as_slice() {
            return parsed(Provider::Github, owner, repo, number);
        }
        bail!("expected a GitHub URL like https://github.com/owner/repo/pull/123");
    }

    if host.eq_ignore_ascii_case("bitbucket.org") {
        if let [owner, repo, "pull-requests", number] = parts.as_slice() {
            return parsed(Provider::Bitbucket, owner, repo, number);
        }
        bail!(
            "expected a Bitbucket URL like https://bitbucket.org/workspace/repo/pull-requests/123"
        );
    }

    if let Some(marker) = parts
        .windows(2)
        .position(|pair| pair == ["-", "merge_requests"])
    {
        if marker < 2 || marker + 2 >= parts.len() || marker + 3 != parts.len() {
            bail!("invalid GitLab merge request URL");
        }

        let repo = parts[marker - 1];
        let owner = parts[..marker - 1].join("/");
        return parsed(Provider::Gitlab, &owner, repo, parts[marker + 2]);
    }

    bail!("unsupported pull request URL host: {host}")
}

fn parsed(
    provider: Provider,
    owner: &str,
    repo: &str,
    number: &str,
) -> Result<ParsedPullRequestUrl> {
    let number = number
        .parse::<u64>()
        .with_context(|| format!("invalid pull request number: {number}"))?;

    Ok(ParsedPullRequestUrl {
        provider,
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        number,
    })
}

pub(crate) async fn response_json<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("request failed with {status}: {body}");
    }

    response.json().await.context("invalid API response")
}

pub(crate) async fn response_empty(response: reqwest::Response) -> Result<()> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("request failed with {status}: {body}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_pull_request_url() {
        let parsed = parse_pull_request_url("https://github.com/tembo/rv/pull/42").unwrap();
        assert_eq!(parsed.provider, Provider::Github);
        assert_eq!(parsed.owner, "tembo");
        assert_eq!(parsed.repo, "rv");
        assert_eq!(parsed.number, 42);
    }

    #[test]
    fn parses_gitlab_nested_group_url() {
        let parsed =
            parse_pull_request_url("https://gitlab.com/frayt/platform/core/-/merge_requests/2761")
                .unwrap();
        assert_eq!(parsed.provider, Provider::Gitlab);
        assert_eq!(parsed.owner, "frayt/platform");
        assert_eq!(parsed.repo, "core");
        assert_eq!(parsed.number, 2761);
    }

    #[test]
    fn parses_bitbucket_pull_request_url() {
        let parsed =
            parse_pull_request_url("https://bitbucket.org/tembo/rv/pull-requests/7").unwrap();
        assert_eq!(parsed.provider, Provider::Bitbucket);
        assert_eq!(parsed.owner, "tembo");
        assert_eq!(parsed.repo, "rv");
        assert_eq!(parsed.number, 7);
    }

    #[test]
    fn rejects_lookalike_urls() {
        assert!(parse_pull_request_url("https://example.com/tembo/rv/pull/42").is_err());
        assert!(parse_pull_request_url("https://github.com/tembo/rv/issues/42").is_err());
    }
}
