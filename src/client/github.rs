use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::{Method, RequestBuilder};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    Client, Comment, PullRequestClient, RepoContext, Review, ReviewAction, ReviewThread,
    ThreadComment,
};
use crate::config::Provider;
use crate::utils::{parse_pull_request_url, response_empty, response_json};

const API_BASE: &str = "https://api.github.com";
const GRAPHQL_URL: &str = "https://api.github.com/graphql";

const REVIEW_THREADS_QUERY: &str = r#"
query($owner: String!, $repo: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $cursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          isResolved
          path
          line
          comments(first: 100) {
            nodes {
              body
              author { login }
              createdAt
            }
          }
        }
      }
    }
  }
}
"#;

#[derive(Debug, Clone)]
pub struct GitHubClient {
    http: reqwest::Client,
    token: String,
    context: RepoContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GitHubUser {
    pub login: String,
    pub name: Option<String>,
    pub id: u64,
}

impl GitHubClient {
    pub fn new(token: impl Into<String>, context: RepoContext) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("rv/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build GitHub client")?;
        Ok(Self {
            http,
            token: token.into(),
            context,
        })
    }

    pub async fn authenticated_user(&self) -> Result<GitHubUser> {
        let response = self.request(Method::GET, "/user").send().await?;
        response_json(response).await
    }

    pub async fn repositories(&self, limit: u32) -> Result<Value> {
        let response = self
            .request(Method::GET, "/user/repos")
            .query(&[("per_page", limit.clamp(1, 100))])
            .send()
            .await?;
        response_json(response).await
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{API_BASE}{path}"))
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }
}

impl Client for GitHubClient {
    fn pull_request(&self, id: u64) -> Box<dyn PullRequestClient> {
        Box::new(GitHubPullRequest {
            client: self.clone(),
            context: self.context.clone(),
            number: id,
        })
    }

    fn pull_request_from_url(&self, url: &str) -> Result<Box<dyn PullRequestClient>> {
        let parsed = parse_pull_request_url(url)?;
        if parsed.provider != Provider::Github {
            bail!("expected a GitHub pull request URL");
        }

        Ok(Box::new(GitHubPullRequest {
            client: self.clone(),
            context: RepoContext::new(parsed.owner, parsed.repo),
            number: parsed.number,
        }))
    }
}

struct GitHubPullRequest {
    client: GitHubClient,
    context: RepoContext,
    number: u64,
}

#[derive(Deserialize)]
struct ApiUser {
    login: String,
}

#[derive(Deserialize)]
struct ApiComment {
    id: u64,
    #[serde(default)]
    body: String,
    user: Option<ApiUser>,
    created_at: String,
    updated_at: String,
}

impl From<ApiComment> for Comment {
    fn from(comment: ApiComment) -> Self {
        Self {
            id: comment.id.to_string(),
            body: comment.body,
            author: comment
                .user
                .map(|user| user.login)
                .unwrap_or_else(|| "unknown".to_owned()),
            created_at: comment.created_at,
            updated_at: comment.updated_at,
            is_resolved: None,
        }
    }
}

impl GitHubPullRequest {
    fn repo_path(&self) -> String {
        format!("/repos/{}/{}", self.context.owner, self.context.repo)
    }

    fn request(&self, method: Method, suffix: &str) -> RequestBuilder {
        self.client
            .request(method, &format!("{}{suffix}", self.repo_path()))
    }
}

#[async_trait]
impl PullRequestClient for GitHubPullRequest {
    async fn get_comments(&self) -> Result<Vec<Comment>> {
        let response = self
            .request(Method::GET, &format!("/issues/{}/comments", self.number))
            .query(&[("per_page", 100)])
            .send()
            .await?;
        let comments: Vec<ApiComment> = response_json(response).await?;
        Ok(comments.into_iter().map(Comment::from).collect())
    }

    async fn get_comment(&self, comment_id: &str) -> Result<Comment> {
        let response = self
            .request(Method::GET, &format!("/issues/comments/{comment_id}"))
            .send()
            .await?;
        Ok(Comment::from(response_json::<ApiComment>(response).await?))
    }

    async fn add_comment(&self, body: &str) -> Result<Comment> {
        let response = self
            .request(Method::POST, &format!("/issues/{}/comments", self.number))
            .json(&json!({ "body": body }))
            .send()
            .await?;
        Ok(Comment::from(response_json::<ApiComment>(response).await?))
    }

    async fn delete_comment(&self, comment_id: &str) -> Result<()> {
        let response = self
            .request(Method::DELETE, &format!("/issues/comments/{comment_id}"))
            .send()
            .await?;
        response_empty(response).await
    }

