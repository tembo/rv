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

const API_BASE: &str = "https://gitlab.com/api/v4";

#[derive(Debug, Clone)]
pub struct GitLabClient {
    http: reqwest::Client,
    token: String,
    context: RepoContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GitLabUser {
    pub username: String,
    pub name: String,
    pub id: u64,
}

impl GitLabClient {
    pub fn new(token: impl Into<String>, context: RepoContext) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("rv/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build GitLab client")?;
        Ok(Self {
            http,
            token: token.into(),
            context,
        })
    }

    pub async fn authenticated_user(&self) -> Result<GitLabUser> {
        let response = self.request(Method::GET, "/user").send().await?;
        response_json(response).await
    }

    pub async fn projects(&self, limit: u32) -> Result<Value> {
        let response = self
            .request(Method::GET, "/projects")
            .query(&[
                ("membership", "true".to_owned()),
                ("per_page", limit.clamp(1, 100).to_string()),
            ])
            .send()
            .await?;
        response_json(response).await
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{API_BASE}{path}"))
            .header("PRIVATE-TOKEN", &self.token)
    }
}

impl Client for GitLabClient {
    fn pull_request(&self, id: u64) -> Box<dyn PullRequestClient> {
        Box::new(GitLabPullRequest {
            client: self.clone(),
            project: self.context.project_path(),
            iid: id,
        })
    }

    fn pull_request_from_url(&self, url: &str) -> Result<Box<dyn PullRequestClient>> {
        let parsed = parse_pull_request_url(url)?;
        if parsed.provider != Provider::Gitlab {
            bail!("expected a GitLab merge request URL");
        }

        Ok(Box::new(GitLabPullRequest {
            client: self.clone(),
            project: parsed.project_path(),
            iid: parsed.number,
        }))
    }
}

struct GitLabPullRequest {
    client: GitLabClient,
    project: String,
    iid: u64,
}

#[derive(Deserialize)]
struct ApiAuthor {
    username: String,
}

#[derive(Deserialize)]
struct ApiNote {
    id: u64,
    body: String,
    author: Option<ApiAuthor>,
    created_at: String,
    updated_at: String,
    resolved: Option<bool>,
    #[serde(default)]
    resolvable: bool,
    #[serde(rename = "type")]
    note_type: Option<String>,
    position: Option<ApiPosition>,
}

impl From<ApiNote> for Comment {
    fn from(note: ApiNote) -> Self {
        Self {
            id: note.id.to_string(),
            body: note.body,
            author: note
                .author
                .map(|author| author.username)
                .unwrap_or_else(|| "unknown".to_owned()),
            created_at: note.created_at,
            updated_at: note.updated_at,
            is_resolved: note.resolved,
        }
    }
}

#[derive(Deserialize)]
struct ApiPosition {
    new_path: Option<String>,
    old_path: Option<String>,
    new_line: Option<u64>,
    old_line: Option<u64>,
}

#[derive(Deserialize)]
struct ApiDiscussion {
    id: String,
    #[serde(default)]
    individual_note: bool,
    #[serde(default)]
    notes: Vec<ApiNote>,
}

impl GitLabPullRequest {
    fn path(&self) -> String {
        format!(
            "/projects/{}/merge_requests/{}",
            urlencoding::encode(&self.project),
            self.iid
        )
    }

    fn request(&self, method: Method, suffix: &str) -> RequestBuilder {
        self.client
            .request(method, &format!("{}{suffix}", self.path()))
    }

    async fn discussions(&self) -> Result<Vec<ApiDiscussion>> {
        let response = self
            .request(Method::GET, "/discussions")
            .query(&[("per_page", 100)])
            .send()
            .await?;
        response_json(response).await
    }

    async fn add_plain_note(&self, body: &str) -> Result<()> {
        let response = self
            .request(Method::POST, "/notes")
            .json(&json!({ "body": body }))
            .send()
            .await?;
        response_empty(response).await
    }
}

