# Forking Email by YCorp

Email by YCorp is MIT licensed, so fork it, change it and ship your own build.
Just bring your own:

- name, logo and app icon (see [TRADEMARKS.md](TRADEMARKS.md));
- Google and Microsoft OAuth clients;
- signing, update feed and support channels.

## What a plain build gets

A build that selects no distribution profile uses the neutral `unofficial`
profile ([reference](docs/distribution-profile.md)). It is safe to run next to
an official installation:

- identifiers under the reserved `.invalid` domain, so its bundle ID, Keychain
  items, Secret Service collection and data directories never touch an official
  app's tokens or mail;
- no OAuth client: Gmail browser sign-in is unavailable until you add your own;
- no support or feedback address: **Support & feedback** says so instead;
- no Scheduling service: Bookings, booking links and open tracking are hidden;
- no update feed: the macOS updater is disabled;
- no use of the YCorp name.

A plain build still uses the project's app icon. Replace it before you
distribute a modified build (step 1).

## Checklist for shipping your own build

### 1. Choose your name and identity

- Pick a product name that does not contain "YCorp" and does not suggest an
  official build. The in-app word "Email" is generic and may stay.
- Replace the app icon: `assets/email-icon.png` (Linux) and
  `scripts/generate-macos-icon.swift` (macOS).
- Pick a reverse-DNS base on a domain you control, for example
  `com.example.mail`. Do not reuse `com.ycorplabs.*`,
  `dev.email.client` or any value from `config/profiles/official.env` or
  `config/profiles/development.env`. Reusing them would let your build read
  or overwrite another app's credentials and data.
- Choose the bundle ID before your first public release. It cannot change later
  without breaking Keychain access, stored data and Sparkle updates.

### 2. Create your distribution profile

```sh
cp config/profiles/unofficial.env config/profiles/acme.env
# Edit every value; set PROFILE=acme to match the file name.
echo acme > config/distribution-profile.local    # ignored by Git
python3 scripts/distribution-profile.py get MACOS_BUNDLE_ID
```

You can keep the profile elsewhere and select it with
`EMAIL_DISTRIBUTION_PROFILE=/absolute/path/acme.env`. Set at least:

- `MACOS_BUNDLE_ID`, `MACOS_DATA_DIRECTORY` and `KEYCHAIN_NAMESPACE`, for
  example `com.example.mail` for all three;
