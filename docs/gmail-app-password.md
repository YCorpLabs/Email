# Connect Gmail with an app password

Until Google finishes reviewing Email, the released app connects Gmail with an
**app password** instead of one-click Google sign-in. It takes about two
minutes. One-click Google sign-in is coming once Google finishes reviewing the
app; accounts you connect now keep working afterwards.

## What an app password is

An app password is a 16-letter code, such as `abcd efgh ijkl mnop`, that you
create in your Google Account for one app. Email uses it to sign in to Gmail's
mail servers (`imap.gmail.com` and `smtp.gmail.com`) instead of your Google
password.

- It works only for mail over IMAP and SMTP: it can't sign in to your Google
  Account or change its settings. Anyone who has it can read and send your
  mail, though, so keep it private and revoke it if you think it leaked.
- Email keeps it in your system's credential store (the Keychain on a Mac, the
  Secret Service keyring on Linux) and sends it only to Google.
- You can revoke it at any time (see below). Google also revokes all your app
  passwords when you change your Google password.

## Set it up

1. **Turn on 2-Step Verification.** Google offers app passwords only when
   2-Step Verification is on: [myaccount.google.com/signinoptions/twosv](https://myaccount.google.com/signinoptions/twosv).
2. **Create an app password.** Open
   [myaccount.google.com/apppasswords](https://myaccount.google.com/apppasswords),
   enter a name such as "Email" and select **Create**. Google shows the
   16-letter password once.
3. **Add it in Email.** Choose **Add account**, then **Gmail**. Enter your Gmail
   address, paste the app password (spaces don't matter) and select
   **Add account**. Email checks the password with Google before saving
   anything, then downloads your inbox.

Google Workspace addresses work the same way if your organization allows it.

## If it doesn't work

| Email says | What to do |
| --- | --- |
| "That isn't a Google app password" | You entered something other than the 16 letters, often your regular Google password. Create an app password (step 2) and paste all of it. With the Gmail choice, Email never sends a value that isn't shaped like an app password. |
| "Google didn't accept this app password" | Check the address. Create a new app password and paste it; an old one may have been revoked, for example after a Google password change. |
| "Google needs an app password here" | Google refused your regular password. Use an app password. |
| "Your Google Workspace administrator has turned off IMAP" | Your administrator controls this in the Google Admin console (Apps › Google Workspace › Gmail › End User Access). Ask them to enable IMAP access. |
| "Google blocked this sign-in" | Open Gmail in your browser on this computer and confirm that the sign-in was you, then try again. |
| "Couldn't reach imap.gmail.com:993" | Check your internet connection, or a firewall or VPN that blocks mail ports 993 and 465. |

**No App passwords option in your Google Account?** Google doesn't offer app
passwords to accounts that use only security keys for 2-Step Verification, to
accounts enrolled in Advanced Protection, or to some work and school accounts.
Those accounts can connect once one-click Google sign-in is available, or your
administrator can help. See Google's
[app password help](https://support.google.com/accounts/answer/185833).

## What works

Reading, searching, sending, archiving, marking read or unread, moving to Spam,
triaging several selected messages at once, snoozes and reminders work as they
do for other accounts. Archive moves a message to Gmail's All Mail, which
removes only its Inbox label. Read state changed elsewhere (for example in Gmail
on the web) appears after the next sync.

Not available over an app password: Gmail labels (applying or removing them in
Email), choosing another send-as address, cleaning up every message from a
sender across the whole mailbox, and connecting Google Calendar. These need
Google sign-in.

Known limitation: a message you archive or delete elsewhere (Gmail on the web,
your phone, a Gmail filter) stays in Email's inbox until you archive it in
Email too. This applies to every IMAP account today.

## Revoke access

- **In Google:** open [App passwords](https://myaccount.google.com/apppasswords)
  and remove the one you created for Email. Email can no longer reach your mail
  from that moment.
- **In Email:** remove the account. This deletes its downloaded mail and the
  stored app password from this computer, but it does not revoke the password
  at Google, so also remove it there.

To remove everything Email stores, see [Delete your data](delete-your-data.md).
