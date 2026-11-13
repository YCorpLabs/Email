"""Exercise package OAuth and profile policy with synthetic config, no compiler or vault."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


HERE = Path(__file__).resolve().parent
DEV = "123456-development-fixture.apps.googleusercontent.com"
PROD = "123456-production-fixture.apps.googleusercontent.com"
KNOWN_DEV = "987654-known-development-fixture.apps.googleusercontent.com"
DIGEST_LINE = '    "65bf0d27eaac8c8573ee4a865a6149a42a0c110b56010b56fb13adbf522506b4",'


class LinuxPackageOAuth(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        (self.root / "scripts").mkdir()
        for name in ("linux-build-config.sh", "distribution-profile.sh"):
            shutil.copy2(HERE / name, self.root / "scripts" / name)
        # The real script refuses the development client by digest.
        # Substitute a synthetic client's digest to exercise that refusal.
        resolver = (HERE / "distribution-profile.py").read_text()
        self.assertEqual(resolver.count(DIGEST_LINE), 1)
        synthetic = hashlib.sha256(KNOWN_DEV.encode()).hexdigest()
        (self.root / "scripts/distribution-profile.py").write_text(
            resolver.replace(DIGEST_LINE, f'    "{synthetic}",'))
        shutil.copytree(HERE.parent / "config/profiles", self.root / "config/profiles")
        (self.root / "scripts/build-identity.py").write_text("print('fixture-build')\n")

    def configure(self, profile="development", **values):
        env = {k: v for k, v in os.environ.items() if not k.startswith("EMAIL_")}
        if profile:
            env["EMAIL_DISTRIBUTION_PROFILE"] = profile
        env.update(values)
        # Compare synthetic configuration in the child, never print values.
        code = '''set -e
source scripts/linux-build-config.sh
require_linux_oauth_config
if [[ "${EMAIL_LINUX_RELEASE_CHANNEL:-local-alpha}" == distribution ]]; then
  [[ "$EMAIL_GOOGLE_CLIENT_ID" == "$EMAIL_RELEASE_GOOGLE_CLIENT_ID" ]]
  [[ "$EMAIL_GOOGLE_CLIENT_SECRET" == "$EMAIL_RELEASE_GOOGLE_CLIENT_SECRET" ]]
fi
if [[ -n "${EXPECT_CLIENT:-}" ]]; then [[ "$EMAIL_GOOGLE_CLIENT_ID" == "$EXPECT_CLIENT" ]]; fi
printf 'accepted %s %s channel=%s google-oauth-public=%s\\n' "$EMAIL_PROFILE_PROFILE" "$EMAIL_PROFILE_LINUX_APP_ID" \\
  "$EMAIL_RELEASE_CHANNEL" "$EMAIL_GOOGLE_OAUTH_PUBLIC"
'''
        result = subprocess.run(["bash", "-c", code], cwd=self.root, env=env,
                                capture_output=True, text=True)
        self.assertNotIn("fixture-token-exchange", result.stdout + result.stderr)
        return result

    def local(self, text):
        (self.root / "config/oauth-desktop.local.env").write_text(text)

    def test_development_local_alpha_uses_local_client(self):
        self.local(f"EMAIL_GOOGLE_CLIENT_ID={KNOWN_DEV}\nEMAIL_GOOGLE_CLIENT_SECRET=fixture-token-exchange\n")
        result = self.configure(EXPECT_CLIENT=KNOWN_DEV)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("accepted development dev.email.client", result.stdout)

    def test_missing_development_client_names_variable_and_file(self):
        result = self.configure(EMAIL_GOOGLE_CLIENT_SECRET="fixture-token-exchange")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("EMAIL_GOOGLE_CLIENT_ID", result.stderr)
        self.assertIn("config/oauth-desktop.local.env", result.stderr)

    def test_malformed_client_is_refused(self):
        result = self.configure(EMAIL_GOOGLE_CLIENT_ID="not-a-google-client",
                                EMAIL_GOOGLE_CLIENT_SECRET="fixture-token-exchange")
        self.assertNotEqual(result.returncode, 0)

    def test_environment_brings_its_own_client_over_local_file(self):
        self.local(f"EMAIL_GOOGLE_CLIENT_ID={DEV}\nEMAIL_GOOGLE_CLIENT_SECRET=fixture-token-exchange\n")
        result = self.configure("unofficial", EMAIL_GOOGLE_CLIENT_ID=PROD, EXPECT_CLIENT=PROD)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_development_profile_rejects_another_client(self):
        self.local(f"EMAIL_GOOGLE_CLIENT_ID={DEV}\nEMAIL_GOOGLE_CLIENT_SECRET=fixture-token-exchange\n")
        result = self.configure()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("frozen", result.stderr)

    def test_missing_local_application_secret_fails(self):
        self.assertNotEqual(self.configure(EMAIL_GOOGLE_CLIENT_ID=DEV).returncode, 0)

    def test_neutral_default_builds_without_oauth_or_selection(self):
        result = self.configure(profile=None)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("accepted unofficial invalid.email.Unofficial", result.stdout)

    def test_selection_file_chooses_profile(self):
        (self.root / "config/distribution-profile.local").write_text("development\n")
        self.local(f"EMAIL_GOOGLE_CLIENT_ID={KNOWN_DEV}\nEMAIL_GOOGLE_CLIENT_SECRET=fixture-token-exchange\n")
        result = self.configure(profile=None)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("accepted development", result.stdout)

    def test_local_alpha_refuses_a_distribution_profile(self):
        result = self.configure("official", EMAIL_GOOGLE_CLIENT_ID=PROD,
                                EMAIL_GOOGLE_CLIENT_SECRET="fixture-token-exchange")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("signed releases", result.stderr)

    def test_unknown_profile_fails(self):
        self.assertNotEqual(self.configure(profile="missing").returncode, 0)

    def test_distribution_refuses_non_distribution_profile(self):
        self.assertNotEqual(self.configure(EMAIL_LINUX_RELEASE_CHANNEL="distribution",
                            EMAIL_RELEASE_GOOGLE_CLIENT_ID=PROD,
                            EMAIL_RELEASE_GOOGLE_CLIENT_SECRET="fixture-token-exchange").returncode, 0)

    def test_distribution_refuses_development_fallback(self):
        self.assertNotEqual(self.configure("official", EMAIL_LINUX_RELEASE_CHANNEL="distribution",
                            EMAIL_GOOGLE_CLIENT_SECRET="fixture-token-exchange").returncode, 0)

    def test_distribution_refuses_generic_environment_override(self):
        self.assertNotEqual(self.configure("official", EMAIL_LINUX_RELEASE_CHANNEL="distribution",
                            EMAIL_GOOGLE_CLIENT_ID=PROD,
                            EMAIL_GOOGLE_CLIENT_SECRET="fixture-token-exchange").returncode, 0)

    def test_distribution_refuses_known_development_client(self):
        result = self.configure("official", EMAIL_LINUX_RELEASE_CHANNEL="distribution",
                                EMAIL_RELEASE_GOOGLE_CLIENT_ID=KNOWN_DEV,
                                EMAIL_RELEASE_GOOGLE_CLIENT_SECRET="fixture-token-exchange")
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn(KNOWN_DEV, result.stdout + result.stderr)

    def test_release_machine_may_keep_its_production_client_locally(self):
        self.local(f"EMAIL_GOOGLE_CLIENT_ID={PROD}\nEMAIL_GOOGLE_CLIENT_SECRET=fixture-token-exchange-local\n")
        result = self.configure("official", EMAIL_LINUX_RELEASE_CHANNEL="distribution",
                                EMAIL_RELEASE_GOOGLE_CLIENT_ID=PROD,
                                EMAIL_RELEASE_GOOGLE_CLIENT_SECRET="fixture-token-exchange")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_distribution_requires_both_release_values(self):
        self.assertNotEqual(self.configure("official", EMAIL_LINUX_RELEASE_CHANNEL="distribution",
                            EMAIL_RELEASE_GOOGLE_CLIENT_ID=PROD).returncode, 0)

    def test_distribution_uses_explicit_release_config_over_local_development(self):
        self.local(f"EMAIL_GOOGLE_CLIENT_ID={DEV}\nEMAIL_GOOGLE_CLIENT_SECRET=fixture-token-exchange-local\n")
        result = self.configure("official", EMAIL_LINUX_RELEASE_CHANNEL="distribution",
                                EMAIL_RELEASE_GOOGLE_CLIENT_ID=PROD,
                                EMAIL_RELEASE_GOOGLE_CLIENT_SECRET="fixture-token-exchange-production")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("accepted official com.ycorplabs.Mail", result.stdout)

    def test_distribution_rejects_malformed_client_id(self):
        self.assertNotEqual(self.configure("official", EMAIL_LINUX_RELEASE_CHANNEL="distribution",
                            EMAIL_RELEASE_GOOGLE_CLIENT_ID="not-a-google-client",
                            EMAIL_RELEASE_GOOGLE_CLIENT_SECRET="fixture-token-exchange").returncode, 0)

    RELEASE = dict(EMAIL_LINUX_RELEASE_CHANNEL="distribution", EMAIL_RELEASE_GOOGLE_CLIENT_ID=PROD,
                   EMAIL_RELEASE_GOOGLE_CLIENT_SECRET="fixture-token-exchange")

    def test_channel_and_google_sign_in_switch_reach_the_compiler(self):
        self.local(f"EMAIL_GOOGLE_CLIENT_ID={KNOWN_DEV}\nEMAIL_GOOGLE_CLIENT_SECRET=fixture-token-exchange\n")
        local = self.configure()
        self.assertEqual(local.returncode, 0, local.stderr)
        self.assertIn("accepted development dev.email.client channel=local-alpha google-oauth-public=true",
                      local.stdout)
        # The official profile keeps Google sign-in off until maintainers turn it on there.
        release = self.configure("official", **self.RELEASE)
        self.assertEqual(release.returncode, 0, release.stderr)
        self.assertIn("channel=distribution google-oauth-public=false", release.stdout)
        # One build may override it, for example the recording for Google's review.
        self.assertIn("google-oauth-public=true",
                      self.configure("official", **self.RELEASE, EMAIL_GOOGLE_OAUTH_PUBLIC="true").stdout)
        self.assertNotEqual(self.configure("official", **self.RELEASE,
                                           EMAIL_GOOGLE_OAUTH_PUBLIC="yes").returncode, 0)

    def test_google_sign_in_follows_the_official_profile(self):
        # After Google approves, maintainers set GOOGLE_OAUTH_PUBLIC=true in the profile.
        official = self.root / "config/profiles/official.env"
        text = official.read_text()
        self.assertEqual(text.count("GOOGLE_OAUTH_PUBLIC=false"), 1)
        official.write_text(text.replace("GOOGLE_OAUTH_PUBLIC=false", "GOOGLE_OAUTH_PUBLIC=true"))
        release = self.configure("official", **self.RELEASE)
        self.assertEqual(release.returncode, 0, release.stderr)
        self.assertIn("channel=distribution google-oauth-public=true", release.stdout)

    def test_unknown_channel_does_not_build(self):
        self.assertNotEqual(self.configure(EMAIL_LINUX_RELEASE_CHANNEL="distibution",
                            EMAIL_GOOGLE_CLIENT_ID=KNOWN_DEV,
                            EMAIL_GOOGLE_CLIENT_SECRET="fixture-token-exchange").returncode, 0)


if __name__ == "__main__":
    unittest.main()
