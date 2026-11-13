# Security policy

Please report vulnerabilities privately so they can be fixed before disclosure.

## Reporting a vulnerability

- Use GitHub's private vulnerability reporting:
  [report a vulnerability](https://github.com/YCorpLabs/Email/security/advisories/new)
  (the repository's **Security** tab, **Report a vulnerability**); or
- email support@ycorplabs.com with "Security" in the subject.

Include the affected version or commit, your platform, steps to reproduce and
the impact you observed. Do not include real mailbox contents, passwords or
tokens; synthetic examples are enough. We'll get back to you, fix it and agree
a disclosure date with you.

## Supported versions

Security fixes go to `main` and the latest release.

## Security model

- **Mail rendering:** message HTML is sanitized and shown with JavaScript
  disabled, navigation blocked and a restrictive content security policy.
  Remote images are blocked by default; **Load images** shows them for one
  email. The macOS app also looks up BIMI sender logos.
- **Credentials:** account passwords and OAuth tokens are stored in the
  operating system's credential vault: the macOS Keychain or the Linux Secret
  Service. Account configuration files hold server names, usernames and the
  OAuth client used, including a Google desktop client's token-exchange value,
  never passwords or tokens. The local database and configuration
  directory are readable only by the current user.
- **Sign-in:** official builds connect Gmail with an app password and hide
  Microsoft accounts. Builds with their own OAuth clients sign in to Gmail and
  Microsoft through the browser with PKCE and a loopback redirect. Email
  connects directly to your mail provider.
- **Build identity:** a build without the official distribution profile uses
  its own Keychain namespace and data directory, so it cannot read an official
  installation's credentials or mail. See
  [distribution profiles](docs/distribution-profile.md).
- **MCP:** the optional MCP server is a local stdio process, not a network
  listener. It is read-only by default. Each write capability needs an explicit
  grant when the server is registered; see [docs/mcp.md](docs/mcp.md).
- **Updates:** only signed official macOS releases check for updates, through
  Sparkle with EdDSA-signed feeds and archives. Linux uses the system package
  manager.
- **No telemetry:** the app has no analytics and uploads no crash reports.
