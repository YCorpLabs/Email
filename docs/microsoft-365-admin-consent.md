# Microsoft 365: admin consent for Email

> Outlook and Microsoft 365 accounts are not offered by distribution builds
> until they have been fully tested. Development builds keep them. This page
> prepares the guidance for when they ship.

## Why a work account may say "Need admin approval"

Email signs in to Microsoft accounts with the Microsoft identity platform and
asks for these **delegated** permissions (code: `crates/mail-cloud/src/lib.rs`,
`CloudProvider::scope`):

| Permission | Why Email needs it |
| --- | --- |
| `Mail.ReadWrite` | Download mail into the local cache and apply the user's actions (read state, archive, move). |
| `Mail.Send` | Send the messages the user writes. |
| `User.Read` | Read the signed-in user's name and address to label the account. |
| `offline_access` | Keep the user signed in with a refresh token stored in the OS credential store. |

Email requests no application (app-only) permissions, no shared-mailbox
permissions and no Calendar or Contacts permissions from Microsoft.

Whether a user may grant these is decided by the tenant's
[app consent policies](https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/manage-app-consent-policies).
Microsoft's managed default for new tenants ("Let Microsoft manage your consent
settings") lets users consent to delegated permissions **except** a list that
includes `Mail.Read`, `Mail.ReadWrite`, `Mail.ReadBasic` and the shared-mailbox
variants. Many organizations go further and turn user consent off entirely. In
either case a user who signs in to Email sees an approval request instead of
the consent screen, and an administrator must grant consent once for the
organization. Microsoft's built-in mail-client exception
(`microsoft-user-default-allow-consent-apps`) covers only named clients such as
Apple Mail and Thunderbird; Email is not one of them.

## The admin-consent link

An administrator can grant tenant-wide consent from this URL
([Microsoft documentation](https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/grant-admin-consent)):

```text
https://login.microsoftonline.com/{organization}/adminconsent?client_id={client-id}
```

- `{client-id}` is Email's Microsoft **application (client) ID**, the value
  official builds embed as `EMAIL_MICROSOFT_CLIENT_ID`. It will be published here
  when the production registration exists.
- `{organization}` is the tenant ID or a verified domain of the tenant, for
  example `contoso.com`. `organizations` consents in the home tenant of the
  administrator who signs in.

Example with placeholders:
`https://login.microsoftonline.com/contoso.com/adminconsent?client_id=00000000-0000-0000-0000-000000000000`

Signing in with this link requires one of these roles:
- Privileged Role Administrator;
- Cloud Application Administrator or Application Administrator. These are enough because Email asks only for delegated Microsoft Graph permissions.

The admin can also open **Entra ID › Enterprise apps**, select Email after one
user's attempt has added it, open **Permissions** and choose **Grant admin
consent**. Tenants that use the admin consent workflow receive the user's
request there instead.

## What IT should check before approving

1. **The application.** The consent prompt names Email and its publisher; check
   that the client ID matches the one published above and, once it is complete,
   that the publisher is verified.
2. **Permissions.** Only the four delegated permissions listed above. Reject the
   request if it asks for application permissions, `Mail.ReadWrite.Shared`,
   directory access or anything else.
3. **Where data goes.** Email is a desktop client. Mail is exchanged directly
   between the user's computer and Microsoft Graph and cached only on that
   computer; Y Corp operates no server that receives mailbox data. Tokens stay in
   the macOS Keychain or the Linux Secret Service. The local cache relies on the
   device's disk encryption (FileVault or LUKS) and is not encrypted separately
   by the app. See the [privacy policy](https://ycorplabs.com/privacy/).
4. **Sign-in method.** A public client using authorization code with PKCE and a
   `http://localhost` loopback redirect. No client secret is involved.
5. **Who may use it.** Granting consent allows every user in the tenant by
   default. To limit Email to a group, set **Assignment required** on the
   enterprise app and assign users or groups.
6. **Conditional Access and device policy.** Existing policies (MFA, compliant
   device, location) apply to Email sign-ins as to any other app.
7. **Revoking.** Remove the grant under the enterprise app's **Permissions**, or
   disable or delete the enterprise app. Users' refresh tokens stop working; mail
   already cached on devices stays there until the user removes the account in
   Email (see [Delete your data](delete-your-data.md)).

## Maintainer steps before Microsoft accounts ship

1. Create the production Microsoft Entra app registration as a multitenant
   public client with the four delegated permissions and the
   `http://localhost/callback` mobile/desktop redirect
   (`crates/mail-cloud/README.md`).
2. Complete Microsoft publisher verification so the consent prompt shows a
   verified publisher.
3. Test admin consent in a test tenant with user consent disabled, then add the
   client ID to this page and finish testing Outlook and Microsoft 365 accounts.
