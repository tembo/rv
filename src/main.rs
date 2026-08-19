use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use rv::client::{BitbucketClient, GitHubClient, GitLabClient};
use rv::config::{Provider, load_config, save_config};
use rv::{Client, RepoContext, parse_pull_request_url};

#[derive(Debug, Parser)]
#[command(
    name = "rv",
    version,
    about = "A code review CLI for GitHub, GitLab, and Bitbucket"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(name = "github", alias = "gh", about = "GitHub repository commands")]
    Github {
        #[command(subcommand)]
        command: GithubCommand,
    },
    #[command(name = "gitlab", alias = "gl", about = "GitLab repository commands")]
    Gitlab {
        #[command(subcommand)]
        command: GitlabCommand,
    },
    #[command(
        name = "bitbucket",
        alias = "bb",
        about = "Bitbucket repository commands"
    )]
    Bitbucket {
        #[command(subcommand)]
        command: BitbucketCommand,
    },
    #[command(about = "Set up the review state for a pull request")]
    Setup(SetupArgs),
}

#[derive(Debug, Subcommand)]
enum GithubCommand {
    #[command(about = "GitHub authentication commands")]
    Auth {
        #[command(subcommand)]
        command: GithubAuthCommand,
    },
    #[command(about = "View the review threads on a pull request")]
    View {
        #[arg(value_name = "PR_URL")]
        pr_url: String,
    },
    #[command(about = "List your GitHub repositories")]
    Repos(LimitArgs),
}

#[derive(Debug, Subcommand)]
enum GithubAuthCommand {
    #[command(about = "Authenticate with a GitHub personal access token")]
    Login {
        #[arg(short, long, value_name = "TOKEN")]
        token: String,
    },
    #[command(about = "Remove stored GitHub credentials")]
    Logout,
    #[command(about = "Check GitHub authentication status")]
    Status,
}

#[derive(Debug, Subcommand)]
enum GitlabCommand {
    #[command(about = "Authenticate with a GitLab personal access token")]
    Auth {
        #[arg(short, long, value_name = "TOKEN")]
        token: Option<String>,
    },
    #[command(about = "List your GitLab projects")]
    Repos(LimitArgs),
    #[command(about = "Clone a GitLab project")]
    Clone { project: String },
}

#[derive(Debug, Subcommand)]
enum BitbucketCommand {
    #[command(about = "Authenticate with a Bitbucket access token or username:app-password")]
    Auth {
        #[arg(short, long, value_name = "TOKEN")]
        token: Option<String>,
    },
    #[command(about = "List your Bitbucket repositories")]
    Repos(LimitArgs),
    #[command(about = "Clone a Bitbucket repository")]
    Clone { repo: String },
}

#[derive(Debug, Args)]
struct LimitArgs {
    #[arg(short, long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=100))]
    limit: u32,
}

#[derive(Debug, Args)]
struct SetupArgs {
    #[arg(short, long, value_name = "URL")]
    url: String,
    #[arg(short, long, value_name = "USERNAME")]
    bot: String,
    #[arg(short, long, value_enum)]
    provider: Provider,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Github { command } => run_github(command).await,
        Command::Gitlab { command } => run_gitlab(command).await,
        Command::Bitbucket { command } => run_bitbucket(command).await,
        Command::Setup(args) => setup(args),
    }
}

async fn run_github(command: GithubCommand) -> Result<()> {
    match command {
        GithubCommand::Auth { command } => match command {
            GithubAuthCommand::Login { token } => {
                println!("Validating token...");
                let client = github_client(&token)?;
                let user = client.authenticated_user().await.context("invalid token")?;
                let mut config = load_config()?;
                config.github.token = Some(token);
                save_config(&config)?;
                if let Some(name) = user.name {
                    println!("✓ Authenticated as {} ({name})", user.login);
                } else {
                    println!("✓ Authenticated as {}", user.login);
                }
                Ok(())
            }
            GithubAuthCommand::Logout => {
                let mut config = load_config()?;
                config.github.token = None;
                save_config(&config)?;
                println!("✓ GitHub credentials removed");
                Ok(())
            }
            GithubAuthCommand::Status => {
                let config = load_config()?;
                let Some(token) = config.github.token else {
                    println!("✗ Not authenticated with GitHub");
                    return Ok(());
                };
                let user = github_client(&token)?.authenticated_user().await?;
                if let Some(name) = user.name {
                    println!("✓ Authenticated as {} ({name})", user.login);
                } else {
                    println!("✓ Authenticated as {}", user.login);
                }
                Ok(())
            }
        },
        GithubCommand::View { pr_url } => {
            let token = provider_token(Provider::Github)?;
            let client = github_client(&token)?;
            let threads = client
                .pull_request_from_url(&pr_url)?
                .get_review_threads()
                .await?;
            println!("{}", serde_json::to_string_pretty(&threads)?);
            Ok(())
        }
        GithubCommand::Repos(args) => {
            let token = provider_token(Provider::Github)?;
            let repos = github_client(&token)?.repositories(args.limit).await?;
            println!("{}", serde_json::to_string_pretty(&repos)?);
            Ok(())
        }
    }
}

