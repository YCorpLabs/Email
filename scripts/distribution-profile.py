#!/usr/bin/env python3
"""Resolve the build's distribution profile: the one place naming an official build.

Selection: EMAIL_DISTRIBUTION_PROFILE (a profile name or file path), else the
ignored config/distribution-profile.local file, else the neutral `unofficial`
profile. The Rust engine and Linux shell resolve the same file at compile time
(crates/distribution-profile). Keep the two parsers' rules identical.

OAuth application values are build credentials, not profile values. They come
from the environment or the ignored config/oauth-desktop.local.env, with the
environment taking precedence. Never print them except as shell assignments.
"""

import argparse
import base64
import hashlib
import os
from pathlib import Path
import re
import shlex
import sys

ROOT = Path(__file__).resolve().parents[1]
SELECTION_FILE = "config/distribution-profile.local"
OAUTH_FILE = "config/oauth-desktop.local.env"
DEFAULT_PROFILE = "unofficial"
OAUTH_NAMES = ("EMAIL_GOOGLE_CLIENT_ID", "EMAIL_GOOGLE_CLIENT_SECRET", "EMAIL_MICROSOFT_CLIENT_ID")
# SHA-256 of Y Corp AS's development Google desktop client ID. Distribution
# builds refuse it without the ID itself appearing in source control.
DEVELOPMENT_GOOGLE_CLIENT_SHA256 = frozenset({
    "65bf0d27eaac8c8573ee4a865a6149a42a0c110b56010b56fb13adbf522506b4",
})

COMPONENT = r"[A-Za-z0-9-]+"
LINUX_COMPONENT = r"[A-Za-z_][A-Za-z0-9_-]*"
NAME = r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}"
LABEL = r"[A-Za-z0-9][A-Za-z0-9 ._-]{0,63}"
HOST = r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)+"
ADDRESS = r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+"
GOOGLE_CLIENT_ID = r"[a-zA-Z0-9-]+\.apps\.googleusercontent\.com"
BOOLEAN = r"true|false"
RULES = {
    "PROFILE": r"[a-z0-9][a-z0-9-]{0,39}",
    "DISTRIBUTION": BOOLEAN,
    "GOOGLE_CLIENT_ID_REQUIRED": BOOLEAN,
    # Whether one-click Google sign-in is offered (Google verification state).
    "GOOGLE_OAUTH_PUBLIC": BOOLEAN,
    "MACOS_BUNDLE_ID": rf"{COMPONENT}(?:\.{COMPONENT}){{2,}}",
    "MACOS_DATA_DIRECTORY": LABEL,
    "KEYCHAIN_NAMESPACE": NAME,
    "LINUX_APP_ID": rf"{LINUX_COMPONENT}(?:\.{LINUX_COMPONENT}){{2,4}}",
    "LINUX_DATA_DIRECTORY": NAME,
    "LINUX_SECRET_COLLECTION": LABEL,
    "LINUX_DISPATCH_UNIT": NAME,
    "MCP_SERVER_NAME": r"[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}",
    "SUPPORT_ADDRESS": rf"(?:{ADDRESS})?",
    "FEEDBACK_ADDRESS": rf"(?:{ADDRESS})?",
    "SCHEDULING_ORIGIN": rf"(?:https://{HOST})?",
    "UPDATE_FEED_URL": r"(?:https://\S+)?",
    "UPDATE_PUBLIC_ED_KEY": r"(?:[A-Za-z0-9+/]{43}=)?",
    "UPDATE_ALLOWED_HOSTS": rf"(?:{HOST}(?: {HOST})*)?",
    # Per-draft email open tracking (Track opens). Development builds only.
    "OPEN_TRACKING": BOOLEAN,
}


class ProfileError(ValueError):
    pass


