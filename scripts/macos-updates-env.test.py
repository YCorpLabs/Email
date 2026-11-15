#!/usr/bin/env python3
"""Update feed resolution for each macOS packaging mode (macos-updates-env.sh).

The official profile records the production appcast. --distribute must ship it
and refuse a different one; an unnotarized --beta must never ship a feed.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
OFFICIAL = dict(line.split("=", 1) for line in (HERE.parent / "config/profiles/official.env").read_text().splitlines()
                if "=" in line and not line.startswith("#"))
OFFICIAL_FEED = OFFICIAL["UPDATE_FEED_URL"]
OFFICIAL_KEY = OFFICIAL["UPDATE_PUBLIC_ED_KEY"]
OTHER_KEY = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE="


class UpdateFeedResolution(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        # Only the profile resolver and the catalog: no local OAuth file or selection.
        self.root = Path(scratch.name)
        (self.root / "scripts").mkdir()
        for name in ("distribution-profile.py", "distribution-profile.sh", "macos-updates-env.sh"):
            shutil.copy(HERE / name, self.root / "scripts" / name)
        shutil.copytree(HERE.parent / "config/profiles", self.root / "config/profiles")

    def resolve(self, profile, mode, **env):
        environment = {k: v for k, v in os.environ.items() if not k.startswith("EMAIL_")}
        environment.update(env, EMAIL_DISTRIBUTION_PROFILE=profile)
        script = (
            'set -euo pipefail\n'
            'source "$1/scripts/distribution-profile.sh"\n'
            'source "$1/scripts/macos-updates-env.sh"\n'
            'email_load_distribution_profile "$1"\n'
            'email_resolve_update_feed "$2"\n'
            'printf "%s|%s|%s|%s" "$EMAIL_UPDATE_FEED_URL" "$EMAIL_UPDATE_PUBLIC_ED_KEY" '
            '"$EMAIL_UPDATE_ALLOWED_HOSTS" "$EMAIL_UPDATE_CHANNEL"\n'
        )
        result = subprocess.run(["bash", "-c", script, "resolve", str(self.root), mode],
                                env=environment, capture_output=True, text=True)
        if result.returncode:
            return None, result.stderr
        return result.stdout.split("|"), result.stderr

    def test_distribute_ships_the_official_feed(self):
        self.assertEqual(OFFICIAL_FEED, "https://updates.ycorplabs.com/email/macos/appcast.xml")
        values, _ = self.resolve("official", "--distribute")
        self.assertEqual(values, [OFFICIAL_FEED, OFFICIAL_KEY, "ycorplabs.com updates.ycorplabs.com", "stable"])
        values, _ = self.resolve("official", "--distribute", EMAIL_UPDATE_CHANNEL="testing",
                                 EMAIL_UPDATE_FEED_URL=OFFICIAL_FEED, EMAIL_UPDATE_PUBLIC_ED_KEY=OFFICIAL_KEY)
        self.assertEqual(values[0], OFFICIAL_FEED)
        self.assertEqual(values[3], "testing")

    def test_distribute_refuses_another_feed_or_key(self):
        values, error = self.resolve("official", "--distribute",
                                     EMAIL_UPDATE_FEED_URL="https://ycorplabs.com/other/appcast.xml")
        self.assertIsNone(values)
        self.assertIn("EMAIL_UPDATE_FEED_URL differs", error)
        values, error = self.resolve("official", "--distribute", EMAIL_UPDATE_PUBLIC_ED_KEY=OTHER_KEY)
        self.assertIsNone(values)
        self.assertIn("EMAIL_UPDATE_PUBLIC_ED_KEY differs", error)
        values, error = self.resolve("official", "--distribute", EMAIL_UPDATE_CHANNEL="nightly")
        self.assertIsNone(values)

    def test_beta_never_has_a_feed_although_the_profile_records_one(self):
        values, _ = self.resolve("official", "--beta")
        self.assertEqual(values[:2], ["", ""])
        for name, value in (("EMAIL_UPDATE_FEED_URL", OFFICIAL_FEED), ("EMAIL_UPDATE_PUBLIC_ED_KEY", OFFICIAL_KEY)):
            values, error = self.resolve("official", "--beta", **{name: value})
            self.assertIsNone(values, name)
            self.assertIn("Beta builds are not notarized", error)

    def test_profiles_without_hosts_never_get_a_feed(self):
        for profile in ("development", "unofficial"):
            values, _ = self.resolve(profile, "--local-alpha")
            self.assertEqual(values[:3], ["", "", ""], profile)
            values, error = self.resolve(profile, "--local-alpha", EMAIL_UPDATE_FEED_URL=OFFICIAL_FEED,
                                         EMAIL_UPDATE_PUBLIC_ED_KEY=OFFICIAL_KEY)
            self.assertIsNone(values, profile)
            self.assertIn("no UPDATE_ALLOWED_HOSTS", error)

    def test_fixture_takes_its_loopback_feed_from_the_environment(self):
        feed = "http://127.0.0.1:8123/appcast.xml"
        values, _ = self.resolve("unofficial", "--fixture", EMAIL_UPDATE_FEED_URL=feed,
                                 EMAIL_UPDATE_PUBLIC_ED_KEY=OTHER_KEY, EMAIL_UPDATE_CHANNEL="testing")
        self.assertEqual(values, [feed, OTHER_KEY, "", "testing"])

    def test_packager_uses_the_shared_resolution(self):
        packager = (HERE / "package-macos-release.sh").read_text()
        self.assertIn('email_resolve_update_feed "$mode" || exit 2', packager)
        self.assertNotIn("EMAIL_PROFILE_UPDATE_FEED_URL", packager)


if __name__ == "__main__":
    unittest.main()
