#!/usr/bin/env python3
"""APT repository rules: keyrings, version order, package checks and indexes.

The real-apt end-to-end test is scripts/test-linux-apt.sh."""
import gzip
import hashlib
import importlib.util
from io import StringIO
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("apt_repository", HERE / "apt-repository.py")
apt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(apt)

OFFICIAL_KEY = HERE.parent / "config/profiles/official.apt.asc"
OFFICIAL_FINGERPRINT = "20B087BF1D1B2A04102882F213081979B6CEABDA"
REPOSITORY = "https://updates.example.test/email/apt"


class Versions(unittest.TestCase):
    def test_release_version_order(self):
        # docs/release-versions.md, as dpkg --compare-versions orders them.
        ordered = ["0.1.0", "1:0.1.0~beta.2", "1:0.1.0~beta.10", "1:0.1.0~rc.1", "1:0.1.0", "1:0.1.1~beta.1"]
        for lower, higher in zip(ordered, ordered[1:]):
            self.assertLess(apt.compare_versions(lower, higher), 0, (lower, higher))
            self.assertGreater(apt.compare_versions(higher, lower), 0, (higher, lower))

    def test_dpkg_edge_cases(self):
        self.assertEqual(apt.compare_versions("1.0", "1.00"), 0)
        self.assertLess(apt.compare_versions("1.0~", "1.0"), 0)
        self.assertLess(apt.compare_versions("1.0", "1.0a"), 0)
        self.assertLess(apt.compare_versions("1.0a", "1.0+"), 0)
        self.assertGreater(apt.compare_versions("1:0.1", "9.9"), 0)
        self.assertLess(apt.compare_versions("1.0-1", "1.0-2"), 0)
        self.assertEqual(apt.compare_versions("1:0.1.0~beta.2", "1:0.1.0~beta.2"), 0)

    def test_release_and_debian_forms(self):
        self.assertEqual(apt.release_version("1:0.1.0~beta.2"), "0.1.0-beta.2")
        self.assertEqual(apt.release_version("1:0.2.0"), "0.2.0")
        for bad in ("0.1.0", "1:0.1", "2:0.1.0", "1:0.1.0-1", "1:0.1.0~beta 2"):
            with self.assertRaises(apt.RepositoryError, msg=bad):
                apt.release_version(bad)
        self.assertEqual(apt.suite_for("0.1.0-beta.3"), "beta")
        self.assertEqual(apt.suite_for("0.1.0-rc.1"), "beta")
        self.assertEqual(apt.suite_for("0.1.0"), "stable")
        with self.assertRaises(apt.RepositoryError):
            apt.suite_for("0.1")


