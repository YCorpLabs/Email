# Delete your data

Email keeps your mail on your computer, not on our servers. This page explains
how to remove everything Email stores, how to revoke its access at Google and
Microsoft, and how to ask us to delete data held by the hosted Scheduling
service.

## 1. Remove accounts in the app

For each account, open **Manage accounts** in Email and remove it (on macOS
**Remove an account…**, on Linux **Remove account**). This deletes that
account's downloaded mail, drafts, queued mail, snoozes, reminders, signatures,
cached calendar events and open-tracking records, and removes its password or
token from your system's credential store. For an account added with Google
sign-in, Email also asks Google to revoke its access first. If that fails, for
example when you're offline, the account is still removed; check section 3.
Removing other accounts does not revoke access at your provider; see section 3.

## 2. Delete everything on this computer

Quit Email first.

### macOS

1. Delete the data folder `~/Library/Application Support/com.ycorplabs.mail/`. In Finder,
   choose **Go › Go to Folder…** and paste the path. It contains the mail cache
   (`mail.sqlite` and its `-wal` and `-shm` files), account settings
   (`mail.accounts.json`) and custom sender pictures (`Portraits`).
2. Remove credentials from the Keychain. Open **Keychain Access**, search for
   `com.ycorplabs.mail`, and delete every item whose name starts with it
   (`com.ycorplabs.mail.v4`, and `.v3`, `.v2` or `com.ycorplabs.mail` if
   present). In Terminal,
   `security delete-generic-password -s com.ycorplabs.mail.v4` deletes one item
   at a time; repeat it, and with `.v3`, `.v2` and no suffix, until it reports
   that no item was found.
3. Optional: remove app preferences such as window size and the image setting
   with `defaults delete com.ycorplabs.mail`.
4. If you installed the scheduled-send helper, remove it:
   `launchctl bootout gui/$(id -u)/com.ycorplabs.mail.dispatch` and delete
   `~/Library/LaunchAgents/com.ycorplabs.mail.dispatch.plist`.
5. Move the Email app to the Trash.

### Linux

1. Delete the data folder `${XDG_DATA_HOME:-$HOME/.local/share}/com.ycorplabs.Mail/`
   (mail cache, account settings, sender pictures and preferences).
2. Remove credentials from the Secret Service keyring (GNOME Keyring, KWallet
   and others):
   ```sh
   secret-tool search --all service com.ycorplabs.mail.v2   # review first
   secret-tool clear service com.ycorplabs.mail.v2
   secret-tool clear service com.ycorplabs.mail
   ```
   You can also delete them in your desktop's password manager (for example
   **Passwords and Keys**).
3. If you installed the scheduled-send timer, remove it:
   `systemctl --user disable --now com.ycorplabs.Mail.dispatch.timer`, then delete
   `com.ycorplabs.Mail.dispatch.service` and `com.ycorplabs.Mail.dispatch.timer` from
   `${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/`.
4. Uninstall the package (`sudo apt remove email-linux` or
   `sudo pacman -R email-linux`).

### AI assistants

If you connected an AI assistant to Email's local MCP server, remove the
`email` server from that assistant's settings. Data the assistant already
received is handled under your agreement with its provider.

### Deleted data and disk encryption

Deleting the folder removes the files, but your disk may keep traces until the
space is reused. Disk encryption (FileVault on macOS, LUKS on Linux) protects
those traces and your mail while Email is installed; we recommend turning it on.

## 3. Revoke access at your mail provider

| Account | How to revoke |
| --- | --- |
| Gmail with an app password | Remove the app password at [myaccount.google.com/apppasswords](https://myaccount.google.com/apppasswords). Changing your Google password also revokes every app password. |
| Gmail with Google sign-in | Removing the account in Email does this for you. To check, or if you removed it while offline, open [myaccount.google.com/connections](https://myaccount.google.com/connections), select **Email by YCorp** and remove its access. This also covers Google Calendar. |
| Outlook or Hotmail (personal Microsoft account) | Open [account.live.com/consent/Manage](https://account.live.com/consent/Manage) and remove Email. |
| Microsoft 365 work or school account | Ask your IT administrator; access granted for your organization is managed in the Microsoft Entra admin center. |
| iCloud | At [account.apple.com](https://account.apple.com), open **Sign-In and Security › App-Specific Passwords** and revoke the one you created for Email. |
| Other IMAP providers | Revoke the app password in your provider's account settings. |

Except for Google sign-in, removing an account in Email does not do this for you.

## 4. Hosted Scheduling data

If you used Scheduling (`meet.ycorplabs.com`), we hold your host profile,
settings, bookings, booking statistics and an encrypted Google Calendar
authorization. To delete them:

1. In Scheduling, disconnect your calendar. This deletes the stored calendar
   authorization. It isn't possible while bookings are upcoming or in progress.
2. Revoke Scheduling's access at
   [myaccount.google.com/connections](https://myaccount.google.com/connections)
   (**Scheduling by YCorp**).
3. Email **support@ycorplabs.com** from the address of your Google account and
   ask us to delete your Scheduling account. We confirm the request with you,
   then delete your host profile, settings, bookings and statistics.

Some data expires on its own: booking details 90 days after a finished
booking, open-tracking records after 90 days, and operational logs within
7 days. Our hosting provider can restore storage for up to 30 days, so deleted
data can remain recoverable for that period. Details are in the
[privacy policy](https://ycorplabs.com/privacy/).

If you booked a meeting with a host, your booking is also in the host's
calendar; ask the host to remove it there.
