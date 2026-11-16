#!/usr/bin/env python3
"""pacman repository rules for Email's Arch Linux package (docs/arch-updates.md).

Used by publish-arch-repo.sh (package checks and the staged repository),
upload-updates.py (upload order and the published-history check) and the
tests. Standard library only, so it runs on the maintainer's Mac. It never
reads private key material: signatures are checked with gpgv against the
committed public key only.

Repository layout, relative to https://updates.ycorplabs.com/email/arch:

  ycorp.asc                                           public signing key
  <arch>/email-ycorp-bin-<pkgver>-<pkgrel>-<arch>.pkg.tar.zst   and .sig
  <arch>/ycorp.db, <arch>/ycorp.db.tar.gz             database, and .sig
  <arch>/ycorp.files, <arch>/ycorp.files.tar.gz       file lists, and .sig

<arch> is x86_64 or aarch64. repo-add writes ycorp.db and ycorp.files as
symlinks to the .tar.gz files; the bucket has no symlinks, so they are real
copies with the same bytes, and so are their signatures. Every package file
stays in the repository; the database names only the newest one.
"""

import argparse
import base64
import gzip
import hashlib
import io
import importlib.util
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile

REPOSITORY = "ycorp"
PACKAGE = "email-ycorp-bin"
ARCHITECTURES = ("x86_64", "aarch64")
KEY_FILE = f"{REPOSITORY}.asc"
DATABASES = ("db", "files")
PACKAGE_SUFFIX = ".pkg.tar.zst"
# The Arch form of a release version (docs/release-versions.md): 0.1.0beta3,
# 0.2.0rc1, 0.2.0. No epoch; pkgrel rises only for packaging-only changes.
PKGVER = r"[0-9]+\.[0-9]+\.[0-9]+(?:[a-z]+[0-9]+(?:\.[0-9]+)*)?"
PKGREL = r"[1-9][0-9]*"
FULL_VERSION = re.compile(rf"(?P<pkgver>{PKGVER})-(?P<pkgrel>{PKGREL})")
PACKAGE_FILE = re.compile(rf"{re.escape(PACKAGE)}-(?P<version>{PKGVER}-{PKGREL})-(?P<arch>[a-z0-9_]+){re.escape(PACKAGE_SUFFIX)}")
FINGERPRINT = re.compile(r"[0-9A-F]{40}")
# The largest signature repo-add accepts.
MAX_SIGNATURE = 16384


def _load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# The APT repository's key signs this repository too (LINUX_APT_KEY_FINGERPRINT,
# config/profiles/<profile>.apt.asc); its keyring helpers parse it.
apt = _load("apt_repository", "apt-repository.py")


class RepositoryError(apt.RepositoryError):
    pass


# --- Versions -----------------------------------------------------------------

def _alpha(char):
    return ("a" <= char <= "z") or ("A" <= char <= "Z")


def _digit(char):
    return "0" <= char <= "9"


def _rpmvercmp(a, b):
    """libalpm's rpmvercmp, character for character."""
    if a == b:
        return 0
    one = two = 0
    while one < len(a) and two < len(b):
        start1, start2 = one, two
        while one < len(a) and not (_alpha(a[one]) or _digit(a[one])):
            one += 1
        while two < len(b) and not (_alpha(b[two]) or _digit(b[two])):
            two += 1
        if one >= len(a) or two >= len(b):
            break
        if one - start1 != two - start2:
            return -1 if one - start1 < two - start2 else 1
        end1, end2 = one, two
        if _digit(a[end1]):
            while end1 < len(a) and _digit(a[end1]):
                end1 += 1
            while end2 < len(b) and _digit(b[end2]):
                end2 += 1
            numeric = True
        else:
            while end1 < len(a) and _alpha(a[end1]):
                end1 += 1
            while end2 < len(b) and _alpha(b[end2]):
                end2 += 1
            numeric = False
        segment1, segment2 = a[one:end1], b[two:end2]
        if not segment2:
            return 1 if numeric else -1
        if numeric:
            segment1, segment2 = segment1.lstrip("0"), segment2.lstrip("0")
            if len(segment1) != len(segment2):
                return 1 if len(segment1) > len(segment2) else -1
        if segment1 != segment2:
            return 1 if segment1 > segment2 else -1
        one, two = end1, end2
    if one >= len(a) and two >= len(b):
        return 0
    rest1 = a[one] if one < len(a) else ""
    rest2 = b[two] if two < len(b) else ""
    if (not rest1 and not _alpha(rest2)) or _alpha(rest1):
        return -1
    return 1