class Keyrings(unittest.TestCase):
    def test_committed_official_key(self):
        self.assertEqual(apt.check_keyring(OFFICIAL_KEY, OFFICIAL_FINGERPRINT), [OFFICIAL_FINGERPRINT])
        with self.assertRaises(apt.RepositoryError):
            apt.check_keyring(OFFICIAL_KEY, "0" * 40)
        with self.assertRaises(apt.RepositoryError):
            apt.check_keyring(OFFICIAL_KEY, OFFICIAL_FINGERPRINT.lower())

    def test_secret_key_packets_are_refused(self):
        with tempfile.TemporaryDirectory() as scratch:
            public = apt.dearmor(OFFICIAL_KEY.read_bytes())
            for tag in (5, 7):
                path = Path(scratch) / f"secret-{tag}.gpg"
                # A new-format packet with the secret tag, after the real public key.
                path.write_bytes(public + bytes([0xC0 | tag, 3]) + b"\x04\x00\x00")
                with self.assertRaises(apt.RepositoryError) as caught:
                    apt.keyring_fingerprints(path)
                self.assertIn("secret key material", str(caught.exception))
            path = Path(scratch) / "garbage.asc"
            path.write_text("-----BEGIN PGP PUBLIC KEY BLOCK-----\n\n!!!\n-----END PGP PUBLIC KEY BLOCK-----\n")
            with self.assertRaises(apt.RepositoryError):
                apt.keyring_fingerprints(path)
            # Built at run time, so this file never carries the marker itself.
            kind = "PGP " + "PRIVATE KEY BLOCK"
            path.write_text(f"-----BEGIN {kind}-----\n\nAAAA\n-----END {kind}-----\n")
            with self.assertRaises(apt.RepositoryError):
                apt.keyring_fingerprints(path)

    def test_binary_and_armored_forms_agree(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "key.gpg"
            path.write_bytes(apt.dearmor(OFFICIAL_KEY.read_bytes()))
            self.assertEqual(apt.keyring_fingerprints(path), [OFFICIAL_FINGERPRINT])

    def test_keyring_sits_beside_its_profile(self):
        self.assertEqual(apt.keyring_for("/x/config/profiles/official.env"), Path("/x/config/profiles/official.apt.asc"))


class Repository(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.dir = Path(scratch.name)
        self.keyring = self.dir / "fixture.apt.asc"
        self.keyring.write_bytes(OFFICIAL_KEY.read_bytes())
        self.profile = {"LINUX_APT_REPOSITORY": REPOSITORY, "keyring": str(self.keyring),
                        "LINUX_APP_ID": "com.example.Mail"}
        self.root = self.dir / "repo"
        self.count = 0

    def package(self, version, architecture="arm64", **changes):
        """A stand-in .deb and the files the container would read from it."""
        self.count += 1
        release = apt.release_version(version)
        deb = self.dir / f"in-{self.count}" / f"email-linux_{release}_{architecture}.deb"
        deb.parent.mkdir()
        deb.write_bytes(f"fixture {version} {architecture}".encode())
        inspected = self.dir / f"inspected-{self.count}"
        inspected.mkdir()
        files = {
            "control": f"Package: email-linux\nVersion: {version}\nArchitecture: {architecture}\n"
                       "Maintainer: Email Desktop\nDescription: Keyboard-first desktop email client\n"
                       " A local-first email client.\n",
            "conffiles": f"/etc/apparmor.d/email-linux\n{apt.SOURCES_PATH}\n{apt.KEYRING_PATH}\n",
            "sources": apt.sources_text(REPOSITORY, apt.suite_for(release), architecture),
            "channel": "distribution\n",
            "contents": "./usr/bin/email-linux\n./usr/share/applications/com.example.Mail.desktop\n",
        }
        files.update(changes)
        for name, text in files.items():
            if text is not None:
                (inspected / name).write_text(text)
        if changes.get("keyring", True) is not None:
            (inspected / "keyring").write_bytes(self.keyring.read_bytes())
        if "keyring" in changes and changes["keyring"]:
            (inspected / "keyring").write_text(changes["keyring"])
        return str(deb), str(inspected)

    def add(self, *packages):
        out = StringIO()
        changed = apt.add(self.root, list(packages), self.profile, out=out)
        return changed, out.getvalue()

    def versions(self, suite, architecture="arm64"):
        return [e["Version"] for e in apt.read_index(self.root, suite, architecture)]

    def test_suites_follow_the_release_version(self):
        changed, _ = self.add(self.package("1:0.2.0~beta.1"))
        self.assertEqual(changed, ["beta"])
        self.assertEqual(self.versions("beta"), ["1:0.2.0~beta.1"])
        self.assertFalse((self.root / "dists/stable").exists())
        changed, _ = self.add(self.package("1:0.2.0"), self.package("1:0.2.0", "amd64"))
        self.assertEqual(changed, ["beta", "stable"])
        self.assertEqual(self.versions("stable"), ["1:0.2.0"])
        self.assertEqual(self.versions("stable", "amd64"), ["1:0.2.0"])
        self.assertEqual(self.versions("beta"), ["1:0.2.0~beta.1", "1:0.2.0"])
        self.assertEqual(apt.verify(self.root), ["beta", "stable"])

    def test_release_lists_every_index_with_by_hash_copies(self):
        self.add(self.package("1:0.2.0~beta.1"))
        release = apt.paragraphs((self.root / "dists/beta/Release").read_text())[0]
        self.assertEqual(release["Suite"], "beta")
        self.assertEqual(release["Architectures"], "amd64 arm64")
        self.assertEqual(release["Acquire-By-Hash"], "yes")
        self.assertRegex(release["Date"], r"^(Mon|Tue|Wed|Thu|Fri|Sat|Sun), \d\d [A-Z][a-z]{2} \d{4} \d\d:\d\d:\d\d UTC$")
        names = [line.split()[2] for line in release["SHA256"].strip().split("\n")]
        self.assertEqual(names, ["main/binary-amd64/Packages", "main/binary-amd64/Packages.gz",
                                 "main/binary-arm64/Packages", "main/binary-arm64/Packages.gz"])
        packages = (self.root / "dists/beta/main/binary-arm64/Packages").read_bytes()
        self.assertEqual(gzip.decompress((self.root / "dists/beta/main/binary-arm64/Packages.gz").read_bytes()), packages)
        by_hash = self.root / "dists/beta/main/binary-arm64/by-hash/SHA256" / hashlib.sha256(packages).hexdigest()
        self.assertEqual(by_hash.read_bytes(), packages)
        entry = apt.paragraphs(packages.decode())[0]
        self.assertEqual(entry["Filename"], "pool/main/e/email-linux/email-linux_0.2.0-beta.1_arm64.deb")
        self.assertIn("MD5sum", entry)
        self.assertEqual(entry["Description"], "Keyboard-first desktop email client\n A local-first email client.")
        self.assertEqual((self.root / "dists/beta/main/binary-amd64/Packages").read_bytes(), b"")

    def test_refuses_versions_that_are_not_newer(self):
        self.add(self.package("1:0.2.0~beta.2"))
        for version in ("1:0.2.0~beta.2", "1:0.2.0~beta.1", "1:0.1.0~rc.1"):
            with self.assertRaises(apt.RepositoryError, msg=version) as caught:
                self.add(self.package(version))
            self.assertIn("not newer", str(caught.exception))
        # Another architecture of the same version is a separate package.
        self.add(self.package("1:0.2.0~beta.2", "amd64"))
        self.add(self.package("1:0.2.0"))
        with self.assertRaises(apt.RepositoryError):
            self.add(self.package("1:0.2.0"))

    def test_final_skips_beta_when_beta_is_already_newer(self):
        self.add(self.package("1:0.2.0"))
        self.add(self.package("1:0.3.0~beta.1"))
        changed, out = self.add(self.package("1:0.2.1"))
        self.assertEqual(changed, ["stable"])
        self.assertIn("added to stable only", out)
        self.assertEqual(self.versions("stable"), ["1:0.2.0", "1:0.2.1"])
        self.assertEqual(self.versions("beta"), ["1:0.2.0", "1:0.3.0~beta.1"])

    def test_refuses_builds_from_another_profile_or_channel(self):
        cases = {
            "local-alpha channel": {"channel": "local-alpha\n"},
            "no APT conffiles": {"conffiles": "/etc/apparmor.d/email-linux\n"},
            "no sources file": {"sources": None},
            "another repository": {"sources": apt.sources_text("https://other.example.test/apt", "0.2.0-beta.1", "arm64")},
            "beta package naming stable": {"sources": apt.sources_text(REPOSITORY, "0.2.0", "arm64")},
            "another keyring": {"keyring": "-----BEGIN PGP PUBLIC KEY BLOCK-----\n"},
            "another app ID": {"contents": "./usr/share/applications/invalid.email.Unofficial.desktop\n"},
            "another package": {"control": "Package: other\nVersion: 1:0.2.0~beta.1\nArchitecture: arm64\n"},
        }
        for name, changes in cases.items():
            with self.assertRaises(apt.RepositoryError, msg=name):
                self.add(self.package("1:0.2.0~beta.1", **changes))
        deb, inspected = self.package("1:0.2.0~beta.1")
        renamed = Path(deb).with_name("email-linux_latest_arm64.deb")
        Path(deb).rename(renamed)
        with self.assertRaises(apt.RepositoryError):
            self.add((str(renamed), inspected))
        with self.assertRaises(apt.RepositoryError):
            self.add(self.package("1:0.2.0~beta.1", "armhf"))
        self.assertFalse(self.root.exists(), "a refused package must not touch the repository")

    def test_published_pool_files_never_change(self):
        deb, inspected = self.package("1:0.2.0~beta.1")
        self.add((deb, inspected))
        pool = self.root / apt.POOL / Path(deb).name
        # Same name and version in the pool with other bytes (index rebuilt by hand).
        for suite in ("beta",):
            index = apt.index_dir(self.root, suite, "arm64") / "Packages"
            index.write_text("")
        pool.write_bytes(b"different")
        with self.assertRaises(apt.RepositoryError) as caught:
            self.add((deb, inspected))
        self.assertIn("never change", str(caught.exception))

    def test_verify_detects_tampering(self):
        self.add(self.package("1:0.2.0~beta.1"))
        apt.verify(self.root)
        packages = self.root / "dists/beta/main/binary-arm64/Packages"
        original = packages.read_bytes()
        packages.write_bytes(original.replace(b"Maintainer: Email Desktop", b"Maintainer: Email Desktoq"))
        with self.assertRaises(apt.RepositoryError):
            apt.verify(self.root)
        packages.write_bytes(original)
        pool = next((self.root / apt.POOL).iterdir())
        pool.write_bytes(pool.read_bytes() + b"x")
        with self.assertRaises(apt.RepositoryError):
            apt.verify(self.root)

    def test_working_copy_keeps_earlier_pool_files(self):
        self.add(self.package("1:0.2.0~beta.1"))
        work = self.dir / "work"
        apt.copy_tree(self.root, work)
        apt.add(work, [self.package("1:0.2.0~beta.2")], self.profile, out=StringIO())
        names = sorted(p.name for p in (work / apt.POOL).iterdir())
        self.assertEqual(names, ["email-linux_0.2.0-beta.1_arm64.deb", "email-linux_0.2.0-beta.2_arm64.deb"])
        self.assertEqual(self.versions("beta"), ["1:0.2.0~beta.1"], "the original staging directory is unchanged")


@unittest.skipUnless(shutil.which("gpg") and shutil.which("gpgv"), "GnuPG is not installed")
class Signatures(unittest.TestCase):
    """verify_signatures with throwaway keys, the way publish and upload use it."""

    def setUp(self):
        self.homes = []
        for _ in range(2):
            # GnuPG keeps its agent socket in the home directory; keep the path short.
            home = tempfile.mkdtemp(prefix="email-gpg-", dir="/tmp")
            self.homes.append(home)
            self.addCleanup(shutil.rmtree, home, True)
            self.addCleanup(subprocess.run, ["gpgconf", "--homedir", home, "--kill", "gpg-agent"], capture_output=True)
            subprocess.run(["gpg", "--homedir", home, "--batch", "--quiet", "--passphrase", "",
                            "--quick-generate-key", "Email test, throwaway <test@email-test.invalid>", "ed25519",
                            "sign", "1d"], check=True, capture_output=True)
        self.fingerprints = [self.fingerprint(home) for home in self.homes]
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.dir = Path(scratch.name)
        self.keyring = self.dir / "public.asc"
        self.keyring.write_bytes(subprocess.run(["gpg", "--homedir", self.homes[0], "--batch", "--armor", "--export"],
                                                check=True, capture_output=True).stdout)
        self.base = self.dir / "repo/dists/beta"
        self.base.mkdir(parents=True)
        (self.base / "Release").write_text("Origin: updates.example.test\nSuite: beta\nCodename: beta\n")

    @staticmethod
    def fingerprint(home):
        out = subprocess.run(["gpg", "--homedir", home, "--batch", "--with-colons", "--list-secret-keys"],
                             check=True, capture_output=True, text=True).stdout
        return next(line.split(":")[9] for line in out.splitlines() if line.startswith("fpr:"))

    def sign(self, key=0, release=None):
        release = release or self.base / "Release"
        common = ["gpg", "--homedir", self.homes[key], "--batch", "--yes", "--digest-algo", "SHA512"]
        subprocess.run([*common, "--clearsign", "-o", str(self.base / "InRelease"), str(release)], check=True, capture_output=True)
        subprocess.run([*common, "--armor", "--detach-sign", "-o", str(self.base / "Release.gpg"), str(self.base / "Release")],
                       check=True, capture_output=True)

    def verify(self, expected=None):
        return apt.verify_signatures(self.dir / "repo", self.keyring, expected or self.fingerprints[0])

    def test_valid_signatures(self):
        self.assertEqual(apt.keyring_fingerprints(self.keyring), [self.fingerprints[0]])
        self.sign()
        self.assertEqual(self.verify(), ["beta"])
        with self.assertRaises(apt.RepositoryError):
            self.verify(self.fingerprints[1])

    def test_tampering_and_other_keys_are_refused(self):
        self.sign()
        inrelease = self.base / "InRelease"
        inrelease.write_text(inrelease.read_text().replace("Suite: beta", "Suite: betb"))
        with self.assertRaises(apt.RepositoryError):
            self.verify()
        self.sign()
        release = self.base / "Release"
        release.write_text(release.read_text() + "Date: later\n")
        with self.assertRaises(apt.RepositoryError):
            self.verify()
        self.sign(key=1)
        with self.assertRaises(apt.RepositoryError):
            self.verify()

    def test_inrelease_must_carry_the_release_file(self):
        other = self.dir / "Other"
        other.write_text("Origin: updates.example.test\nSuite: beta\nCodename: beta\nDate: other\n")
        self.sign(release=other)
        with self.assertRaises(apt.RepositoryError) as caught:
            self.verify()
        self.assertIn("does not carry", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
