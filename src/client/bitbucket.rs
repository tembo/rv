use std::collections::HashMap;

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

const API_BASE: &str = "https://api.bitbucket.org/2.0";

#[derive(Debug, Clone)]
pub struct BitbucketClient {
    http: reqwest::Client,
    token: String,
    context: RepoContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BitbucketUser {
    pub display_name: String,
    pub username: Option<String>,
    pub uuid: String,
}

impl BitbucketClient {
    pub fn new(token: impl Into<String>, context: RepoContext) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("rv/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build Bitbucket client")?;
        Ok(Self {
            http,
            token: token.into(),
            context,
        })
    }

    pub async fn authenticated_user(&self) -> Result<BitbucketUser> {
        let response = self.request(Method::GET, "/user").send().await?;
        response_json(response).await
    }

    pub async fn repositories(&self, limit: u32) -> Result<Value> {
        let response = self
            .request(Method::GET, "/repositories")
            .query(&[
                ("role", "member".to_owned()),
                ("pagelen", limit.clamp(1, 100).to_string()),
            ])
            .send()
            .await?;
        response_json(response).await
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let request = self.http.request(method, format!("{API_BASE}{path}"));
        if let Some((username, password)) = self.token.split_once(':') {
            request.basic_auth(username, Some(password))
        } else {
            request.bearer_auth(&self.token)
        }
    }
}

impl Client for BitbucketClient {
    fn pull_request(&self, id: u64) -> Box<dyn PullRequestClient> {
        Box::new(BitbucketPullRequest {
            client: self.clone(),
            context: self.context.clone(),
            id,
        })
    }

    fn pull_request_from_url(&self, url: &str) -> Result<Box<dyn PullRequestClient>> {
        let parsed = parse_pull_request_url(url)?;
        if parsed.provider != Provider::Bitbucket {
            bail!("expected a Bitbucket pull request URL");
        }

        Ok(Box::new(BitbucketPullRequest {
            client: self.clone(),
            context: RepoContext::new(parsed.owner, parsed.repo),
            id: parsed.number,
        }))
    }
}

struct BitbucketPullRequest {
    client: BitbucketClient,
    context: RepoContext,
    id: u64,
}

#[derive(Clone, Deserialize)]
struct ApiUser {
    display_name: String,
}

#[derive(Clone, Deserialize)]
struct ApiContent {
    #[serde(default)]
    raw: String,
}

#[derive(Clone, Deserialize)]
struct ApiInline {
    path: Option<String>,
    to: Option<u64>,
    from: Option<u64>,
}

#[derive(Clone, Deserialize)]
struct ApiParent {
    id: u64,
}

#[derive(Clone, Deserialize)]
struct ApiComment {
    id: u64,
    content: Option<ApiContent>,
    user: Option<ApiUser>,
    #[serde(default)]
    created_on: String,
    #[serde(default)]
    updated_on: String,
    resolution: Option<Value>,
    inline: Option<ApiInline>,
    parent: Option<ApiParent>,
}

impl ApiComment {
    fn author(&self) -> String {
        self.user
            .as_ref()
            .map(|user| user.display_name.clone())
            .unwrap_or_else(|| "unknown".to_owned())
    }

    fn body(&self) -> String {
        self.content
            .as_ref()
            .map(|content| content.raw.clone())
            .unwrap_or_default()
    }
}

impl From<ApiComment> for Comment {
    fn from(comment: ApiComment) -> Self {
        Self {
            id: comment.id.to_string(),
            body: comment.body(),
            author: comment.author(),
            created_at: comment.created_on,
            updated_at: comment.updated_on,
            is_resolved: comment.resolution.map(|_| true),
        }
    }
}

#[derive(Deserialize)]
struct CommentPage {
    #[serde(default)]
    values: Vec<ApiComment>,
    next: Option<String>,
}

impl BitbucketPullRequest {
    fn path(&self) -> String {
        format!(
            "/repositories/{}/{}/pullrequests/{}",
            urlencoding::encode(&self.context.owner),
            urlencoding::encode(&self.context.repo),
            self.id
        )
    }

    fn request(&self, method: Method, suffix: &str) -> RequestBuilder {
        self.client
            .request(method, &format!("{}{suffix}", self.path()))
    }