def _split(version):
    """[epoch:]pkgver[-pkgrel], as libalpm's parseEVR."""
    index = 0
    while index < len(version) and _digit(version[index]):
        index += 1
    if index < len(version) and version[index] == ":":
        epoch, rest = version[:index] or "0", version[index + 1:]
    else:
        epoch, rest = "0", version
    pkgver, separator, pkgrel = rest.rpartition("-")
    if not separator:
        return epoch, rest, None
    return epoch, pkgver, pkgrel


def vercmp(a, b):
    """Negative, zero or positive, as pacman's `vercmp a b`."""
    if a == b:
        return 0
    (e1, v1, r1), (e2, v2, r2) = _split(a), _split(b)
    result = _rpmvercmp(e1, e2) or _rpmvercmp(v1, v2)
    if result == 0 and r1 is not None and r2 is not None:
        result = _rpmvercmp(r1, r2)
    return result


# --- Package inspection -------------------------------------------------------

def parse_pkginfo(text):
    """.PKGINFO: `key = value` lines; keys such as depend repeat."""
    fields = {}
    for line in text.split("\n"):
        if not line.strip() or line.startswith("#"):
            continue
        key, separator, value = line.partition(" = ")
        if not separator or not key:
            raise RepositoryError(f"malformed .PKGINFO line: {line[:60]}")
        fields.setdefault(key, []).append(value)
    return fields


def inspect(package, inspected, profile):
    """Refuse anything but email-ycorp-bin repackaged from this profile's
    distribution .debs. `inspected` holds what the container read from the
    package: pkginfo (.PKGINFO), contents (bsdtar -t) and, when present, channel
    (the RELEASE-CHANNEL.txt the .deb carries). Returns (version, arch)."""
    package, inspected = Path(package), Path(inspected)
    read = lambda name: (inspected / name).read_text() if (inspected / name).is_file() else None
    info = parse_pkginfo(read("pkginfo") or "")
    single = lambda key: info.get(key, [""])[0] if len(info.get(key, [])) <= 1 else None
    name, base, version, architecture = single("pkgname"), single("pkgbase"), single("pkgver"), single("arch")
    if name != PACKAGE or base not in ("", PACKAGE):
        raise RepositoryError(f"{package.name}: package {name!r} is not {PACKAGE}")
    if not version or not FULL_VERSION.fullmatch(version):
        raise RepositoryError(f"{package.name}: version {version!r} is not a release version such as 0.1.0beta3-1")
    if architecture not in ARCHITECTURES:
        raise RepositoryError(f"{package.name}: architecture {architecture!r} is not one of {', '.join(ARCHITECTURES)}")
    expected = f"{PACKAGE}-{version}-{architecture}{PACKAGE_SUFFIX}"
    if package.name != expected:
        raise RepositoryError(f"{package.name}: expected the file name {expected}")
    contents = set((read("contents") or "").split("\n"))
    app_id = profile["LINUX_APP_ID"]
    for path in ("usr/bin/email-linux", "usr/lib/email-linux/mail-mcp",
                 f"usr/share/applications/{app_id}.desktop", f"usr/share/metainfo/{app_id}.metainfo.xml"):
        if path not in contents:
            raise RepositoryError(f"{package.name}: no /{path}; not built from this profile's release .deb?")
    if (read("channel") or "").strip() != "distribution":
        raise RepositoryError(f"{package.name}: not repackaged from a distribution build "
                              f"(/usr/share/licenses/{PACKAGE}/RELEASE-CHANNEL.txt)")
    return version, architecture


