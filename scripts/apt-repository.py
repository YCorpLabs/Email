#!/usr/bin/env python3
"""APT repository rules for Email's Linux packages (docs/linux-updates.md).

Used by package-linux-deb.sh (the keyring and sources file a distribution .deb
ships), publish-linux-apt.sh (indexes of a staged repository) and the tests.
Standard library only, so it runs on the maintainer's Mac and in the build
container. It never reads or prints private key material: keyrings are parsed
only to refuse secret-key packets and to compute public fingerprints.

Repository layout, relative to the repository URL:

  pool/main/e/email-linux/email-linux_<release>_<arch>.deb
  dists/<suite>/{Release,InRelease,Release.gpg}
  dists/<suite>/main/binary-<arch>/{Packages,Packages.gz}
  dists/<suite>/main/binary-<arch>/by-hash/SHA256/<sha256>

Suites: `beta` carries every release, `stable` only final releases. A
pre-release (a release version with `-`, such as 0.2.0-beta.1) goes to beta; a
final goes to stable, and to beta when it is newer than everything there.
"""

import argparse
import base64
import datetime
import gzip
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

PACKAGE = "email-linux"
ARCHITECTURES = ("amd64", "arm64")
SUITES = ("beta", "stable")
COMPONENT = "main"
POOL = f"pool/{COMPONENT}/e/{PACKAGE}"
KEYRING_PATH = f"/usr/share/keyrings/{PACKAGE}-archive-keyring.asc"
SOURCES_PATH = f"/etc/apt/sources.list.d/{PACKAGE}.sources"
DEB_VERSION = re.compile(r"1:[0-9]+\.[0-9]+\.[0-9]+(?:~[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?")
RELEASE_VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?")
FINGERPRINT = re.compile(r"[0-9A-F]{40}")
INDEX_NAMES = ("Packages", "Packages.gz")


class RepositoryError(ValueError):
    pass


# --- OpenPGP keyrings -------------------------------------------------------

def dearmor(data):
    """Binary packets from an ASCII-armored or binary OpenPGP file."""
    if data[:5] != b"-----":
        return data
    try:
        text = data.decode("ascii")
    except UnicodeDecodeError:
        raise RepositoryError("armored OpenPGP block is not ASCII") from None
    lines = text.replace("\r\n", "\n").split("\n")
    try:
        start = lines.index("-----BEGIN PGP PUBLIC KEY BLOCK-----")
    except ValueError:
        raise RepositoryError("not an armored OpenPGP public key block") from None
    body, index = [], start + 1
    while index < len(lines) and lines[index].strip():  # armor headers
        if ":" not in lines[index]:
            break
        index += 1
    for line in lines[index:]:
        line = line.strip()
        if line.startswith("-----END PGP PUBLIC KEY BLOCK-----"):
            break
        if line and not line.startswith("="):
            body.append(line)
    else:
        raise RepositoryError("unterminated armored OpenPGP block")
    try:
        return base64.b64decode("".join(body), validate=True)
    except ValueError:
        raise RepositoryError("armored OpenPGP block is not valid base64") from None


def packets(data):
    """(tag, body) for each OpenPGP packet (RFC 9580 section 4.2)."""
    index = 0
    while index < len(data):
        header = data[index]
        if not header & 0x80:
            raise RepositoryError("malformed OpenPGP packet header")
        if header & 0x40:  # new format
            tag, index = header & 0x3F, index + 1
            first = data[index]
            if first < 192:
                length, index = first, index + 1
            elif first < 224:
                length, index = ((first - 192) << 8) + data[index + 1] + 192, index + 2
            elif first == 255:
                length, index = int.from_bytes(data[index + 1:index + 5], "big"), index + 5
            else:
                raise RepositoryError("partial-length OpenPGP packets are not expected in a keyring")
        else:  # old format
            tag, kind, index = (header >> 2) & 0x0F, header & 0x03, index + 1
            if kind == 3:
                raise RepositoryError("indeterminate-length OpenPGP packets are not expected in a keyring")
            size = (1, 2, 4)[kind]
            length, index = int.from_bytes(data[index:index + size], "big"), index + size
        body = data[index:index + length]
        if len(body) != length:
            raise RepositoryError("truncated OpenPGP packet")
        yield tag, body
        index += length


