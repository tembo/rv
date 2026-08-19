# rv

`rv` is a Rust CLI and library for reviewing pull requests across GitHub,
GitLab, and Bitbucket.

## Build

The project requires a recent stable Rust toolchain.

```bash
cargo build --release
```

The binary will be available at `target/release/rv`. To install it from a
checkout:

```bash
cargo install --path .
```

## Authentication

Credentials are stored in `~/.rv.json`, using the same format as the original
TypeScript implementation. Set `RV_CONFIG` to use a different config path.
The config file is created with owner-only permissions on Unix.

```bash
rv github auth login --token "$GITHUB_TOKEN"
rv gitlab auth --token "$GITLAB_TOKEN"
rv bitbucket auth --token "$BITBUCKET_TOKEN"
```

Bitbucket accepts an access token or `username:app-password` for Basic
authentication.

## Commands

```text
rv github auth <login|logout|status>
rv github view <PR_URL>
rv github repos [--limit 10]

rv gitlab auth --token <TOKEN>
rv gitlab repos [--limit 10]
rv gitlab clone <PROJECT>

rv bitbucket auth --token <TOKEN>
rv bitbucket repos [--limit 10]
rv bitbucket clone <REPO>

rv setup --url <PR_URL> --bot <USERNAME> --provider <PROVIDER>
```

Provider aliases from the TypeScript CLI are preserved: `gh`, `gl`, and `bb`.

## Review API

The library exposes a common `PullRequestClient` interface for all three
providers. Each implementation supports:

- listing, fetching, adding, and deleting comments;
- resolving comments when the provider supports it;
- submitting approvals, change requests, review bodies, and inline comments;
- updating pull request titles and descriptions;
- retrieving review threads in a provider-independent format.

GitLab URLs with nested namespaces are supported, for example
`https://gitlab.com/group/subgroup/project/-/merge_requests/123`.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```