# --- Databases ----------------------------------------------------------------

def parse_desc(text):
    """A database entry: %KEY% then one value per line, blocks separated by
    blank lines."""
    fields, key = {}, None
    for line in text.split("\n"):
        if key is None:
            if not line:
                continue
            if not (line.startswith("%") and line.endswith("%") and len(line) > 2):
                raise RepositoryError(f"malformed database entry line: {line[:60]}")
            key = line[1:-1]
            if key in fields:
                raise RepositoryError(f"database entry repeats %{key}%")
            fields[key] = []
        elif line:
            fields[key].append(line)
        else:
            key = None
    return fields


def parse_db(data):
    """{package name: desc fields} from a repo-add database (gzip tar)."""
    entries = {}
    try:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
            for member in archive.getmembers():
                parts = member.name.strip("/").split("/")
                if member.isdir() and len(parts) == 1:
                    continue
                if not member.isfile() or len(parts) != 2 or parts[1] not in ("desc", "files"):
                    raise RepositoryError(f"unexpected database member {member.name}")
                if parts[1] != "desc":
                    continue
                fields = parse_desc(archive.extractfile(member).read().decode())
                name = (fields.get("NAME") or [""])[0]
                version = (fields.get("VERSION") or [""])[0]
                if f"{name}-{version}" != parts[0]:
                    raise RepositoryError(f"database entry {parts[0]} names {name}-{version}")
                if name in entries:
                    raise RepositoryError(f"the database lists {name} twice")
                entries[name] = fields
    except (tarfile.TarError, EOFError, gzip.BadGzipFile, UnicodeDecodeError) as error:
        raise RepositoryError(f"not a pacman database: {error}") from None
    return entries


def read_db(root, architecture, database="db"):
    path = Path(root) / architecture / f"{REPOSITORY}.{database}.tar.gz"
    return parse_db(path.read_bytes()) if path.is_file() else {}


def field(entry, key):
    values = entry.get(key) or [""]
    return values[0]


def files_listed(data):
    """Entry directories in a files database, each with desc and files."""
    entries = {}
    try:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
            for member in archive.getmembers():
                parts = member.name.strip("/").split("/")
                if member.isfile() and len(parts) == 2:
                    entries.setdefault(parts[0], set()).add(parts[1])
    except (tarfile.TarError, EOFError, gzip.BadGzipFile) as error:
        raise RepositoryError(f"not a pacman files database: {error}") from None
    return entries


# --- Signatures ---------------------------------------------------------------

class Verifier:
    """gpgv against one public keyring, as pacman checks with a locally
    signed key: only VALIDSIG from the expected primary key counts."""

    def __init__(self, keyring, expected):
        apt.check_keyring(keyring, expected)
        self.keyring, self.expected = Path(keyring), expected
        self._scratch = tempfile.TemporaryDirectory(prefix="email-arch-verify-")
        scratch = Path(self._scratch.name)
        self.ring = scratch / "keyring.gpg"
        self.ring.write_bytes(apt.dearmor(Path(keyring).read_bytes()))
        self.home = scratch / "home"
        self.home.mkdir(mode=0o700)
        self.status = scratch / "status"

    def close(self):
        self._scratch.cleanup()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def check(self, signature, data):
        signature, data = Path(signature), Path(data)
        if not signature.is_file():
            raise RepositoryError(f"{signature.name} is missing")
        raw = signature.read_bytes()
        if len(raw) > MAX_SIGNATURE or raw.startswith(b"-----"):
            raise RepositoryError(f"{signature.name} is not a binary detached signature")
        with open(self.status, "w+") as status:
            result = subprocess.run(["gpgv", "--homedir", str(self.home), "--keyring", str(self.ring),
                                     "--status-fd", str(status.fileno()), str(signature), str(data)],
                                    pass_fds=(status.fileno(),), capture_output=True)
            status.seek(0)
            lines = status.read().splitlines()
        valid = [line.split() for line in lines if line.startswith("[GNUPG:] VALIDSIG ")]
        if result.returncode or not valid or any(fields[-1] != self.expected for fields in valid):
            raise RepositoryError(f"{signature.name} is not a valid signature of {data.name} by {self.expected}")