    async fn resolve_comment(&self, _comment_id: &str) -> Result<()> {
        bail!("GitHub resolves review threads by thread ID, not comment ID")
    }

    async fn submit_review(&self, review: &Review) -> Result<()> {
        let event = match review.action {
            ReviewAction::Approve => "APPROVE",
            ReviewAction::RequestChanges => "REQUEST_CHANGES",
            ReviewAction::Comment => "COMMENT",
        };
        let comments: Vec<_> = review
            .comments
            .iter()
            .map(|comment| {
                json!({
                    "path": comment.path,
                    "line": comment.line,
                    "body": comment.body,
                })
            })
            .collect();
        let response = self
            .request(Method::POST, &format!("/pulls/{}/reviews", self.number))
            .json(&json!({
                "event": event,
                "body": review.body,
                "comments": comments,
            }))
            .send()
            .await?;
        response_empty(response).await
    }

    async fn update_description(&self, description: &str) -> Result<()> {
        let response = self
            .request(Method::PATCH, &format!("/pulls/{}", self.number))
            .json(&json!({ "body": description }))
            .send()
            .await?;
        response_empty(response).await
    }

    async fn update_title(&self, title: &str) -> Result<()> {
        let response = self
            .request(Method::PATCH, &format!("/pulls/{}", self.number))
            .json(&json!({ "title": title }))
            .send()
            .await?;
        response_empty(response).await
    }

    async fn get_review_threads(&self) -> Result<Vec<ReviewThread>> {
        let mut threads = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let response = self
                .client
                .http
                .post(GRAPHQL_URL)
                .bearer_auth(&self.client.token)
                .header("Accept", "application/vnd.github+json")
                .json(&json!({
                    "query": REVIEW_THREADS_QUERY,
                    "variables": {
                        "owner": self.context.owner,
                        "repo": self.context.repo,
                        "number": self.number,
                        "cursor": cursor,
                    }
                }))
                .send()
                .await?;
            let result: GraphQlResponse = response_json(response).await?;
            if !result.errors.is_empty() {
                bail!(
                    "GitHub GraphQL request failed: {}",
                    result.errors.join("; ")
                );
            }
            let page = result
                .data
                .context("GitHub GraphQL response did not include data")?
                .repository
                .pull_request
                .review_threads;

            threads.extend(page.nodes.into_iter().map(|thread| {
                ReviewThread {
                    id: thread.id,
                    path: thread.path,
                    line: thread.line.unwrap_or_default(),
                    is_resolved: thread.is_resolved,
                    comments: thread
                        .comments
                        .nodes
                        .into_iter()
                        .map(|comment| ThreadComment {
                            author: comment
                                .author
                                .map(|author| author.login)
                                .unwrap_or_else(|| "unknown".to_owned()),
                            body: comment.body,
                            created_at: comment.created_at,
                        })
                        .collect(),
                }
            }));

            if !page.page_info.has_next_page {
                break;
            }
            cursor = page.page_info.end_cursor;
            if cursor.is_none() {
                break;
            }
        }

        Ok(threads)
    }
}

#[derive(Deserialize)]
struct GraphQlResponse {
    data: Option<GraphQlData>,
    #[serde(default, deserialize_with = "deserialize_graphql_errors")]
    errors: Vec<String>,
}

fn deserialize_graphql_errors<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let errors = Vec::<GraphQlError>::deserialize(deserializer)?;
    Ok(errors.into_iter().map(|error| error.message).collect())
}

#[derive(Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Deserialize)]
struct GraphQlData {
    repository: GraphQlRepository,
}

#[derive(Deserialize)]
struct GraphQlRepository {
    #[serde(rename = "pullRequest")]
    pull_request: GraphQlPullRequest,
}

#[derive(Deserialize)]
struct GraphQlPullRequest {
    #[serde(rename = "reviewThreads")]
    review_threads: GraphQlThreads,
}

#[derive(Deserialize)]
struct GraphQlThreads {
    #[serde(rename = "pageInfo")]
    page_info: GraphQlPageInfo,
    nodes: Vec<GraphQlThread>,
}

#[derive(Deserialize)]
struct GraphQlPageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
struct GraphQlThread {
    id: String,
    #[serde(rename = "isResolved")]
    is_resolved: bool,
    path: String,
    line: Option<u64>,
    comments: GraphQlComments,
}

#[derive(Deserialize)]
struct GraphQlComments {
    nodes: Vec<GraphQlComment>,
}

#[derive(Deserialize)]
struct GraphQlComment {
    body: String,
    author: Option<ApiUser>,
    #[serde(rename = "createdAt")]
    created_at: String,
}