    async fn all_comments(&self) -> Result<Vec<ApiComment>> {
        let mut url = format!("{API_BASE}{}/comments?pagelen=100", self.path());
        let mut comments = Vec::new();
        loop {
            let request = self.client.http.get(&url);
            let request = if let Some((username, password)) = self.client.token.split_once(':') {
                request.basic_auth(username, Some(password))
            } else {
                request.bearer_auth(&self.client.token)
            };
            let page: CommentPage = response_json(request.send().await?).await?;
            comments.extend(page.values);
            let Some(next) = page.next else {
                break;
            };
            url = next;
        }
        Ok(comments)
    }

    async fn add_plain_comment(&self, body: &str) -> Result<()> {
        let response = self
            .request(Method::POST, "/comments")
            .json(&json!({ "content": { "raw": body } }))
            .send()
            .await?;
        response_empty(response).await
    }
}

#[async_trait]
impl PullRequestClient for BitbucketPullRequest {
    async fn get_comments(&self) -> Result<Vec<Comment>> {
        Ok(self
            .all_comments()
            .await?
            .into_iter()
            .map(Comment::from)
            .collect())
    }

    async fn get_comment(&self, comment_id: &str) -> Result<Comment> {
        let response = self
            .request(Method::GET, &format!("/comments/{comment_id}"))
            .send()
            .await?;
        Ok(Comment::from(response_json::<ApiComment>(response).await?))
    }

    async fn add_comment(&self, body: &str) -> Result<Comment> {
        let response = self
            .request(Method::POST, "/comments")
            .json(&json!({ "content": { "raw": body } }))
            .send()
            .await?;
        Ok(Comment::from(response_json::<ApiComment>(response).await?))
    }

    async fn delete_comment(&self, comment_id: &str) -> Result<()> {
        let response = self
            .request(Method::DELETE, &format!("/comments/{comment_id}"))
            .send()
            .await?;
        response_empty(response).await
    }

    async fn resolve_comment(&self, comment_id: &str) -> Result<()> {
        let response = self
            .request(Method::PUT, &format!("/comments/{comment_id}/resolve"))
            .send()
            .await?;
        response_empty(response).await
    }

    async fn submit_review(&self, review: &Review) -> Result<()> {
        let action = match review.action {
            ReviewAction::Approve => Some("/approve"),
            ReviewAction::RequestChanges => Some("/request-changes"),
            ReviewAction::Comment => None,
        };
        if let Some(path) = action {
            let response = self.request(Method::POST, path).send().await?;
            response_empty(response).await?;
        }

        if let Some(body) = review.body.as_deref() {
            self.add_plain_comment(body).await?;
        }
        for comment in &review.comments {
            let response = self
                .request(Method::POST, "/comments")
                .json(&json!({
                    "content": { "raw": comment.body },
                    "inline": { "path": comment.path, "to": comment.line },
                }))
                .send()
                .await?;
            response_empty(response).await?;
        }
        Ok(())
    }

    async fn update_description(&self, description: &str) -> Result<()> {
        let response = self
            .request(Method::PUT, "")
            .json(&json!({ "summary": { "raw": description } }))
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
        let comments = self.all_comments().await?;
        let mut replies: HashMap<u64, Vec<ApiComment>> = HashMap::new();
        for comment in &comments {
            if let Some(parent) = &comment.parent {
                replies.entry(parent.id).or_default().push(comment.clone());
            }
        }

        Ok(comments
            .into_iter()
            .filter(|comment| comment.inline.is_some() && comment.parent.is_none())
            .map(|root| {
                let inline = root.inline.as_ref().expect("inline comment");
                let mut thread_comments = vec![ThreadComment {
                    author: root.author(),
                    body: root.body(),
                    created_at: root.created_on.clone(),
                }];
                thread_comments.extend(
                    replies
                        .remove(&root.id)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|reply| ThreadComment {
                            author: reply.author(),
                            body: reply.body(),
                            created_at: reply.created_on,
                        }),
                );

                ReviewThread {
                    id: root.id.to_string(),
                    path: inline.path.clone().unwrap_or_default(),
                    line: inline.to.or(inline.from).unwrap_or_default(),
                    is_resolved: root.resolution.is_some(),
                    comments: thread_comments,
                }
            })
            .collect())
    }
}