# --- Staging ------------------------------------------------------------------

def database_files():
    names = []
    for database in DATABASES:
        for name in (f"{REPOSITORY}.{database}", f"{REPOSITORY}.{database}.tar.gz"):
            names += [name, name + ".sig"]
    return names


def copy_tree(source, destination):
    """A working copy of the staging directory. Packages and their signatures
    never change once published, so they are hard-linked when the filesystem
    allows; databases are copied."""
    def link_or_copy(src, dst):
        if Path(src).name.endswith((PACKAGE_SUFFIX, PACKAGE_SUFFIX + ".sig")):
            try:
                os.link(src, dst)
                return dst
            except OSError:
                pass
        return shutil.copy2(src, dst)
    if Path(source).is_dir():
        for path in Path(source).rglob("*"):
            if path.is_symlink():
                raise RepositoryError(f"{path} is a symlink; the staging directory holds real files only")
        shutil.copytree(source, destination, copy_function=link_or_copy, dirs_exist_ok=True)


def stage(root, packages, profile, verifier, out=sys.stdout):
    """Adds packages to a working copy of the repository, ready for repo-add.

    `packages` are (package file, inspected directory) pairs. A signature
    beside a package must be the profile key's; it is copied with the package.
    Returns [(arch, file name, signed)] for the caller to sign and add."""
    root = Path(root)
    plans, seen = [], set()
    for package, inspected in packages:
        package = Path(package)
        version, architecture = inspect(package, inspected, profile)
        if architecture in seen:
            raise RepositoryError(f"{package.name}: give one package per architecture; the database names only the newest")
        seen.add(architecture)
        signature = package.with_name(package.name + ".sig")
        if signature.exists():
            verifier.check(signature, package)
        current = field(read_db(root, architecture).get(PACKAGE, {}), "VERSION")
        if current and vercmp(version, current) <= 0:
            raise RepositoryError(f"{package.name}: {version} is not newer than {current}, already in [{REPOSITORY}] "
                                  f"for {architecture}")
        plans.append((package, signature if signature.exists() else None, version, architecture))
    added = []
    for package, signature, version, architecture in plans:
        directory = root / architecture
        directory.mkdir(exist_ok=True)
        target = directory / package.name
        if target.exists():
            # Newer than the database, yet already there: never rewrite or
            # re-sign a file that may have been uploaded.
            raise RepositoryError(f"{architecture}/{package.name} is already in the staging directory but not in "
                                  "its database; published files never change. Recreate the staging directory "
                                  "from the published repository.")
        target.write_bytes(package.read_bytes())
        target_signature = directory / (package.name + ".sig")
        if signature:
            target_signature.write_bytes(signature.read_bytes())
        # repo-add rewrites the databases; their old signatures and links go.
        for name in database_files():
            if not name.endswith(".tar.gz"):
                (directory / name).unlink(missing_ok=True)
        added.append((architecture, package.name, bool(signature)))
        print(f"staging {PACKAGE} {version} for {architecture}"
              f"{' with its signature' if signature else ''}", file=out)
    shutil.copyfile(profile["keyring"], root / KEY_FILE)
    return added


def finalize(root, architectures):
    """After repo-add: drop its .old backups and turn the ycorp.db and
    ycorp.files symlinks into real files."""
    root = Path(root)
    for architecture in architectures:
        directory = root / architecture
        for path in directory.iterdir():
            if path.name.startswith(f"{REPOSITORY}.") and (".old" in path.name or path.name.endswith(".lck")):
                path.unlink()
        for database in DATABASES:
            archive = directory / f"{REPOSITORY}.{database}.tar.gz"
            link = directory / f"{REPOSITORY}.{database}"
            if not archive.is_file():
                raise RepositoryError(f"repo-add wrote no {architecture}/{archive.name}")
            link.unlink(missing_ok=True)
            shutil.copyfile(archive, link)


