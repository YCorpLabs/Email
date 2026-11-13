# Trademarks

The source code of Email by YCorp is licensed under the [MIT License](LICENSE).
The license covers the code. It does not grant rights to Y Corp AS's names,
logos or app icon.

## What is protected

- **YCorp**, including the full product name **Email by YCorp**;
- Y Corp AS logos and the official Email by YCorp app icon.

The word "Email" on its own is generic. Y Corp AS makes no claim to it.

## What you may do

- **Redistribute unmodified official builds** under their name, as received,
  including their signatures. Do not suggest that your distribution is endorsed
  by Y Corp AS.
- **Say factually** that your software is based on, or compatible with, Email by
  YCorp. For example: "based on the Email by YCorp source code".
- **Fork and modify the code** under the MIT license.

## What modified builds must do

A build is modified if you changed its source, or built it with any
distribution profile other than `official`. Modified builds must:

- use their own product name and icon, without "YCorp";
- use their own OAuth clients, not the official Google or Microsoft
  registrations;
- use their own update feed and signing keys, or none;
- use their own support and feedback channels, or none;
- use their own app identifiers, never those in `config/profiles/official.env`
  or `config/profiles/development.env`.

[FORKING.md](FORKING.md) lists every place to change. A build that selects no
profile already uses its own identifiers and none of the official OAuth
clients, feeds or support channels. It still carries the project's app icon,
which you must replace before distributing a modified build.

## Questions

For permission requests or questions about this policy, email
support@ycorplabs.com.
