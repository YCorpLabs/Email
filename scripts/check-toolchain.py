#!/usr/bin/env python3
"""Check that every build uses the Rust version pinned in rust-toolchain.toml."""
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
# Documents that state the pinned version for readers.
DOCUMENTS = ('docs/building.md',)


def pinned_version(root):
    text = (root / 'rust-toolchain.toml').read_text()
    match = re.search(r'^channel = "(\d+\.\d+\.\d+)"$', text, re.MULTILINE)
    if not match:
        raise ValueError('rust-toolchain.toml must pin an exact channel, for example "1.99.0".')
    return match.group(1)


def failures(root):
    try:
        version = pinned_version(root)
    except (OSError, ValueError) as error:
        return [str(error)]
    found = []
    dockerfile = (root / 'apps/linux/Dockerfile.verify').read_text()
    if not re.search(rf'^ARG RUST_VERSION={re.escape(version)}$', dockerfile, re.MULTILINE):
        found.append(f'apps/linux/Dockerfile.verify must default RUST_VERSION to {version}.')
    # The image must already hold the pinned components, or every container run
    # would download them again.
    listed = re.search(r'^components = \[(.*)\]$', (root / 'rust-toolchain.toml').read_text(), re.MULTILINE)
    for component in re.findall(r'"([a-z-]+)"', listed.group(1) if listed else ''):
        if f'-c {component}' not in dockerfile:
            found.append(f'apps/linux/Dockerfile.verify must install the {component} component.')
    workflow = (root / '.github/workflows/validate.yml').read_text()
    # CI installs the toolchain named by rust-toolchain.toml instead of its own copy.
    installs = re.findall(r'rustup toolchain install([^\n]*)', workflow)
    if (re.search(r'RUST_VERSION|RUSTUP_TOOLCHAIN|rustup (override|default)\b|cargo \+', workflow)
            or any(arguments.strip() for arguments in installs)):
        found.append('validate.yml must install Rust with a bare `rustup toolchain install`.')
    if 'rustup toolchain install' not in workflow:
        found.append('validate.yml must install the pinned Rust toolchain.')
    for name in DOCUMENTS:
        path = root / name
        if not path.exists():
            continue
        text = path.read_text()
        stated_versions = re.findall(r"\bRust (\d+\.\d+\.\d+)\b|toolchain install (\d+\.\d+\.\d+)|toolchain '(\d+\.\d+\.\d+)-", text)
        for stated in (next(v for v in match if v) for match in stated_versions):
            if stated != version:
                found.append(f'{name} states Rust {stated}; rust-toolchain.toml pins {version}.')
    return found


def main():
    if sys.argv[1:] == ['--print-version']:
        try:
            print(pinned_version(ROOT))
        except (OSError, ValueError) as error:
            print(error, file=sys.stderr)
            return 1
        return 0
    if sys.argv[1:]:
        print('Usage: python3 scripts/check-toolchain.py [--print-version]', file=sys.stderr)
        return 2
    problems = failures(ROOT)
    if problems:
        print('Toolchain check FAILED:', file=sys.stderr)
        for problem in problems:
            print(f'- {problem}', file=sys.stderr)
        return 1
    print(f'Toolchain check passed: Rust {pinned_version(ROOT)} everywhere.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