def verify(root, verifier=None):
    """The staged repository as pacman reads it: every database names a
    package that exists with its size, SHA-256 and signature; ycorp.db and
    ycorp.files are real copies of the .tar.gz files; nothing else is there.
    With a verifier, every signature must be the profile key's."""
    root = Path(root)
    problems = []
    architectures = []
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        if any(part.startswith(".") for part in Path(relative).parts):
            continue
        if path.is_symlink():
            problems.append(f"{relative} is a symlink")
        elif path.is_dir():
            if relative in ARCHITECTURES:
                architectures.append(relative)
            else:
                problems.append(f"{relative}/ is not an architecture directory")
        elif "/" not in relative and relative != KEY_FILE:
            problems.append(f"{relative} is not part of the repository")
    if not (root / KEY_FILE).is_file():
        problems.append(f"{KEY_FILE} is missing")
    signatures = []
    for architecture in architectures:
        directory = root / architecture
        known = set()
        for database in DATABASES:
            archive = directory / f"{REPOSITORY}.{database}.tar.gz"
            link = directory / f"{REPOSITORY}.{database}"
            for path in (archive, link, archive.with_name(archive.name + ".sig"), link.with_name(link.name + ".sig")):
                known.add(path.name)
                if not path.is_file() or path.is_symlink():
                    problems.append(f"{architecture}/{path.name} is missing")
            if all(p.is_file() for p in (archive, link)) and archive.read_bytes() != link.read_bytes():
                problems.append(f"{architecture}/{link.name} differs from {archive.name}")
            signature, link_signature = (directory / f"{archive.name}.sig", directory / f"{link.name}.sig")
            if signature.is_file() and link_signature.is_file() and signature.read_bytes() != link_signature.read_bytes():
                problems.append(f"{architecture}/{link_signature.name} differs from {signature.name}")
            signatures += [(signature, archive), (link_signature, link)]
        try:
            entries = read_db(root, architecture)
            listed = files_listed((directory / f"{REPOSITORY}.files.tar.gz").read_bytes()) \
                if (directory / f"{REPOSITORY}.files.tar.gz").is_file() else {}
        except RepositoryError as error:
            problems.append(f"{architecture}: {error}")
            continue
        if set(listed) != {f"{name}-{field(e, 'VERSION')}" for name, e in entries.items()} or \
                any(names != {"desc", "files"} for names in listed.values()):
            problems.append(f"{architecture}: {REPOSITORY}.files does not list the same packages as {REPOSITORY}.db")
        for name, entry in entries.items():
            filename = field(entry, "FILENAME")
            match = PACKAGE_FILE.fullmatch(filename)
            if name != PACKAGE or not match or match["version"] != field(entry, "VERSION") or \
                    match["arch"] != architecture or field(entry, "ARCH") != architecture:
                problems.append(f"{architecture}: unexpected database entry {name} {filename}")
                continue
            package = directory / filename
            if not package.is_file():
                problems.append(f"{architecture}: {filename} is missing")
                continue
            data = package.read_bytes()
            if str(len(data)) != field(entry, "CSIZE") or hashlib.sha256(data).hexdigest() != field(entry, "SHA256SUM"):
                problems.append(f"{architecture}: {filename} does not match its database entry")
            signature = directory / f"{filename}.sig"
            if not signature.is_file() or base64.b64encode(signature.read_bytes()).decode() != field(entry, "PGPSIG"):
                problems.append(f"{architecture}: {filename}.sig is missing or not the signature in the database")
        for path in sorted(directory.iterdir()):
            if path.name.startswith(".") or path.name in known:
                continue
            match = PACKAGE_FILE.fullmatch(path.name.removesuffix(".sig"))
            if not match or match["arch"] != architecture or path.is_dir():
                problems.append(f"{architecture}/{path.name} is not part of the repository")
            elif not path.name.endswith(".sig"):
                signature = path.with_name(path.name + ".sig")
                if not signature.is_file():
                    problems.append(f"{architecture}/{path.name} has no signature")
                signatures.append((signature, path))
            elif not path.with_name(path.name.removesuffix(".sig")).is_file():
                problems.append(f"{architecture}/{path.name} has no package")
    if problems:
        raise RepositoryError("; ".join(problems))
    if verifier:
        for signature, data in signatures:
            try:
                verifier.check(signature, data)
            except RepositoryError as error:
                raise RepositoryError(f"{signature.parent.name}/{error}") from None
        if (root / KEY_FILE).read_bytes() != verifier.keyring.read_bytes():
            raise RepositoryError(f"{KEY_FILE} differs from {verifier.keyring}")
    return architectures


