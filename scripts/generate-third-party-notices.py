#!/usr/bin/env python3
"""Collect locked Cargo license documents without including local build paths."""

from pathlib import Path
import argparse
import hashlib
import json
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
LOCKS = ("Cargo.lock", "apps/linux/Cargo.lock", "apps/macos/Package.resolved")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def swift_section():
    pins = json.loads((ROOT / "apps/macos/Package.resolved").read_text())["pins"]
    notices = json.loads((ROOT / "licenses/swift-notices.json").read_text())
    by_identity = {notice["identity"]: notice for notice in notices}
    if {pin["identity"] for pin in pins} != set(by_identity):
        raise ValueError("Swift dependencies need an exact notice inventory")
    lines = ["## Bundled macOS update framework", "",
             "macOS release bundles include Sparkle. Its complete upstream notice,",
             "including notices for its bundled components, is reproduced below.", ""]
    for pin in pins:
        notice = by_identity[pin["identity"]]
        if (pin["state"]["version"] != notice["version"] or
                pin["state"]["revision"] != notice["revision"] or
                pin["location"] != notice["source"]):
            raise ValueError("Swift dependency changed; review its upstream notices")
        path = ROOT / notice["license_file"]
        if digest(path) != notice["sha256"]:
            raise ValueError("Swift license document hash changed; review upstream")
        lines.extend([f"### {pin['identity']} {notice['version']}", "",
                      f"Source: {notice['source']}/tree/{notice['revision']}", "",
                      "~~~~", path.read_text().rstrip(), "~~~~", ""])
    return lines


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Check recorded lock identities offline")
    args = parser.parse_args()
    output = ROOT / "THIRD_PARTY_NOTICES.md"
    identities = [f"- `{name}`: `{digest(ROOT / name)}`" for name in LOCKS]
    if args.check:
        text = output.read_text()
        if not all(line in text for line in identities + swift_section()):
            parser.exit(1, "Notices are stale: regenerate after reviewing lockfile changes.\n")
        print("Third-party notice lock identities match")
        return

    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    packages = {}
    for manifest in ("Cargo.toml", "apps/linux/Cargo.toml"):
        result = subprocess.check_output([cargo, "metadata", "--locked", "--format-version", "1",
                                          "--manifest-path", str(ROOT / manifest)], cwd=ROOT)
        for package in json.loads(result)["packages"]:
            if package.get("source"):
                packages[(package["name"], package["version"])] = package

    supplemental = json.loads((ROOT / "licenses/upstream-notices.json").read_text())
    documents = {}
    rows = []
    for (name, version), package in sorted(packages.items()):
        directory = Path(package["manifest_path"]).parent
        notices = []
        for path in sorted(directory.iterdir()):
            if path.is_file() and path.name.lower().startswith(
                    ("license", "copying", "notice", "unlicense", "copyright", "authors")):
                notices.append((path.name, path.read_text()))
        extra = [item for item in supplemental if item["package"] == name]
        notices.extend((item["name"], item["text"]) for item in extra)
        if not notices:
            parser.exit(1, f"No license document collected for {name} {version}; review upstream.\n")
        refs = []
        for filename, text in notices:
            key = hashlib.sha256(text.encode()).hexdigest()[:16]
            documents[key] = text
            refs.append(f"[{filename}](#notice-{key})")
        source = f"https://crates.io/api/v1/crates/{name}/{version}/download"
        rows.append(f"| [{name} {version}]({source}) | {package.get('license') or 'REVIEW REQUIRED'} | "
                    + ", ".join(refs) + " |")

    lines = ["# Third-party notices", "",
             "Email's original source is MIT licensed; third-party code retains its own terms.", "",
             "This conservative inventory covers both complete Cargo lock resolutions, including build, test",
             "and other-platform packages; it is broader than the libraries linked into one native artifact.",
             "Package names link to exact source archives. License alternatives are reproduced as declared,",
             "without claiming every alternative applies simultaneously. For MPL-covered dependencies,",
             "the linked exact source archives provide the corresponding unmodified dependency source.", "",
             "System GTK, WebKitGTK, platform frameworks and other system libraries are not bundled here;",
             "their distributors provide their applicable license notices and source. A release still needs",
             "an artifact-specific redistribution review, including any modifications to dependency sources.", "",
             "## Source identity", "", *identities, "",
             "Regenerate with `python3 scripts/generate-third-party-notices.py`; check lock identities with `--check`.", "",
             *swift_section(), "",
             "## Rust dependency inventory", "", "| Package / exact source | Declared license | Included documents |",
             "| --- | --- | --- |", *rows, "", "## Upstream license supplements", ""]
    lines.extend(f"- `{item['package']}` / `{item['name']}`: {item['url']}" for item in supplemental)
    lines.extend(["", "## License documents", ""])
    for key, text in sorted(documents.items()):
        fence = "~" * max(4, max((len(s) for s in text.splitlines() if s and set(s) == {"~"}), default=0) + 1)
        normalized = "\n".join(line.rstrip() for line in text.splitlines()).rstrip()
        lines.extend([f'<a id="notice-{key}"></a>', "", f"### Notice {key}", "", fence, normalized, fence, ""])
    output.write_text("\n".join(lines))
    print(f"Collected notices for {len(packages)} packages; {len(documents)} distinct documents")


if __name__ == "__main__":
    main()