- `LINUX_APP_ID`, using [Flathub's ID rules](https://docs.flathub.org/docs/for-app-authors/requirements):
  3 to 5 components, lowercase domain, for example `com.example.Mail`. Also
  `LINUX_DATA_DIRECTORY`, `LINUX_SECRET_COLLECTION` and `LINUX_DISPATCH_UNIT`;
- `MCP_SERVER_NAME`, the default MCP client registration name;
- `DISTRIBUTION=true` only for the profile you sign and publish;
- `GOOGLE_CLIENT_ID_REQUIRED=true` once you have a Google client, so builds
  fail clearly when it is missing;
- `GOOGLE_OAUTH_PUBLIC=true` once your Google project may sign in any user
  (published or verified); with `false`, Gmail connects only through an app
  password. See the [profile reference](docs/distribution-profile.md).

### 3. Register your own Google desktop OAuth client

1. In Google Cloud Console, create a project and configure the OAuth consent
   screen with your app name, support email and privacy policy.
2. Enable the Gmail API. Also enable the People API for contact photos and the
   Google Calendar API for calendars.
3. Create an OAuth client of type **Desktop app**. Email signs in through the
   system browser with PKCE and a `127.0.0.1` loopback redirect.
4. Put the values in the ignored `config/oauth-desktop.local.env`:

   ```sh
   EMAIL_GOOGLE_CLIENT_ID=<your client ID>.apps.googleusercontent.com
   EMAIL_GOOGLE_CLIENT_SECRET=<its desktop client secret>
   ```

   Exported environment variables override this file. Never commit either
   value: Google's [OAuth policies](https://developers.google.com/identity/protocols/oauth2/policies)
   forbid client credentials in public repositories. For signed releases, use
   `EMAIL_RELEASE_GOOGLE_CLIENT_ID` and `EMAIL_RELEASE_GOOGLE_CLIENT_SECRET`
   from your release secret store.

The app requests `gmail.modify`, plus optional `contacts.readonly`,
`contacts.other.readonly`, `calendar.events` and
`calendar.calendarlist.readonly`. These are sensitive and restricted scopes:

- In **Testing** status, only listed test users (at most 100) can sign in.
  Their refresh tokens expire after 7 days, so they must sign in again
  ([Google](https://developers.google.com/identity/protocols/oauth2)).
- An app for **personal use** does not need verification. Google defines this
  as not shared with anyone else, or used by fewer than 100 people who are all
  known personally to you. The unverified-app warning, the 100-user cap and
  token limits still apply
  ([Google](https://support.google.com/cloud/answer/13464323)).
- Anything wider needs Google's verification, which for restricted Gmail scopes
  includes a security assessment.

### 4. Optional: a Microsoft client

Register an app in Microsoft Entra with the **Mobile and desktop
applications** platform and the redirect URI `http://localhost/callback`. At
sign-in Email listens on `http://localhost:<port>/callback`; Microsoft ignores
the port for `localhost` redirects. Request
`offline_access Mail.ReadWrite Mail.Send User.Read`, then set
`EMAIL_MICROSOFT_CLIENT_ID` in the same local file or environment.

### 5. Support channels

Set `SUPPORT_ADDRESS` and `FEEDBACK_ADDRESS` to two different mailboxes that you
operate, or leave both empty. Users may attach logs and screenshots, so make
sure both addresses deliver.

### 6. Scheduling

Bookings, booking links and open tracking need a Scheduling service. Run your
own and set `SCHEDULING_ORIGIN=https://scheduling.example.com`: HTTPS origin
only, no path. The engine accepts tracking pixels only from that origin. Leave
it empty to hide the integration.

Per-draft open tracking (**Track opens**) is a separate switch,
`OPEN_TRACKING`. Keep it `false` for anything you distribute: the parsers refuse
`true` with `DISTRIBUTION=true` or without a `SCHEDULING_ORIGIN`, and without it
the engine compiles the tracking pixel out. Saved activity for mail already
sent with tracking stays readable. See [reader privacy](docs/reader-privacy.md).

### 7. Updates

Linux builds update through the system package manager. On macOS:

1. Generate your own Sparkle EdDSA key pair with Sparkle's `generate_keys`.
   Keep the private key in a secret store with an offline backup.
2. Set `UPDATE_FEED_URL`, `UPDATE_PUBLIC_ED_KEY` and `UPDATE_ALLOWED_HOSTS`. The
   feed must be HTTPS on one of the allowed hosts.
3. Publish with `scripts/publish-macos-update.sh`. See
   [macOS updates](docs/macos-updates.md).

With no allowed hosts, the updater stays disabled and packaging refuses a feed.
Never point a build at another publisher's feed.

### 8. Sign, notarize and package

- **macOS:** join the Apple Developer Program and create a Developer ID
  Application certificate. Store a notarytool profile with
  `xcrun notarytool store-credentials`. Then run:

  ```sh
  EMAIL_RELEASE_CODESIGN_IDENTITY="Developer ID Application: Your Org (TEAMID)" \
  EMAIL_NOTARY_PROFILE=your-profile \
  EMAIL_RELEASE_GOOGLE_CLIENT_ID=... EMAIL_RELEASE_GOOGLE_CLIENT_SECRET=... \
    scripts/package-macos-release.sh --distribute
  ```

  Distribution requires `DISTRIBUTION=true`. See
  [macOS release packaging](docs/macos-release.md).
- **Linux:** run `EMAIL_LINUX_RELEASE_CHANNEL=distribution scripts/package-linux-deb.sh`,
  or use the Arch PKGBUILD. Packages install the desktop entry, AppStream
  metadata and icon under your `LINUX_APP_ID`.

### 9. Check before you publish

```sh
python3 scripts/distribution-profile.test.py
cargo test --workspace
python3 scripts/validate-local.py --profile source
```

Then confirm in the built app:

- **Support & feedback** shows your addresses, or the no-support notice;
- Bookings appear only when you configured Scheduling;
- the bundle or app ID is your own.

## Contributing back

Fixes are welcome upstream too; see [CONTRIBUTING.md](CONTRIBUTING.md).
