mod bitbucket;
mod github;
mod gitlab;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use bitbucket::BitbucketClient;
pub use github::GitHubClient;
pub use gitlab::GitLabClient;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub body: String,
    pub author: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_resolved: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAction {
    Approve,
    RequestChanges,
    Comment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub action: ReviewAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<ReviewComment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub path: String,
    pub line: u64,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadComment {
    pub author: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThread {
    pub id: String,
    pub path: String,
    pub line: u64,
    pub is_resolved: bool,
    pub comments: Vec<ThreadComment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoContext {
    pub owner: String,
    pub repo: String,
}

impl RepoContext {
    pub fn new(owner: impl Into<String>, repo: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
        }
    }

    pub fn project_path(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

#[async_trait]
pub trait PullRequestClient: Send + Sync {
    async fn get_comments(&self) -> Result<Vec<Comment>>;
    async fn get_comment(&self, comment_id: &str) -> Result<Comment>;
    async fn add_comment(&self, body: &str) -> Result<Comment>;
    async fn delete_comment(&self, comment_id: &str) -> Result<()>;
    async fn resolve_comment(&self, comment_id: &str) -> Result<()>;
    async fn submit_review(&self, review: &Review) -> Result<()>;
    async fn update_description(&self, description: &str) -> Result<()>;
    async fn update_title(&self, title: &str) -> Result<()>;
    async fn get_review_threads(&self) -> Result<Vec<ReviewThread>>;
}

pub trait Client {
    fn pull_request(&self, id: u64) -> Box<dyn PullRequestClient>;

    fn pull_request_from_url(&self, url: &str) -> Result<Box<dyn PullRequestClient>>;
}