def fingerprint(body):
    version = body[0]
    if version == 4:
        return hashlib.sha1(b"\x99" + len(body).to_bytes(2, "big") + body).hexdigest().upper()
    if version in (5, 6):
        prefix = b"\x9a" if version == 5 else b"\x9b"
        return hashlib.sha256(prefix + len(body).to_bytes(4, "big") + body).hexdigest().upper()
    raise RepositoryError(f"unsupported OpenPGP key version {version}")


def keyring_fingerprints(path):
    """Primary key fingerprints of a public keyring. Refuses secret keys."""
    data = dearmor(Path(path).read_bytes())
    primaries = []
    for tag, body in packets(data):
        if tag in (5, 7):
            raise RepositoryError(f"{path} contains secret key material; ship only the public key")
        if tag == 6:
            primaries.append(fingerprint(body))
    if not primaries:
        raise RepositoryError(f"{path} contains no OpenPGP public key")
    return primaries


def check_keyring(path, expected):
    if not FINGERPRINT.fullmatch(expected):
        raise RepositoryError("the signing key fingerprint must be 40 uppercase hexadecimal digits")
    found = keyring_fingerprints(path)
    if expected not in found:
        raise RepositoryError(f"{path} does not contain the signing key {expected}")
    return found


# --- Versions -----------------------------------------------------------------

def _order(char):
    if char.isdigit():
        return 0
    if char.isalpha():
        return ord(char)
    if char == "~":
        return -1
    return ord(char) + 256 if char else 0


def _compare_part(a, b):
    """dpkg's verrevcmp."""
    i = j = 0
    while i < len(a) or j < len(b):
        first_diff = 0
        while (i < len(a) and not a[i].isdigit()) or (j < len(b) and not b[j].isdigit()):
            ac = _order(a[i] if i < len(a) else "")
            bc = _order(b[j] if j < len(b) else "")
            if ac != bc:
                return ac - bc
            i, j = i + 1, j + 1
        while i < len(a) and a[i] == "0":
            i += 1
        while j < len(b) and b[j] == "0":
            j += 1
        while i < len(a) and a[i].isdigit() and j < len(b) and b[j].isdigit():
            if not first_diff:
                first_diff = ord(a[i]) - ord(b[j])
            i, j = i + 1, j + 1
        if i < len(a) and a[i].isdigit():
            return 1
        if j < len(b) and b[j].isdigit():
            return -1
        if first_diff:
            return first_diff
    return 0


def _split(version):
    epoch, _, rest = version.rpartition(":") if ":" in version else ("0", "", version)
    upstream, _, revision = rest.rpartition("-") if "-" in rest else (rest, "", "")
    return int(epoch or "0"), upstream, revision


def compare_versions(a, b):
    """Negative, zero or positive, as `dpkg --compare-versions`."""
    (ea, ua, ra), (eb, ub, rb) = _split(a), _split(b)
    if ea != eb:
        return ea - eb
    return _compare_part(ua, ub) or _compare_part(ra, rb)


def release_version(deb_version):
    """1:0.2.0~beta.1 -> 0.2.0-beta.1, the inverse of email_deb_version."""
    if not DEB_VERSION.fullmatch(deb_version):
        raise RepositoryError(f"{deb_version} is not an Email package version such as 1:0.2.0~beta.1")
    return deb_version[2:].replace("~", "-", 1)


def suite_for(version):
    """The suite a package of this release version names in its sources file."""
    if not RELEASE_VERSION.fullmatch(version):
        raise RepositoryError(f"{version} is not a release version such as 0.2.0 or 0.2.0-beta.1")
    return "beta" if "-" in version else "stable"


# --- deb822 -------------------------------------------------------------------

def paragraphs(text):
    """Fields of each deb822 paragraph, in order. Comments are ignored."""
    result, current, last = [], {}, None
    for line in text.split("\n"):
        if line.startswith("#"):
            continue
        if not line.strip():
            if current:
                result.append(current)
            current, last = {}, None
        elif line[0] in " \t":
            if last is None:
                raise RepositoryError("continuation line without a field")
            current[last] += "\n" + line
        else:
            key, separator, value = line.partition(":")
            if not separator or not key or key in current:
                raise RepositoryError(f"malformed deb822 line: {line[:60]}")
            current[key], last = value.strip(), key
    if current:
        result.append(current)
    return result


