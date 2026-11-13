"""Exercise the preflight against isolated Git indexes, without creating commits."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("check-release-readiness.py")


class PreflightChecks(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.git("init", "-q")
        self.write("scripts/check-release-readiness.py", SCRIPT.read_text())
        self.write(".gitignore", "/config/*.local\n/config/*.local.env\n/config/macos-update-signing.local/\n/dist-release/\n")
        self.write("Cargo.lock", "# fixture\n")
        self.write("apps/linux/Cargo.lock", "# fixture\n")
        self.git("add", ".")

    def write(self, name, value):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value)

    def git(self, *args):
        subprocess.run(["git", "-C", str(self.root), *args], check=True, capture_output=True)

    def check(self):
        return subprocess.run([sys.executable, str(self.root / "scripts/check-release-readiness.py")],
                              capture_output=True, text=True)

    def test_valid_source_and_untracked_ignored_local_config(self):
        self.write("config/oauth-desktop.local.env", "SYNTHETIC_FIXTURE=placeholder\n")
        self.assertEqual(self.check().returncode, 0)

    def test_force_added_private_config_fails_without_echoing_contents(self):
        self.write("config/oauth-desktop.local.env", "SYNTHETIC_FIXTURE=never-print-this-value\n")
        self.git("add", "-f", "config/oauth-desktop.local.env")
        result = self.check()
        self.assertEqual(result.returncode, 1)
        self.assertIn("Private/local file is tracked", result.stderr)
        self.assertNotIn("never-print-this-value", result.stdout + result.stderr)

    def test_force_added_local_config_with_any_extension_fails(self):
        self.write("config/oauth.local.json", '{"synthetic": "never-print-this-value"}\n')
        self.git("add", "-f", "config/oauth.local.json")
        result = self.check()
        self.assertEqual(result.returncode, 1)
        self.assertIn("Private/local file is tracked", result.stderr)
        self.assertNotIn("never-print-this-value", result.stdout + result.stderr)

    def test_missing_lock_fails(self):
        (self.root / "apps/linux/Cargo.lock").unlink()
        self.assertIn("Missing tracked dependency lockfile", self.check().stderr)

    def test_force_added_update_key_directory_fails_even_with_generic_filename(self):
        self.write("config/macos-update-signing.local/private.txt", "synthetic-never-print-this-value\n")
        self.git("add", "-f", "config/macos-update-signing.local/private.txt")
        result = self.check()
        self.assertEqual(result.returncode, 1)
        self.assertIn("Private/local file is tracked", result.stderr)
        self.assertNotIn("synthetic-never-print-this-value", result.stdout + result.stderr)

    def test_removed_ignore_rule_fails(self):
        self.write(".gitignore", "")
        self.assertIn("Private/generated path is not ignored", self.check().stderr)

    def test_invalid_tracked_shell_fails(self):
        self.write("scripts/broken.sh", "#!/bin/bash\nif then\n")
        self.git("add", "scripts/broken.sh")
        self.assertIn("Shell syntax failed", self.check().stderr)


if __name__ == "__main__":
    unittest.main()
