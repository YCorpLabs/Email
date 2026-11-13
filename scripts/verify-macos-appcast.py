#!/usr/bin/env python3
"""Verify an Email macOS appcast before it is uploaded.

Checks, without any private key:
- the embedded Sparkle feed signature (EdDSA over the signed byte range);
- every item has an integer sparkle:version, unique and newer than --min-build;
- channels are the default (stable) channel or "testing";
- enclosures carry an EdDSA signature and length, and in production use HTTPS
  on an approved host;
- with --archives, each referenced archive exists, its length matches and its
  EdDSA signature verifies against the public key.

Exits nonzero on the first class of failure, printing every problem found.
"""

import argparse
import base64
import re
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
from pathlib import Path
from urllib.parse import urlparse

SPARKLE_NS = "http://www.andymatuschak.org/xml-namespaces/sparkle"
CHANNELS = {None, "testing"}
# DER SubjectPublicKeyInfo prefix for a raw 32-byte Ed25519 key.
ED25519_SPKI_PREFIX = bytes.fromhex("302a300506032b6570032100")
SIGNATURE_BLOCK = re.compile(rb"<!-- sparkle-signatures:\s*\nedSignature: (\S+)\s*\nlength: (\d+)\s*\n-->\s*$")


def ed25519_verify(public_key: bytes, message: bytes, signature: bytes) -> bool:
    with tempfile.TemporaryDirectory(prefix="email-appcast-") as scratch:
        root = Path(scratch)
        der = root / "public.der"
        der.write_bytes(ED25519_SPKI_PREFIX + public_key)
        pem = root / "public.pem"
        subprocess.run(["openssl", "pkey", "-pubin", "-inform", "DER", "-in", der, "-out", pem],
                       check=True, capture_output=True)
        (root / "message").write_bytes(message)
        (root / "signature").write_bytes(signature)
        result = subprocess.run(["openssl", "pkeyutl", "-verify", "-pubin", "-inkey", pem, "-rawin",
                                 "-in", root / "message", "-sigfile", root / "signature"],
                                capture_output=True)
        return result.returncode == 0


def parse_build(text):
    if text and re.fullmatch(r"[1-9][0-9]{0,8}", text):
        return int(text)
    return None


def verify(appcast: Path, public_key_text: str, production: set[str] | bool, min_build: int, archives: Path | None,
           require_channel: str | None) -> list[str]:
    problems = []
    try:
        public_key = base64.b64decode(public_key_text.strip(), validate=True)
    except ValueError:
        return ["public key is not base64"]
    if len(public_key) != 32:
        return ["public key is not 32 bytes"]
    raw = appcast.read_bytes()
    match = SIGNATURE_BLOCK.search(raw)
    if not match:
        return ["feed has no Sparkle signature block"]
    signed_length = int(match.group(2))
    if signed_length > match.start():
        problems.append("feed signature length extends past the signed content")
    elif not ed25519_verify(public_key, raw[:signed_length], base64.b64decode(match.group(1))):
        problems.append("feed signature does not verify")
    if raw[signed_length:match.start()].strip():
        problems.append("feed has unsigned content before the signature block")
    try:
        root = ET.fromstring(raw[:signed_length])
    except ET.ParseError as error:
        return problems + [f"feed is not valid XML: {error}"]
    items = root.findall("./channel/item")
    if not items:
        problems.append("feed has no items")
    seen = set()
    for index, item in enumerate(items):
        label = f"item {index + 1}"
        enclosure = item.find("enclosure")
        version_text = item.findtext(f"{{{SPARKLE_NS}}}version")
        if version_text is None and enclosure is not None:
            version_text = enclosure.get(f"{{{SPARKLE_NS}}}version")
        build = parse_build(version_text)
        if build is None:
            problems.append(f"{label}: sparkle:version {version_text!r} is not a positive integer")
        else:
            label = f"build {build}"
            if build in seen:
                problems.append(f"{label}: duplicate build number")
            seen.add(build)
            if build <= min_build:
                problems.append(f"{label}: not newer than minimum build {min_build}")
        channel = item.findtext(f"{{{SPARKLE_NS}}}channel")
        if channel not in CHANNELS:
            problems.append(f"{label}: unknown channel {channel!r}")
        if enclosure is None:
            problems.append(f"{label}: no enclosure")
            continue
        url = enclosure.get("url", "")
        parsed = urlparse(url)
        hosts = production if isinstance(production, set) else set()
        if production and (parsed.scheme != "https" or parsed.hostname not in hosts or parsed.port not in (None, 443)):
            problems.append(f"{label}: enclosure {url} is not HTTPS on an approved host")
        signature = enclosure.get(f"{{{SPARKLE_NS}}}edSignature")
        length = enclosure.get("length")
        if not signature or not length or not length.isdigit():
            problems.append(f"{label}: enclosure lacks EdDSA signature or length")
            continue
        if archives is not None:
            archive = archives / Path(parsed.path).name
            if not archive.is_file():
                problems.append(f"{label}: archive {archive.name} is missing")
            elif archive.stat().st_size != int(length):
                problems.append(f"{label}: archive length differs from the feed")
            elif not ed25519_verify(public_key, archive.read_bytes(), base64.b64decode(signature)):
                problems.append(f"{label}: archive EdDSA signature does not verify")
    if require_channel is not None and items:
        newest = max(items, key=lambda item: parse_build(item.findtext(f"{{{SPARKLE_NS}}}version")) or 0)
        if (newest.findtext(f"{{{SPARKLE_NS}}}channel") or "stable") != require_channel:
            problems.append(f"newest item is not on the {require_channel} channel")
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("appcast", type=Path)
    parser.add_argument("--public-key", required=True, help="SUPublicEDKey value or @file")
    parser.add_argument("--production", metavar="HOSTS",
                        help="Space-separated update hosts from the distribution profile (UPDATE_ALLOWED_HOSTS)")
    parser.add_argument("--min-build", type=int, default=0)
    parser.add_argument("--archives", type=Path)
    parser.add_argument("--newest-channel", choices=["stable", "testing"])
    args = parser.parse_args()
    key = args.public_key
    if key.startswith("@"):
        key = Path(key[1:]).read_text()
    if args.production is not None and not args.production.split():
        parser.error("--production needs the distribution profile's UPDATE_ALLOWED_HOSTS")
    production = set(args.production.split()) if args.production is not None else False
    problems = verify(args.appcast, key, production, args.min_build, args.archives, args.newest_channel)
    for problem in problems:
        print(f"FAIL {problem}")
    if problems:
        sys.exit(1)
    print(f"Appcast verified: {args.appcast}")


if __name__ == "__main__":
    main()
