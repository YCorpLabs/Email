# Repository boundaries

Email contains the macOS/Linux clients, Rust core/service and MCP server.
Hosted Scheduling and the ycorplabs.com website are separate projects.

Email was split from a larger private development repository. The public
source starts from a fresh root without that history.

Email integrates with the scheduler through the existing hosted API and its
revocable device grant. contracts/scheduling.json pins the service
repository, public origin, Google scopes and tracking limits. It contains no
credentials or hosted implementation. Updating that boundary requires review
against the referenced Scheduling source and corresponding privacy docs.

Builds and tests do not require sibling checkouts.