def format_paragraph(fields):
    return "".join(f"{key}: {value}\n" for key, value in fields.items())


def sources_text(repository, suite, architecture):
    """/etc/apt/sources.list.d/email-linux.sources for a distribution .deb."""
    return (
        "# Email updates (docs/linux-updates.md). Installed by the email-linux package;\n"
        "# delete this file to stop updates from this repository.\n"
        "Types: deb\n"
        f"URIs: {repository}\n"
        f"Suites: {suite}\n"
        f"Components: {COMPONENT}\n"
        f"Architectures: {architecture}\n"
        f"Signed-By: {KEYRING_PATH}\n"
    )


# --- Package inspection -------------------------------------------------------

def inspect(deb, inspected, profile):
    """Refuse anything but this profile's distribution build of email-linux.

    `inspected` holds what the container read from the .deb with dpkg-deb:
    control, conffiles, contents (dpkg-deb -c), and when present sources,
    keyring and channel (RELEASE-CHANNEL.txt)."""
    deb, inspected = Path(deb), Path(inspected)
    read = lambda name: (inspected / name).read_text() if (inspected / name).is_file() else None
    fields = paragraphs(read("control") or "")
    if len(fields) != 1:
        raise RepositoryError(f"{deb.name}: no control file")
    control = fields[0]
    if control.get("Package") != PACKAGE:
        raise RepositoryError(f"{deb.name}: package {control.get('Package')!r} is not {PACKAGE}")
    version = release_version(control.get("Version", ""))
    architecture = control.get("Architecture", "")
    if architecture not in ARCHITECTURES:
        raise RepositoryError(f"{deb.name}: architecture {architecture!r} is not one of {', '.join(ARCHITECTURES)}")
    expected_name = f"{PACKAGE}_{version}_{architecture}.deb"
    if deb.name != expected_name:
        raise RepositoryError(f"{deb.name}: expected the file name {expected_name}")
    if (read("channel") or "").strip() != "distribution":
        raise RepositoryError(f"{deb.name}: not a distribution build (RELEASE-CHANNEL.txt)")
    conffiles = set((read("conffiles") or "").split())
    if not {SOURCES_PATH, KEYRING_PATH} <= conffiles:
        raise RepositoryError(f"{deb.name}: the sources file and keyring are not conffiles; not built from a distribution profile with an APT repository")
    sources = read("sources")
    expected_sources = sources_text(profile["LINUX_APT_REPOSITORY"], suite_for(version), architecture)
    if sources != expected_sources:
        raise RepositoryError(f"{deb.name}: its sources file does not name {profile['LINUX_APT_REPOSITORY']} "
                              f"suite {suite_for(version)} for {architecture}; built from another profile?")
    keyring = inspected / "keyring"
    if not keyring.is_file() or keyring.read_bytes() != Path(profile["keyring"]).read_bytes():
        raise RepositoryError(f"{deb.name}: its keyring differs from {profile['keyring']}")
    desktop = f"./usr/share/applications/{profile['LINUX_APP_ID']}.desktop"
    if desktop not in (read("contents") or "").split():
        raise RepositoryError(f"{deb.name}: no desktop entry for {profile['LINUX_APP_ID']}; built from another profile?")
    return control, version, architecture


# --- Repository ---------------------------------------------------------------

def digests(data):
    return {"MD5Sum": hashlib.md5(data).hexdigest(), "SHA1": hashlib.sha1(data).hexdigest(),
            "SHA256": hashlib.sha256(data).hexdigest()}


def index_dir(root, suite, architecture):
    return Path(root) / "dists" / suite / COMPONENT / f"binary-{architecture}"


def read_index(root, suite, architecture):
    path = index_dir(root, suite, architecture) / "Packages"
    return paragraphs(path.read_text()) if path.is_file() else []


def newest(entries):
    best = None
    for entry in entries:
        if best is None or compare_versions(entry["Version"], best) > 0:
            best = entry["Version"]
    return best


def write_index(root, suite, architecture, entries):
    entries = sorted(entries, key=_version_key)
    data = "\n".join(format_paragraph(entry) for entry in entries).encode()
    directory = index_dir(root, suite, architecture)
    by_hash = directory / "by-hash" / "SHA256"
    by_hash.mkdir(parents=True, exist_ok=True)
    for name, content in (("Packages", data), ("Packages.gz", gzip.compress(data, mtime=0))):
        (directory / name).write_bytes(content)
        # Clients fetch indexes by hash, so a cached or half-uploaded index
        # never mismatches the InRelease that names it. Old ones are kept.
        (by_hash / hashlib.sha256(content).hexdigest()).write_bytes(content)