def parse(text, source):
    """Strict KEY=value lines: no quotes, expansion, export or duplicates."""
    if any(ord(c) < 32 and c not in "\t\r\n" or ord(c) == 127 for c in text) or re.search(r"\r(?!\n)", text):
        raise ProfileError(f"{source}: control characters are not allowed")
    values = {}
    # Split on newlines only, as the Rust parser does; str.splitlines() splits more.
    for number, raw in enumerate(text.split("\n"), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        key, separator, value = line.partition("=")
        if not separator or key not in RULES:
            raise ProfileError(f"{source}:{number}: expected KEY=value with a known profile key")
        if key in values:
            raise ProfileError(f"{source}:{number}: {key} is set twice")
        values[key] = value
    missing = [key for key in RULES if key not in values]
    if missing:
        raise ProfileError(f"{source}: missing {', '.join(missing)}")
    for key, rule in RULES.items():
        if not re.fullmatch(rule, values[key]) or ".." in values[key]:
            raise ProfileError(f"{source}: {key} is not valid")
    # At most 127 labels, as the Rust parser counts them.
    names = [values["MACOS_BUNDLE_ID"], values["SCHEDULING_ORIGIN"]] + values["UPDATE_ALLOWED_HOSTS"].split()
    names += [values[k].rpartition("@")[2] for k in ("SUPPORT_ADDRESS", "FEEDBACK_ADDRESS")]
    if any(name.count(".") > 126 for name in names) or len(values["LINUX_APP_ID"]) > 255:
        raise ProfileError(f"{source}: a name has more than 127 labels")
    support, feedback = values["SUPPORT_ADDRESS"], values["FEEDBACK_ADDRESS"]
    if bool(support) != bool(feedback) or (support and support.lower() == feedback.lower()):
        raise ProfileError(f"{source}: set both SUPPORT_ADDRESS and FEEDBACK_ADDRESS, as different addresses, or neither")
    # A distributed build never tracks opens, and tracking needs the service.
    if values["OPEN_TRACKING"] == "true" and (values["DISTRIBUTION"] == "true" or not values["SCHEDULING_ORIGIN"]):
        raise ProfileError(f"{source}: OPEN_TRACKING=true needs a SCHEDULING_ORIGIN and is not allowed with DISTRIBUTION=true")
    feed, key = values["UPDATE_FEED_URL"], values["UPDATE_PUBLIC_ED_KEY"]
    if bool(feed) != bool(key):
        raise ProfileError(f"{source}: UPDATE_FEED_URL and UPDATE_PUBLIC_ED_KEY must be set together")
    if key and not valid_ed_key(key):
        raise ProfileError(f"{source}: UPDATE_PUBLIC_ED_KEY is not a 32-byte EdDSA public key")
    if feed and not feed_allowed(feed, values["UPDATE_ALLOWED_HOSTS"]):
        raise ProfileError(f"{source}: UPDATE_FEED_URL must be HTTPS on one of UPDATE_ALLOWED_HOSTS")
    return values


def valid_ed_key(text):
    try:
        data = base64.b64decode(text, validate=True)
    except ValueError:
        return False
    return len(data) == 32 and any(data)


# ASCII-only lowering, as Rust's to_ascii_lowercase; str.lower() also folds
# characters such as the Kelvin sign into ASCII letters.
ASCII_LOWER = str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz")


def feed_allowed(url, hosts):
    """Mirrors feed_allowed in crates/distribution-profile: compare the raw
    authority so empty fragments, empty userinfo and padded ports are refused."""
    if not url.startswith("https://") or "#" in url:
        return False
    rest = url[len("https://"):]
    split = min((i for i in (rest.find("/"), rest.find("?")) if i >= 0), default=len(rest))
    authority, tail = rest[:split], rest[split:]
    host, _, port = authority.partition(":")
    path = tail.split("?", 1)[0]
    return ("@" not in authority and port in ("", "443") and len(path) > 1
            and host.translate(ASCII_LOWER) in hosts.split(" "))


def selected(root=ROOT, environ=os.environ):
    """Return the profile file path, following the documented precedence."""
    choice = environ.get("EMAIL_DISTRIBUTION_PROFILE", "").strip()
    origin = "EMAIL_DISTRIBUTION_PROFILE"
    selection = root / SELECTION_FILE
    if not choice and selection.exists():
        # An unreadable selection must fail, never fall back to the default.
        try:
            choice = selection.read_text().strip()
        except (OSError, UnicodeDecodeError) as error:
            raise ProfileError(f"{SELECTION_FILE} cannot be read: {error.__class__.__name__}") from None
        origin = SELECTION_FILE
    if not choice:
        return root / "config/profiles" / f"{DEFAULT_PROFILE}.env"
    if re.fullmatch(RULES["PROFILE"], choice):
        path = root / "config/profiles" / f"{choice}.env"
    elif choice.startswith("/") or choice.endswith(".env"):
        path = (root / choice) if not choice.startswith("/") else Path(choice)
    else:
        raise ProfileError(f"{origin} must name a profile in config/profiles or a .env file path")
    if not path.is_file():
        raise ProfileError(f"{origin} selects a profile that does not exist: {choice}")
    return path


def require_selection(root=ROOT, environ=os.environ):
    """A checkout with local OAuth values must choose its identity explicitly.

    Otherwise forgetting the selection file would silently build the neutral
    identity, which cannot see that checkout's existing accounts and data."""
    explicit = environ.get("EMAIL_DISTRIBUTION_PROFILE", "").strip() or (root / SELECTION_FILE).exists()
    if (root / OAUTH_FILE).is_file() and not explicit:
        raise ProfileError(
            f"{OAUTH_FILE} exists but no distribution profile is selected. Write a profile name to "
            f"{SELECTION_FILE} (ignored by Git), for example `echo development > {SELECTION_FILE}` to keep "
            "the existing development identity, or `echo unofficial > ...` for a neutral build.")


def load(root=ROOT, environ=os.environ, path=None):
    path = Path(path) if path else selected(root, environ)
    # No newline translation: a bare carriage return is refused, as in Rust.
    with open(path, encoding="utf-8", newline="") as stream:
        values = parse(stream.read(), path.name)
    if path.parent == root / "config/profiles" and values["PROFILE"] != path.stem:
        raise ProfileError(f"{path.name}: PROFILE must match the file name")
    return path.resolve(), values


def named(name, root=ROOT):
    return load(root, {}, root / "config/profiles" / f"{name}.env")[1]


def read_oauth_file(root=ROOT):
    """Read the ignored local OAuth file without executing it."""
    path = root / OAUTH_FILE
    if not path.is_file():
        return {}
    values = {}
    for number, raw in enumerate(path.read_text().splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        try:
            words = shlex.split(line)
        except ValueError:
            raise ProfileError(f"{OAUTH_FILE}:{number}: cannot parse this line") from None
        # As in the shell, `#` starts a comment only at the start of a word.
        for index, word in enumerate(words):
            if word.startswith("#"):
                words = words[:index]
                break
        if words[:1] == ["export"]:
            words = words[1:]
        if len(words) != 1 or "=" not in words[0]:
            raise ProfileError(f"{OAUTH_FILE}:{number}: expected NAME=value")
        name, value = words[0].split("=", 1)
        values[name] = value
    return values


def oauth_values(root=ROOT, environ=os.environ):
    local = read_oauth_file(root)
    return {name: environ.get(name) or local.get(name, "") for name in OAUTH_NAMES}


def is_development_google_client(value):
    """The frozen development client, by digest. A local file may legitimately
    hold a production client on a release machine, so it is not compared."""
    return hashlib.sha256(value.encode()).hexdigest() in DEVELOPMENT_GOOGLE_CLIENT_SHA256


def shell(root, environ):
    path, values = load(root, environ)
    lines = [f"EMAIL_DISTRIBUTION_PROFILE={shlex.quote(str(path))}"]
    lines += [f"EMAIL_PROFILE_{key}={shlex.quote(value)}" for key, value in values.items()]
    for name, value in oauth_values(root, environ).items():
        lines.append(f"{name}={shlex.quote(value)}")
    return "\n".join(lines) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("shell", help="Print shell assignments for build scripts")
    get = commands.add_parser("get", help="Print one profile value")
    get.add_argument("key", choices=sorted(RULES))
    get.add_argument("--profile", help="Read this named profile instead of the selection")
    commands.add_parser("check-google-client",
                        help="Validate EMAIL_GOOGLE_CLIENT_ID from the environment for this profile")
    commands.add_parser("check-release-google-client",
                        help="Refuse a development client in EMAIL_RELEASE_GOOGLE_CLIENT_ID")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    try:
        if args.command in ("shell", "check-google-client") or (args.command == "get" and not args.profile):
            require_selection(root, os.environ)
        if args.command == "shell":
            sys.stdout.write(shell(root, os.environ))
        elif args.command == "get":
            values = named(args.profile, root) if args.profile else load(root)[1]
            print(values[args.key])
        elif args.command == "check-google-client":
            values = load(root)[1]
            # Same precedence as builds: the environment, then the local file.
            client = oauth_values(root, os.environ)["EMAIL_GOOGLE_CLIENT_ID"]
            if not client and values["GOOGLE_CLIENT_ID_REQUIRED"] == "true":
                raise ProfileError(
                    f"The {values['PROFILE']} profile needs a Google desktop OAuth client ID. "
                    f"Add EMAIL_GOOGLE_CLIENT_ID=<client ID> to {OAUTH_FILE} (ignored by Git) "
                    "or export EMAIL_GOOGLE_CLIENT_ID before building. See FORKING.md.")
            if client and not re.fullmatch(GOOGLE_CLIENT_ID, client):
                raise ProfileError(
                    f"EMAIL_GOOGLE_CLIENT_ID (from the environment or {OAUTH_FILE}) must be a Google "
                    "desktop OAuth client ID ending in .apps.googleusercontent.com.")
            # The frozen development identity's stored tokens are bound to its
            # client; a different one would change the signed Info.plist too.
            if (client and values["PROFILE"] == "development"
                    and hashlib.sha256(client.encode()).hexdigest() not in DEVELOPMENT_GOOGLE_CLIENT_SHA256):
                raise ProfileError(
                    "The development profile is frozen to its existing Google client. Remove the "
                    f"EMAIL_GOOGLE_CLIENT_ID override or fix {OAUTH_FILE}; use another profile for "
                    "a different client.")
        elif args.command == "check-release-google-client":
            client = os.environ.get("EMAIL_RELEASE_GOOGLE_CLIENT_ID", "")
            if not re.fullmatch(GOOGLE_CLIENT_ID, client) or is_development_google_client(client):
                raise ProfileError("Distribution requires a production Google desktop client ID, "
                                   "not a development client.")
    except (ProfileError, OSError) as error:
        print(f"Distribution profile: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