#[async_trait]
impl PullRequestClient for GitLabPullRequest {
    async fn get_comments(&self) -> Result<Vec<Comment>> {
        let response = self
            .request(Method::GET, "/notes")
            .query(&[("per_page", 100)])
            .send()
            .await?;
        let notes: Vec<ApiNote> = response_json(response).await?;
        Ok(notes.into_iter().map(Comment::from).collect())
    }

    async fn get_comment(&self, comment_id: &str) -> Result<Comment> {
        let response = self
            .request(Method::GET, &format!("/notes/{comment_id}"))
            .send()
            .await?;
        Ok(Comment::from(response_json::<ApiNote>(response).await?))
    }

    async fn add_comment(&self, body: &str) -> Result<Comment> {
        let response = self
            .request(Method::POST, "/notes")
            .json(&json!({ "body": body }))
            .send()
            .await?;
        Ok(Comment::from(response_json::<ApiNote>(response).await?))
    }

    async fn delete_comment(&self, comment_id: &str) -> Result<()> {
        let response = self
            .request(Method::DELETE, &format!("/notes/{comment_id}"))
            .send()
            .await?;
        response_empty(response).await
    }

    async fn resolve_comment(&self, comment_id: &str) -> Result<()> {
        let discussions = self.discussions().await?;
        let discussion = discussions
            .iter()
            .find(|discussion| {
                discussion
                    .notes
                    .iter()
                    .any(|note| note.id.to_string() == comment_id)
            })
            .with_context(|| format!("could not find discussion for comment {comment_id}"))?;
        let response = self
            .request(
                Method::PUT,
                &format!("/discussions/{}/notes/{comment_id}", discussion.id),
            )
            .json(&json!({ "resolved": true }))
            .send()
            .await?;
        response_empty(response).await
    }

    async fn submit_review(&self, review: &Review) -> Result<()> {
        if review.action == ReviewAction::Approve {
            let response = self.request(Method::POST, "/approve").send().await?;
            response_empty(response).await?;
        }

        if let Some(body) = review.body.as_deref() {
            self.add_plain_note(body).await?;
        }
        for comment in &review.comments {
            self.add_plain_note(&format!(
                "**{}:{}**\n\n{}",
                comment.path, comment.line, comment.body
            ))
            .await?;
        }
        Ok(())
    }

    async fn update_description(&self, description: &str) -> Result<()> {
        let response = self
            .request(Method::PUT, "")
            .json(&json!({ "description": description }))
            .send()
            .await?;
        response_empty(response).await
    }

    async fn update_title(&self, title: &str) -> Result<()> {
        let response = self
            .request(Method::PUT, "")
            .json(&json!({ "title": title }))
            .send()
            .await?;
        response_empty(response).await
    }

    async fn get_review_threads(&self) -> Result<Vec<ReviewThread>> {
        let discussions = self.discussions().await?;
        Ok(discussions
            .into_iter()
            .filter_map(|discussion| {
                let first = discussion.notes.first()?;
                if discussion.individual_note || first.note_type.as_deref() != Some("DiffNote") {
                    return None;
                }
                let position = first.position.as_ref()?;
                let path = position
                    .new_path
                    .as_ref()
                    .or(position.old_path.as_ref())?
                    .clone();
                let line = position.new_line.or(position.old_line).unwrap_or_default();
                let is_resolved = discussion
                    .notes
                    .iter()
                    .all(|note| !note.resolvable || note.resolved.unwrap_or(false));
                let comments = discussion
                    .notes
                    .into_iter()
                    .map(|note| ThreadComment {
                        author: note
                            .author
                            .map(|author| author.username)
                            .unwrap_or_else(|| "unknown".to_owned()),
                        body: note.body,
                        created_at: note.created_at,
                    })
                    .collect();

                Some(ReviewThread {
                    id: discussion.id,
                    path,
                    line,
                    is_resolved,
                    comments,
                })
            })
            .collect())
    }
}