class _version_key:
    def __init__(self, entry):
        self.version = entry["Version"]

    def __lt__(self, other):
        return compare_versions(self.version, other.version) < 0


DAYS = ("Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun")
MONTHS = ("Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec")


def rfc1123(moment):
    return (f"{DAYS[moment.weekday()]}, {moment.day:02d} {MONTHS[moment.month - 1]} {moment.year} "
            f"{moment.hour:02d}:{moment.minute:02d}:{moment.second:02d} UTC")


def write_release(root, suite, repository, now=None):
    """dists/<suite>/Release (unsigned). Signing is publish-linux-apt.sh's job."""
    now = now or datetime.datetime.now(datetime.timezone.utc)
    base = Path(root) / "dists" / suite
    files = []
    for architecture in ARCHITECTURES:
        for name in INDEX_NAMES:
            path = index_dir(root, suite, architecture) / name
            files.append((path.relative_to(base).as_posix(), path.read_bytes()))
    host = repository.split("://", 1)[-1].split("/", 1)[0]
    lines = [
        f"Origin: {host}",
        f"Label: {PACKAGE}",
        f"Suite: {suite}",
        f"Codename: {suite}",
        f"Date: {rfc1123(now)}",
        f"Architectures: {' '.join(ARCHITECTURES)}",
        f"Components: {COMPONENT}",
        f"Description: {PACKAGE} packages, {suite} suite",
        "Acquire-By-Hash: yes",
    ]
    for algorithm in ("MD5Sum", "SHA1", "SHA256"):
        lines.append(f"{algorithm}:")
        lines += [f" {digests(data)[algorithm]} {len(data):>16} {name}" for name, data in files]
    (base / "Release").write_text("\n".join(lines) + "\n")


def copy_tree(source, destination):
    """A working copy of the staging directory. Pool files never change once
    published, so they are hard-linked when the filesystem allows."""
    def link_or_copy(src, dst):
        if "/pool/" in Path(src).as_posix():
            try:
                os.link(src, dst)
                return dst
            except OSError:
                pass
        return shutil.copy2(src, dst)
    if Path(source).is_dir():
        shutil.copytree(source, destination, copy_function=link_or_copy, dirs_exist_ok=True)


def add(root, debs, profile, out=sys.stdout):
    """Adds packages to a working copy of the repository. Returns the suites
    whose Release changed, which the caller must sign."""
    root = Path(root)
    plans = []
    for deb, inspected in debs:
        control, version, architecture = inspect(deb, inspected, profile)
        plans.append((Path(deb), control, version, architecture))
    seen = set()
    for _, control, version, architecture in plans:
        key = (control["Version"], architecture)
        if key in seen:
            raise RepositoryError(f"{PACKAGE} {control['Version']} {architecture} is given twice")
        seen.add(key)
    changed = set()
    for deb, control, version, architecture in sorted(plans, key=lambda p: _version_key(p[1])):
        suites = ["beta"] if suite_for(version) == "beta" else ["stable", "beta"]
        targets = []
        for suite in suites:
            current = newest(e for e in read_index(root, suite, architecture))
            if current is not None and compare_versions(control["Version"], current) <= 0:
                if suite == "beta" and "stable" in suites:
                    print(f"note: beta already has {current} for {architecture}, newer than {control['Version']}; "
                          "added to stable only", file=out)
                    continue
                raise RepositoryError(f"{deb.name}: {control['Version']} is not newer than {current}, "
                                      f"already in {suite} for {architecture}")
            targets.append(suite)
        data = deb.read_bytes()
        pool_file = root / POOL / deb.name
        if pool_file.exists() and pool_file.read_bytes() != data:
            raise RepositoryError(f"{pool_file.relative_to(root)} exists with different contents; published files never change")
        pool_file.parent.mkdir(parents=True, exist_ok=True)
        if not pool_file.exists():
            pool_file.write_bytes(data)
        entry = dict(control)
        sums = digests(data)
        entry.update({"Filename": f"{POOL}/{deb.name}", "Size": str(len(data)), "MD5sum": sums["MD5Sum"],
                      "SHA1": sums["SHA1"], "SHA256": sums["SHA256"]})
        for suite in targets:
            entries = read_index(root, suite, architecture) + [entry]
            write_index(root, suite, architecture, entries)
            changed.add(suite)
            print(f"added {control['Version']} {architecture} to {suite}", file=out)
    # The public key, for people who add the repository by hand
    # (docs/linux-updates.md). Packages ship their own copy.
    shutil.copyfile(profile["keyring"], root / f"{PACKAGE}-archive-keyring.asc")
    for suite in sorted(changed):
        # Every listed architecture has an index, empty or not, so apt never
        # reports a missing one.
        for architecture in ARCHITECTURES:
            if not (index_dir(root, suite, architecture) / "Packages").is_file():
                write_index(root, suite, architecture, [])
        write_release(root, suite, profile["LINUX_APT_REPOSITORY"])
    return sorted(changed)


