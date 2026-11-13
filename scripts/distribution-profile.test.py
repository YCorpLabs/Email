#!/usr/bin/env python3
"""Distribution profile resolver: selection, strict parsing, OAuth precedence."""
import hashlib
import importlib.util
import re
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("distribution_profile", HERE / "distribution-profile.py")
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


class DistributionProfileTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        shutil.copytree(HERE.parent / "config/profiles", self.root / "config/profiles")
        self.base = (self.root / "config/profiles/unofficial.env").read_text()

    def run_cli(self, *args, **env):
        environment = {k: v for k, v in os.environ.items() if not k.startswith("EMAIL_")}
        environment.update(env)
        return subprocess.run([sys.executable, str(HERE / "distribution-profile.py"), "--root", str(self.root), *args],
                              env=environment, capture_output=True, text=True)

    def test_tracked_profiles_and_frozen_identities(self):
        development = profile.named("development", HERE.parent)
        self.assertEqual(development["MACOS_BUNDLE_ID"], "dev.email.client")
        self.assertEqual(development["KEYCHAIN_NAMESPACE"], "email.native.mail")
        self.assertEqual(development["MACOS_DATA_DIRECTORY"], "Email")
        self.assertEqual(development["LINUX_DATA_DIRECTORY"], "email-client")
        self.assertEqual(development["LINUX_APP_ID"], "dev.email.client")
        official = profile.named("official", HERE.parent)
        self.assertEqual(official["MACOS_BUNDLE_ID"], "com.ycorplabs.mail")
        self.assertEqual(official["LINUX_APP_ID"], "com.ycorplabs.Mail")
        self.assertEqual(official["DISTRIBUTION"], "true")
        neutral = profile.named("unofficial", HERE.parent)
        for key in ("SUPPORT_ADDRESS", "FEEDBACK_ADDRESS", "SCHEDULING_ORIGIN", "UPDATE_ALLOWED_HOSTS"):
            self.assertEqual(neutral[key], "", key)
        self.assertEqual(neutral["OPEN_TRACKING"], "false")
        identity = ("MACOS_BUNDLE_ID", "MACOS_DATA_DIRECTORY", "KEYCHAIN_NAMESPACE", "LINUX_APP_ID",
                    "LINUX_DATA_DIRECTORY", "LINUX_SECRET_COLLECTION", "LINUX_DISPATCH_UNIT")
        for key in identity:
            values = [p[key].lower() for p in (development, official, neutral)]
            self.assertEqual(len(set(values)), 3, key)

    def test_selection_precedence(self):
        self.assertEqual(profile.selected(self.root, {}).name, "unofficial.env")
        (self.root / profile.SELECTION_FILE).write_text("development\n")
        self.assertEqual(profile.selected(self.root, {}).name, "development.env")
        self.assertEqual(profile.selected(self.root, {"EMAIL_DISTRIBUTION_PROFILE": "official"}).name, "official.env")
        fork = self.root / "fork.env"
        fork.write_text(self.base.replace("PROFILE=unofficial", "PROFILE=acme"))
        self.assertEqual(profile.load(self.root, {"EMAIL_DISTRIBUTION_PROFILE": str(fork)})[1]["PROFILE"], "acme")
        for bad in ("missing", "../outside", "a/b"):
            with self.assertRaises(profile.ProfileError):
                profile.selected(self.root, {"EMAIL_DISTRIBUTION_PROFILE": bad})

    def test_local_oauth_requires_an_explicit_selection(self):
        (self.root / profile.OAUTH_FILE).write_text("EMAIL_GOOGLE_CLIENT_SECRET=fixture\n")
        refused = self.run_cli("get", "MACOS_BUNDLE_ID")
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn(profile.SELECTION_FILE, refused.stderr)
        self.assertNotIn("fixture", refused.stderr)
        self.assertNotEqual(self.run_cli("shell").returncode, 0)
        self.assertEqual(self.run_cli("get", "MACOS_BUNDLE_ID", "--profile", "official").stdout.strip(), "com.ycorplabs.mail")
        self.assertEqual(self.run_cli("get", "MACOS_BUNDLE_ID", EMAIL_DISTRIBUTION_PROFILE="development").returncode, 0)
        (self.root / profile.SELECTION_FILE).write_text("unofficial\n")
        self.assertEqual(self.run_cli("shell").returncode, 0)

    def test_bare_carriage_returns_are_refused(self):
        with self.assertRaises(profile.ProfileError):
            profile.parse(self.base.replace("\n", "\r"), "fixture")
        profile.parse(self.base.replace("\n", "\r\n"), "fixture")

    def test_oauth_values_keep_hash_characters(self):
        (self.root / profile.OAUTH_FILE).write_text(
            "EMAIL_GOOGLE_CLIENT_SECRET=abc#def  # trailing comment\n# full comment\n")
        self.assertEqual(profile.read_oauth_file(self.root)["EMAIL_GOOGLE_CLIENT_SECRET"], "abc#def")

    def test_unreadable_selection_never_falls_back(self):
        selection = self.root / profile.SELECTION_FILE
        selection.write_bytes(b"\xff\xfe")
        with self.assertRaises(profile.ProfileError):
            profile.selected(self.root, {})

    def test_catalog_profile_name_must_match(self):
        (self.root / "config/profiles/other.env").write_text(self.base)
        with self.assertRaises(profile.ProfileError):
            profile.load(self.root, {"EMAIL_DISTRIBUTION_PROFILE": "other"})

    def test_rejects_malformed_profiles(self):
        for old, new in [
            ("SUPPORT_ADDRESS=", "SUPPORT_ADDRESS=help@example.test"),
            ("SCHEDULING_ORIGIN=", "SCHEDULING_ORIGIN=http://example.test"),
            ("SCHEDULING_ORIGIN=", "SCHEDULING_ORIGIN=https://example.test/path"),
            ("MACOS_BUNDLE_ID=invalid.email.unofficial", "MACOS_BUNDLE_ID=email"),
            ("LINUX_APP_ID=invalid.email.Unofficial", "LINUX_APP_ID=invalid.1email.App"),
            ("MACOS_DATA_DIRECTORY=invalid.email.unofficial", "MACOS_DATA_DIRECTORY=../Email"),
            ("OPEN_TRACKING=false", "OPEN_TRACKING=yes"),
            ("GOOGLE_OAUTH_PUBLIC=true", "GOOGLE_OAUTH_PUBLIC=1"),
            ("UPDATE_FEED_URL=", "UPDATE_FEED_URL=https://updates.example.test/appcast.xml"),
            ("PROFILE=unofficial", "PROFILE=unofficial\nPROFILE=again"),
            ("PROFILE=unofficial", "UNKNOWN=value\nPROFILE=unofficial"),
            ("PROFILE=unofficial", "export PROFILE=unofficial"),
            ("OPEN_TRACKING=false\n", ""),
            ("PROFILE=unofficial", "PROFILE=unofficial\x1c"),
        ]:
            self.assertIn(old, self.base)
            with self.assertRaises(profile.ProfileError, msg=new):
                profile.parse(self.base.replace(old, new, 1), "fixture")
        both = self.base.replace("SUPPORT_ADDRESS=", "SUPPORT_ADDRESS=help@example.test").replace(
            "FEEDBACK_ADDRESS=", "FEEDBACK_ADDRESS=ideas@example.test")
        profile.parse(both, "fixture")
        key = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE="
        updates = self.base.replace("UPDATE_FEED_URL=", "UPDATE_FEED_URL=https://updates.example.test/feed.xml").replace(
            "UPDATE_PUBLIC_ED_KEY=", f"UPDATE_PUBLIC_ED_KEY={key}")
        with self.assertRaises(profile.ProfileError):
            profile.parse(updates, "fixture")
        allowed = updates.replace("UPDATE_ALLOWED_HOSTS=", "UPDATE_ALLOWED_HOSTS=updates.example.test")
        profile.parse(allowed, "fixture")
        # Same refusals as the Rust parser.
        for old, new in [("/feed.xml", ""), ("/feed.xml", "/feed.xml#fragment"), ("https://updates", "https://user@updates"),
                         ("example.test/feed", "example.test:8443/feed"), ("/feed.xml", "/feed.xml#"),
                         ("https://updates", "https://@updates"), ("example.test/feed", "example.test:0443/feed"),
                         ("example.test/feed", "example.test:abc/feed")]:
            with self.assertRaises(profile.ProfileError, msg=new):
                profile.parse(allowed.replace(old, new, 1), "fixture")
        profile.parse(allowed.replace("example.test/feed", "example.test:443/feed", 1), "fixture")
        zero = updates.replace(key, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").replace(
            "UPDATE_ALLOWED_HOSTS=", "UPDATE_ALLOWED_HOSTS=updates.example.test")
        with self.assertRaises(profile.ProfileError):
            profile.parse(zero, "fixture")

    def test_shell_output_quotes_and_prefers_environment(self):
        (self.root / profile.OAUTH_FILE).write_text(
            "# comment\nexport EMAIL_GOOGLE_CLIENT_ID='1-local.apps.googleusercontent.com'\n"
            'EMAIL_GOOGLE_CLIENT_SECRET="fixture secret; $(not run)"\n')
        result = self.run_cli("shell", EMAIL_DISTRIBUTION_PROFILE="development",
                              EMAIL_GOOGLE_CLIENT_ID="2-env.apps.googleusercontent.com")
        self.assertEqual(result.returncode, 0, result.stderr)
        check = subprocess.run(["bash", "-c", result.stdout + '\nprintf "%s|%s|%s" "$EMAIL_GOOGLE_CLIENT_ID" "$EMAIL_GOOGLE_CLIENT_SECRET" "$EMAIL_PROFILE_MACOS_BUNDLE_ID"'],
                               capture_output=True, text=True)
        self.assertEqual(check.stdout, "2-env.apps.googleusercontent.com|fixture secret; $(not run)|dev.email.client")

    def test_google_client_requirements(self):
        missing = self.run_cli("check-google-client", EMAIL_DISTRIBUTION_PROFILE="development")
        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("EMAIL_GOOGLE_CLIENT_ID", missing.stderr)
        self.assertIn("config/oauth-desktop.local.env", missing.stderr)
        self.assertEqual(self.run_cli("check-google-client").returncode, 0)
        malformed = self.run_cli("check-google-client", EMAIL_GOOGLE_CLIENT_ID="secret-looking-value")
        self.assertNotEqual(malformed.returncode, 0)
        self.assertNotIn("secret-looking-value", malformed.stderr)
        self.assertEqual(self.run_cli("check-google-client", EMAIL_DISTRIBUTION_PROFILE="official",
                                      EMAIL_GOOGLE_CLIENT_ID="1-x.apps.googleusercontent.com").returncode, 0)
        (self.root / profile.OAUTH_FILE).write_text("EMAIL_GOOGLE_CLIENT_ID=1-local.apps.googleusercontent.com\n")
        self.assertEqual(self.run_cli("check-google-client", EMAIL_DISTRIBUTION_PROFILE="official").returncode, 0)

    def test_development_profile_is_frozen_to_its_client(self):
        refused = self.run_cli("check-google-client", EMAIL_DISTRIBUTION_PROFILE="development",
                               EMAIL_GOOGLE_CLIENT_ID="2-other.apps.googleusercontent.com")
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("frozen", refused.stderr)
        self.assertNotIn("2-other", refused.stderr)
        self.assertEqual(self.run_cli("check-google-client", EMAIL_DISTRIBUTION_PROFILE="unofficial",
                                      EMAIL_GOOGLE_CLIENT_ID="2-other.apps.googleusercontent.com").returncode, 0)

    def test_release_client_refuses_the_development_client_by_digest(self):
        fixture = "1-development.apps.googleusercontent.com"
        original = profile.DEVELOPMENT_GOOGLE_CLIENT_SHA256
        profile.DEVELOPMENT_GOOGLE_CLIENT_SHA256 = frozenset({hashlib.sha256(fixture.encode()).hexdigest()})
        try:
            self.assertTrue(profile.is_development_google_client(fixture))
            self.assertFalse(profile.is_development_google_client("2-prod.apps.googleusercontent.com"))
        finally:
            profile.DEVELOPMENT_GOOGLE_CLIENT_SHA256 = original
        # A release machine may keep its production client in the local file.
        (self.root / profile.OAUTH_FILE).write_text("EMAIL_GOOGLE_CLIENT_ID=2-prod.apps.googleusercontent.com\n")
        (self.root / profile.SELECTION_FILE).write_text("official\n")
        self.assertEqual(self.run_cli("check-release-google-client",
                                      EMAIL_RELEASE_GOOGLE_CLIENT_ID="2-prod.apps.googleusercontent.com").returncode, 0)
        refused = self.run_cli("check-release-google-client", EMAIL_RELEASE_GOOGLE_CLIENT_ID="not-a-client")
        self.assertNotEqual(refused.returncode, 0)
        self.assertEqual(len(profile.DEVELOPMENT_GOOGLE_CLIENT_SHA256), 1)
        self.assertTrue(all(len(d) == 64 for d in profile.DEVELOPMENT_GOOGLE_CLIENT_SHA256))

    def test_shared_conformance_cases(self):
        cases = (HERE.parent / "crates/distribution-profile/conformance.tsv").read_text().splitlines()
        count = 0
        for line in cases:
            if not line or line.startswith("#"):
                continue
            outcome, *edits = line.split("\t")
            text = self.base
            for edit in edits:
                key = edit.split("=", 1)[0]
                text = re.sub(rf"(?m)^{key}=.*$", lambda _m: edit, text, count=1)
            with self.subTest(case=line):
                if outcome == "ok":
                    profile.parse(text, "case")
                else:
                    with self.assertRaises(profile.ProfileError):
                        profile.parse(text, "case")
            count += 1
        self.assertGreater(count, 40)

    def test_open_tracking_is_development_only(self):
        values = {name: profile.parse((self.root / f"config/profiles/{name}.env").read_text(), name)
                  for name in ("development", "official", "unofficial")}
        self.assertEqual({name: v["OPEN_TRACKING"] for name, v in values.items()},
                         {"development": "true", "official": "false", "unofficial": "false"})

    def test_support_notice_text_is_identical_in_both_shells(self):
        swift = (HERE.parent / "apps/macos/Sources/EmailUI/DistributionProfile.swift").read_text()
        rust = (HERE.parent / "apps/linux/src/support.rs").read_text()
        mac = re.search(r'static let supportUnavailableMessage = "(.*)"\n', swift).group(1)
        linux = re.search(r'const SUPPORT_UNAVAILABLE: &str = "(.*)";\n', rust).group(1)
        self.assertEqual(mac, linux)

    def test_python_and_rust_parsers_share_rules(self):
        rust = (HERE.parent / "crates/distribution-profile/src/lib.rs").read_text()
        for key in profile.RULES:
            self.assertIn(f'"{key}"', rust)
        self.assertIn(f'SELECTION_FILE: &str = "{profile.SELECTION_FILE}"', rust)


if __name__ == "__main__":
    unittest.main()