def load_profile(path):
    profiles = _load("distribution_profile", "distribution-profile.py")
    resolved, values = profiles.load(path=path)
    values = dict(values)
    values["keyring"] = str(apt.keyring_for(resolved))
    return values


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    compare = commands.add_parser("compare", help="Exit 0 if A < B in pacman's order")
    compare.add_argument("a")
    compare.add_argument("b")
    add = commands.add_parser("stage", help="Copy STAGING to WORK and add packages to WORK, ready for repo-add")
    add.add_argument("--profile", required=True)
    add.add_argument("--staging", required=True)
    add.add_argument("--work", required=True)
    add.add_argument("--added", required=True, help="file that receives ARCH<TAB>FILE<TAB>SIGNED lines")
    add.add_argument("packages", nargs="+", help="PACKAGE=INSPECTED-DIR pairs")
    done = commands.add_parser("finalize", help="Replace repo-add's links with real files")
    done.add_argument("root")
    done.add_argument("architectures", nargs="+", choices=ARCHITECTURES)
    check = commands.add_parser("verify", help="Check a staged repository's databases, packages and signatures")
    check.add_argument("root")
    check.add_argument("--keyring", help="also check every signature with this public key")
    check.add_argument("--fingerprint", help="the key that must have made them")
    args = parser.parse_args(argv)
    try:
        if args.command == "compare":
            return 0 if vercmp(args.a, args.b) < 0 else 1
        if args.command == "stage":
            profile = load_profile(args.profile)
            fingerprint = profile["LINUX_APT_KEY_FINGERPRINT"]
            if profile["DISTRIBUTION"] != "true" or not fingerprint:
                raise RepositoryError(f"the {profile['PROFILE']} profile has no repository signing key; "
                                      "select a distribution profile such as official")
            pairs = [tuple(item.split("=", 1)) for item in args.packages]
            if any(len(pair) != 2 for pair in pairs):
                raise RepositoryError("packages must be PACKAGE=INSPECTED-DIR pairs")
            with Verifier(profile["keyring"], fingerprint) as verifier:
                copy_tree(args.staging, args.work)
                # Build only on a staging directory that verifies as published.
                if any((Path(args.work) / a).is_dir() for a in ARCHITECTURES):
                    verify(args.work, verifier)
                added = stage(args.work, pairs, profile, verifier)
            Path(args.added).write_text("".join(f"{a}\t{n}\t{int(s)}\n" for a, n, s in added))
        elif args.command == "finalize":
            finalize(args.root, args.architectures)
        elif args.command == "verify":
            if args.keyring or args.fingerprint:
                with Verifier(args.keyring or "", args.fingerprint or "") as verifier:
                    architectures = verify(args.root, verifier)
            else:
                architectures = verify(args.root)
            signed = " and signed by " + args.fingerprint if args.keyring else ""
            print(f"verified [{REPOSITORY}]{signed}: {' '.join(architectures) or 'no architectures'}")
    except (apt.RepositoryError, OSError, KeyError) as error:
        print(f"pacman repository: {error}", file=sys.stderr)
        return 1
    except Exception as error:  # the profile parser's ProfileError
        if type(error).__name__ == "ProfileError":
            print(f"Distribution profile: {error}", file=sys.stderr)
            return 1
        raise
    return 0


if __name__ == "__main__":
    sys.exit(main())