def verify(root):
    """Every index a Release names exists with its size and hashes, and every
    package an index names exists in the pool with its size and SHA-256."""
    root = Path(root)
    problems = []
    suites = [s for s in SUITES if (root / "dists" / s / "Release").is_file()]
    for suite in suites:
        base = root / "dists" / suite
        release = paragraphs((base / "Release").read_text())[0]
        if release.get("Suite") != suite or release.get("Codename") != suite:
            problems.append(f"{suite}: Release names another suite")
        for algorithm in ("MD5Sum", "SHA1", "SHA256"):
            for line in release.get(algorithm, "").split("\n"):
                if not line.strip():
                    continue
                digest, size, name = line.split()
                path = base / name
                if not path.is_file():
                    problems.append(f"{suite}: {name} is missing")
                    continue
                data = path.read_bytes()
                if len(data) != int(size) or digests(data)[algorithm] != digest:
                    problems.append(f"{suite}: {name} does not match its {algorithm} entry")
                elif algorithm == "SHA256":
                    by_hash = path.parent / "by-hash" / "SHA256" / digest
                    if not by_hash.is_file() or by_hash.read_bytes() != data:
                        problems.append(f"{suite}: {name} has no by-hash copy")
        for architecture in ARCHITECTURES:
            for entry in read_index(root, suite, architecture):
                if suite == "stable" and suite_for(release_version(entry["Version"])) != "stable":
                    problems.append(f"stable lists the pre-release {entry['Version']}")
                path = root / entry["Filename"]
                if not path.is_file():
                    problems.append(f"{suite}: {entry['Filename']} is missing")
                    continue
                data = path.read_bytes()
                if len(data) != int(entry["Size"]) or hashlib.sha256(data).hexdigest() != entry["SHA256"]:
                    problems.append(f"{suite}: {entry['Filename']} does not match its index entry")
    if problems:
        raise RepositoryError("; ".join(problems))
    return suites


def verify_signatures(root, keyring, expected):
    """Both signatures of every suite, checked with gpgv against `keyring`
    only, must come from the key `expected`, and InRelease must carry exactly
    the Release file. This is what apt checks with Signed-By."""
    root = Path(root)
    with tempfile.TemporaryDirectory(prefix="email-apt-verify-") as scratch:
        scratch = Path(scratch)
        ring = scratch / "keyring.gpg"
        ring.write_bytes(dearmor(Path(keyring).read_bytes()))
        home = scratch / "home"
        home.mkdir(mode=0o700)
        suites = [s for s in SUITES if (root / "dists" / s / "Release").is_file()]
        for suite in suites:
            base = root / "dists" / suite
            # The signed text goes to standard output (gpgv 2.5 leaves a
            # --output FILE as FILE.part), status lines to their own file.
            for name, extra in (("InRelease", ["--output", "-", str(base / "InRelease")]),
                                ("Release.gpg", [str(base / "Release.gpg"), str(base / "Release")])):
                if not (base / name).is_file():
                    raise RepositoryError(f"{suite}: {name} is missing")
                with open(scratch / "status", "w+") as status:
                    result = subprocess.run(["gpgv", "--homedir", str(home), "--keyring", str(ring),
                                             "--status-fd", str(status.fileno()), *extra],
                                            pass_fds=(status.fileno(),), capture_output=True)
                    status.seek(0)
                    lines = status.read().splitlines()
                valid = [line.split() for line in lines if line.startswith("[GNUPG:] VALIDSIG ")]
                if result.returncode or not valid or any(fields[-1] != expected for fields in valid):
                    raise RepositoryError(f"{suite}: {name} is not a valid signature by {expected}")
                if name == "InRelease" and result.stdout != (base / "Release").read_bytes():
                    raise RepositoryError(f"{suite}: InRelease does not carry the Release file")
        return suites


