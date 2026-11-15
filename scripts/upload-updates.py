#!/usr/bin/env python3
"""Upload a verified update staging directory to the update bucket.

Run through scripts/upload-updates.sh. See docs/linux-updates.md,
docs/arch-updates.md and docs/macos-updates.md.

    scripts/upload-updates.sh --dry-run --source ../email-apt --prefix email/apt
    scripts/upload-updates.sh --source ../email-apt --prefix email/apt

The source is an APT repository staged by publish-linux-apt.sh (it has
dists/), a pacman repository staged by publish-arch-repo.sh (it has
ycorp.asc and <arch>/ycorp.db) or a Sparkle feed staged by
publish-macos-update.sh (it has appcast.xml). Before anything is uploaded the
source is verified again, as a client would, and compared with what is
published: a staged index that lacks a published package or build is refused,
so a stale staging directory can never drop earlier releases.

Files go up in dependency order, so an index never names a file that is not
there yet: packages, archives and by-hash indexes first, then Packages and
release notes, then Release and Release.gpg, and InRelease, ycorp.db or
appcast.xml last. Versioned files are cached for a year and never change;
indexes are cached for five minutes. Uploads use Wrangler, pinned exactly,
with `--remote`; Wrangler takes its Cloudflare credentials from its own login
or CLOUDFLARE_API_TOKEN. Dotfiles are never uploaded: `.upload-manifest.json`
records the versioned files already uploaded, so they are not sent again and
can never be replaced by different bytes.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import re
import shlex
import subprocess
import sys
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
WRANGLER = "wrangler@4.155.0"
DEFAULT_BUCKET = "ycorp-updates"
DEFAULT_PUBLIC_URL = "https://updates.ycorplabs.com"
MANIFEST = ".upload-manifest.json"
IMMUTABLE = "public, max-age=31536000, immutable"
SHORT = "public, max-age=300"
SPARKLE = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"
CONTENT_TYPES = {
    ".deb": "application/vnd.debian.binary-package",
    ".zst": "application/zstd",
    ".sig": "application/pgp-signature",
    ".gz": "application/gzip",
    ".gpg": "application/pgp-signature",
    ".asc": "application/pgp-keys",
    ".xml": "application/xml",
    ".zip": "application/zip",
    ".delta": "application/octet-stream",
    ".md": "text/markdown; charset=utf-8",
    ".html": "text/html; charset=utf-8",
}


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


arch = load("arch_repository", "arch-repository.py")
# The APT module arch-repository.py loaded, so one RepositoryError covers both.
apt = arch.apt


class UploadError(ValueError):
    pass


def files(source):
    """Every regular file, relative to the source; dotfiles are skipped."""
    result = []
    for path in sorted(source.rglob("*")):
        relative = PurePosixPath(path.relative_to(source).as_posix())
        if any(part.startswith(".") for part in relative.parts):
            continue
        if path.is_symlink():
            raise UploadError(f"{relative} is a symlink; refusing to follow it")
        if path.is_file():
            result.append(relative)
    return result


def classify_apt(name):
    """Upload phase (lower first) and caching for an APT repository file."""
    parts = name.parts
    if parts[0] == "pool" and name.suffix == ".deb":
        return 0, IMMUTABLE
    if parts[0] == "dists" and len(parts) >= 4 and "by-hash" in parts:
        return 0, IMMUTABLE
    if parts[0] == "dists" and name.name in ("Packages", "Packages.gz"):
        return 1, SHORT
    if len(parts) == 1 and name.suffix == ".asc":
        return 1, SHORT
    if parts[0] == "dists" and len(parts) == 3 and name.name in ("Release", "Release.gpg"):
        return 2, SHORT
    if parts[0] == "dists" and len(parts) == 3 and name.name == "InRelease":
        return 3, SHORT
    raise UploadError(f"{name} is not part of an APT repository; remove it from the staging directory")


def classify_arch(name):
    """Upload phase and caching for a pacman repository file. ycorp.db and
    its signature go last: pacman -Sy reads nothing else."""
    parts = name.parts
    if len(parts) == 1 and name.name == arch.KEY_FILE:
        return 1, SHORT
    if len(parts) == 2 and parts[0] in arch.ARCHITECTURES:
        package = arch.PACKAGE_FILE.fullmatch(parts[1].removesuffix(".sig"))
        if package and package["arch"] == parts[0]:
            return 0, IMMUTABLE
        if parts[1] in (f"{arch.REPOSITORY}.db", f"{arch.REPOSITORY}.db.sig"):
            return 3, SHORT
        if parts[1] in arch.database_files():
            return 2, SHORT
    raise UploadError(f"{name} is not part of a pacman repository; remove it from the staging directory")


def classify_macos(name):
    """Upload phase and caching for a Sparkle feed file."""
    if len(name.parts) != 1:
        raise UploadError(f"{name}: a Sparkle feed directory has no subdirectories")
    if name.suffix in (".zip", ".delta"):
        return 0, IMMUTABLE
    if name.suffix in (".md", ".html"):
        return 1, SHORT
    if name.name == "appcast.xml":
        return 3, SHORT
    raise UploadError(f"{name} is not part of a Sparkle feed; remove it from the staging directory")


def content_type(name):
    if name.name in ("Packages", "Release", "InRelease"):
        return "text/plain; charset=utf-8"
    if name.name in (f"{arch.REPOSITORY}.db", f"{arch.REPOSITORY}.files"):
        return "application/gzip"
    return CONTENT_TYPES.get(name.suffix, "application/octet-stream")


def fetch(url):
    """Published bytes at url, or None when nothing is published there yet."""
    request = urllib.request.Request(url, headers={"User-Agent": "email-upload-updates", "Cache-Control": "no-cache"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.read()
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise UploadError(f"{url}: HTTP {error.code}") from None
    except urllib.error.URLError as error:
        raise UploadError(f"{url}: {error.reason}; use --skip-remote-check only if nothing is published") from None


def check_remote_apt(source, base):
    """Every published package entry must still be in the staged index."""
    for suite in apt.SUITES:
        for architecture in apt.ARCHITECTURES:
            relative = f"dists/{suite}/main/binary-{architecture}/Packages"
            published = fetch(f"{base}/{relative}")
            if not published:
                continue
            staged = {(e["Filename"], e["SHA256"]) for e in apt.read_index(source, suite, architecture)}
            for entry in apt.paragraphs(published.decode()):
                if (entry["Filename"], entry["SHA256"]) not in staged:
                    raise UploadError(f"{relative}: the published {entry['Filename']} ({entry['Version']}) is missing "
                                      "or different in the staged repository. Recreate the staging directory from "
                                      "the published repository before publishing again.")


def check_remote_arch(source, base):
    """The published database entries must still be in the staged databases,
    the same or older, and their package files must still be staged."""
    for architecture in arch.ARCHITECTURES:
        relative = f"{architecture}/{arch.REPOSITORY}.db"
        published = fetch(f"{base}/{relative}")
        if not published:
            continue
        staged = arch.read_db(source, architecture)
        for name, entry in arch.parse_db(published).items():
            filename, digest = arch.field(entry, "FILENAME"), arch.field(entry, "SHA256SUM")
            version, ours = arch.field(entry, "VERSION"), staged.get(name)
            newer = ours is not None and arch.vercmp(arch.field(ours, "VERSION"), version)
            same = ours is not None and (arch.field(ours, "FILENAME"), arch.field(ours, "SHA256SUM")) == (filename, digest)
            package = source / architecture / filename
            if ours is None or newer < 0 or (newer == 0 and not same) or not package.is_file() or \
                    sha256(package) != digest:
                raise UploadError(f"{relative}: the published {filename} ({version}) is missing, newer or different "
                                  "in the staged repository. Recreate the staging directory from the published "
                                  "repository before publishing again.")


def appcast_items(data):
    items = {}
    for item in ET.fromstring(data).iter("item"):
        version = item.findtext(f"{SPARKLE}version")
        enclosure = item.find("enclosure")
        attributes = dict(enclosure.attrib) if enclosure is not None else {}
        items[version] = (attributes.get("url"), attributes.get("length"), attributes.get(f"{SPARKLE}edSignature"))
    return items


def check_remote_macos(source, base):
    """Every published build must still be in the staged appcast, unchanged."""
    published = fetch(f"{base}/appcast.xml")
    if not published:
        return
    staged = appcast_items((source / "appcast.xml").read_bytes())
    for version, enclosure in appcast_items(published).items():
        if staged.get(version) != enclosure:
            raise UploadError(f"appcast.xml: published build {version} is missing or different in the staged feed. "
                              "Recreate the staging directory from the published feed before publishing again.")


def profile_value(key):
    result = subprocess.run([sys.executable, str(HERE / "distribution-profile.py"), "get", key],
                            capture_output=True, text=True)
    if result.returncode:
        raise UploadError(result.stderr.strip() or f"cannot read {key} from the distribution profile")
    return result.stdout.strip()


def verify_local(kind, source):
    """The checks the publishers ran, again, with public keys only."""
    if profile_value("DISTRIBUTION") != "true":
        raise UploadError("Uploading requires a distribution profile, such as official (EMAIL_DISTRIBUTION_PROFILE).")
    if kind in ("apt", "arch"):
        path = subprocess.run([sys.executable, str(HERE / "distribution-profile.py"), "path"],
                              capture_output=True, text=True, check=True).stdout.strip()
        command = [sys.executable, str(HERE / f"{kind}-repository.py"), "verify", str(source),
                   "--keyring", str(apt.keyring_for(path)), "--fingerprint", profile_value("LINUX_APT_KEY_FINGERPRINT")]
    else:
        key, hosts = profile_value("UPDATE_PUBLIC_ED_KEY"), profile_value("UPDATE_ALLOWED_HOSTS")
        if not key or not hosts:
            raise UploadError("The distribution profile has no macOS update key or hosts.")
        command = [sys.executable, str(HERE / "verify-macos-appcast.py"), str(source / "appcast.xml"),
                   "--public-key", key, "--production", hosts, "--archives", str(source)]
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode:
        raise UploadError(f"the staged directory does not verify:\n{result.stdout}{result.stderr}".rstrip())
    print(result.stdout.strip() or "verified")


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def plan(source, kind, destination, manifest):
    classify = {"apt": classify_apt, "arch": classify_arch, "macos": classify_macos}[kind]
    uploaded = manifest.get(destination, {})
    steps, skipped = [], 0
    for name in files(source):
        phase, cache = classify(name)
        digest = sha256(source / name)
        if cache == IMMUTABLE and str(name) in uploaded:
            if uploaded[str(name)] != digest:
                raise UploadError(f"{name} was uploaded before with other contents; versioned files never change")
            skipped += 1
            continue
        steps.append((phase, str(name), cache, content_type(name), digest))
    steps.sort(key=lambda step: (step[0], step[1]))
    return steps, skipped


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--source", required=True, type=Path, help="staged directory")
    parser.add_argument("--prefix", required=True, help="bucket prefix: email/apt, email/arch or email/macos")
    parser.add_argument("--bucket", default=os.environ.get("EMAIL_UPDATES_BUCKET", DEFAULT_BUCKET))
    parser.add_argument("--public-url", default=os.environ.get("EMAIL_UPDATES_PUBLIC_URL", DEFAULT_PUBLIC_URL))
    parser.add_argument("--dry-run", action="store_true", help="verify and print the plan; upload nothing")
    parser.add_argument("--skip-remote-check", action="store_true",
                        help="do not compare with the published files (only when nothing is published yet)")
    args = parser.parse_args(argv)
    source = args.source.resolve()
    prefix = args.prefix.strip("/")
    try:
        if not source.is_dir():
            raise UploadError(f"{args.source} is not a directory")
        if (source / "dists").is_dir():
            kind = "apt"
        elif (source / "appcast.xml").is_file():
            kind = "macos"
        elif (source / arch.KEY_FILE).is_file():
            kind = "arch"
        else:
            raise UploadError(f"{args.source} is not an APT repository (dists/), a pacman repository "
                              f"({arch.KEY_FILE}) or a Sparkle feed (appcast.xml)")
        if not re.fullmatch(r"[a-z0-9][a-z0-9-]*(?:/[a-z0-9][a-z0-9-]*)*", prefix) or \
                prefix.rsplit("/", 1)[-1] != kind:
            raise UploadError(f"--prefix for this {kind} directory must end in /{kind}, such as email/{kind}")
        if not re.fullmatch(r"[a-z0-9][a-z0-9-]{1,62}", args.bucket):
            raise UploadError("--bucket is not an R2 bucket name")
        if not args.public_url.startswith("https://"):
            raise UploadError("--public-url must be HTTPS")
        verify_local(kind, source)
        base = f"{args.public_url.rstrip('/')}/{prefix}"
        if args.skip_remote_check:
            print("Skipping the comparison with the published files.")
        else:
            {"apt": check_remote_apt, "arch": check_remote_arch, "macos": check_remote_macos}[kind](source, base)
            print(f"Nothing published at {base} is missing from the staged directory.")
        manifest_path = source / MANIFEST
        manifest = json.loads(manifest_path.read_text()) if manifest_path.is_file() else {}
        destination = f"{args.bucket}/{prefix}"
        steps, skipped = plan(source, kind, destination, manifest)
        print(f"{'Plan' if args.dry_run else 'Uploading'}: {len(steps)} files to {destination} "
              f"with npx {WRANGLER} ({skipped} versioned files already uploaded)")
        for phase, name, cache, kind_type, digest in steps:
            command = ["npx", "--yes", WRANGLER, "r2", "object", "put", f"{destination}/{name}",
                       "--file", str(source / name), "--remote", "--content-type", kind_type, "--cache-control", cache]
            print(f"  [{phase}] {name}  ({kind_type}; {cache})")
            if args.dry_run:
                continue
            result = subprocess.run(command, stdout=subprocess.DEVNULL)
            if result.returncode:
                raise UploadError(f"upload of {name} failed; nothing after it was uploaded. Run again to resume:\n  "
                                  + shlex.join(command))
            if cache == IMMUTABLE:
                manifest.setdefault(destination, {})[name] = digest
                manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        print("Dry run: nothing was uploaded." if args.dry_run else f"Uploaded to {destination}.")
    except (UploadError, apt.RepositoryError, OSError, ET.ParseError, subprocess.CalledProcessError) as error:
        print(f"Upload: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