async fn run_gitlab(command: GitlabCommand) -> Result<()> {
    match command {
        GitlabCommand::Auth { token } => {
            let token = token.context("OAuth is not available yet; pass --token")?;
            println!("Validating token...");
            let client = gitlab_client(&token)?;
            let user = client.authenticated_user().await.context("invalid token")?;
            let mut config = load_config()?;
            config.gitlab.token = Some(token);
            save_config(&config)?;
            println!("✓ Authenticated as {} ({})", user.username, user.name);
            Ok(())
        }
        GitlabCommand::Repos(args) => {
            let token = provider_token(Provider::Gitlab)?;
            let projects = gitlab_client(&token)?.projects(args.limit).await?;
            println!("{}", serde_json::to_string_pretty(&projects)?);
            Ok(())
        }
        GitlabCommand::Clone { project } => {
            println!("Cloning GitLab project: {project}");
            Ok(())
        }
    }
}

async fn run_bitbucket(command: BitbucketCommand) -> Result<()> {
    match command {
        BitbucketCommand::Auth { token } => {
            let token = token.context(
                "OAuth is not available yet; pass an access token or username:app-password with --token",
            )?;
            println!("Validating token...");
            let client = bitbucket_client(&token)?;
            let user = client.authenticated_user().await.context("invalid token")?;
            let mut config = load_config()?;
            config.bitbucket.token = Some(token);
            save_config(&config)?;
            println!("✓ Authenticated as {}", user.display_name);
            Ok(())
        }
        BitbucketCommand::Repos(args) => {
            let token = provider_token(Provider::Bitbucket)?;
            let repos = bitbucket_client(&token)?.repositories(args.limit).await?;
            println!("{}", serde_json::to_string_pretty(&repos)?);
            Ok(())
        }
        BitbucketCommand::Clone { repo } => {
            println!("Cloning Bitbucket repo: {repo}");
            Ok(())
        }
    }
}

fn setup(args: SetupArgs) -> Result<()> {
    let parsed = parse_pull_request_url(&args.url)?;
    if parsed.provider != args.provider {
        bail!(
            "URL is for {}, but --provider was {}",
            parsed.provider,
            args.provider
        );
    }

    let mut config = load_config()?;
    if config.provider(args.provider).token.is_none() {
        bail!(
            "no auth token found for {}; authenticate first",
            args.provider
        );
    }
    config.state.pull_request_url = Some(args.url.clone());
    config.provider_mut(args.provider).bot_username = Some(args.bot.clone());
    save_config(&config)?;

    println!("✓ Setup complete");
    println!("  Provider: {}", args.provider);
    println!("  PR URL: {}", args.url);
    println!("  Bot username: {}", args.bot);
    Ok(())
}

fn provider_token(provider: Provider) -> Result<String> {
    load_config()?
        .provider(provider)
        .token
        .clone()
        .with_context(|| format!("no auth token found for {provider}; authenticate first"))
}

fn empty_context() -> RepoContext {
    RepoContext::new("", "")
}

fn github_client(token: &str) -> Result<GitHubClient> {
    GitHubClient::new(token, empty_context())
}

fn gitlab_client(token: &str) -> Result<GitLabClient> {
    GitLabClient::new(token, empty_context())
}

fn bitbucket_client(token: &str) -> Result<BitbucketClient> {
    BitbucketClient::new(token, empty_context())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