def load_profile(path):
    import importlib.util
    spec = importlib.util.spec_from_file_location("distribution_profile", Path(__file__).with_name("distribution-profile.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    resolved, values = module.load(path=path)
    values = dict(values)
    values["keyring"] = str(keyring_for(resolved))
    return values


def keyring_for(profile_path):
    """The public key that verifies a profile's repository: beside the profile,
    config/profiles/official.env -> config/profiles/official.apt.asc."""
    path = Path(profile_path)
    return path.with_name(path.name[: -len(".env")] + ".apt.asc")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    key = commands.add_parser("check-keyring", help="Print a public keyring's fingerprints; require one")
    key.add_argument("keyring")
    key.add_argument("fingerprint")
    sources = commands.add_parser("sources", help="Print the sources file a distribution .deb ships")
    sources.add_argument("repository")
    sources.add_argument("version", help="release version, such as 0.2.0-beta.1")
    sources.add_argument("architecture", choices=ARCHITECTURES)
    keyring = commands.add_parser("keyring-path", help="Print the keyring path for a profile file")
    keyring.add_argument("profile")
    compare = commands.add_parser("compare", help="Exit 0 if A < B in dpkg's order")
    compare.add_argument("a")
    compare.add_argument("b")
    stage = commands.add_parser("add", help="Copy STAGING to WORK and add packages to WORK")
    stage.add_argument("--profile", required=True)
    stage.add_argument("--staging", required=True)
    stage.add_argument("--work", required=True)
    stage.add_argument("--changed", required=True, help="file that receives the changed suites")
    stage.add_argument("packages", nargs="+", help="DEB=INSPECTED-DIR pairs")
    check = commands.add_parser("verify", help="Check a staged repository's indexes, pool and signatures")
    check.add_argument("root")
    check.add_argument("--keyring", help="also check both signatures of every suite with this public key")
    check.add_argument("--fingerprint", help="the key that must have made them")
    args = parser.parse_args(argv)
    try:
        if args.command == "check-keyring":
            print("\n".join(check_keyring(args.keyring, args.fingerprint)))
        elif args.command == "sources":
            sys.stdout.write(sources_text(args.repository, suite_for(args.version), args.architecture))
        elif args.command == "keyring-path":
            print(keyring_for(args.profile))
        elif args.command == "compare":
            return 0 if compare_versions(args.a, args.b) < 0 else 1
        elif args.command == "add":
            profile = load_profile(args.profile)
            if profile["DISTRIBUTION"] != "true" or not profile["LINUX_APT_REPOSITORY"]:
                raise RepositoryError(f"the {profile['PROFILE']} profile has no APT repository; select a distribution profile such as official")
            check_keyring(profile["keyring"], profile["LINUX_APT_KEY_FINGERPRINT"])
            pairs = [tuple(item.split("=", 1)) for item in args.packages]
            if any(len(pair) != 2 for pair in pairs):
                raise RepositoryError("packages must be DEB=INSPECTED-DIR pairs")
            copy_tree(args.staging, args.work)
            changed = add(args.work, pairs, profile)
            Path(args.changed).write_text("".join(f"{suite}\n" for suite in changed))
        elif args.command == "verify":
            suites = verify(args.root)
            if args.keyring or args.fingerprint:
                check_keyring(args.keyring or "", args.fingerprint or "")
                verify_signatures(args.root, args.keyring, args.fingerprint)
            signed = " and signed by " + args.fingerprint if args.keyring else ""
            print(f"verified suites{signed}: {' '.join(suites) or 'none'}")
    except (RepositoryError, OSError, KeyError) as error:
        print(f"APT repository: {error}", file=sys.stderr)
        return 1
    except Exception as error:  # the profile parser's ProfileError
        if type(error).__name__ == "ProfileError":
            print(f"Distribution profile: {error}", file=sys.stderr)
            return 1
        raise
    return 0


if __name__ == "__main__":
    sys.exit(main())
