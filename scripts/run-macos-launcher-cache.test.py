#!/usr/bin/env python3
"""Disposable launcher fixtures only; never operates on a checkout's dist app."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
FIXTURE_CLIENT = '123456-launcher-fixture.apps.googleusercontent.com'
SCRIPT = Path(__file__).with_name('run-macos.sh').read_text()
MAIN = 'apps/macos/Sources/EmailDesktop/main.swift'
MANIFEST = 'apps/macos/Package.swift'
BASE = '''// swift-tools-version: 6.0
import PackageDescription
let package = Package(name: "EmailDesktop", platforms: [.macOS(.v13)],
    products: [.executable(name: "email-desktop", targets: ["EmailDesktop"])],
    targets: [.executableTarget(name: "EmailDesktop")])
'''
UNRELATED = BASE.replace(
    'products: [', 'products: [.library(name: "EmailUpdater", type: .dynamic, targets: ["EmailUpdater"]), '
).replace('targets: [.executableTarget', 'dependencies: [.package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.10.1")], targets: [.target(name: "EmailUpdater", dependencies: [.product(name: "Sparkle", package: "Sparkle")]), .executableTarget')


def sha(data):
    return hashlib.sha256(data).hexdigest()


@unittest.skipUnless(sys.platform == 'darwin', 'requires real macOS ad-hoc signature verification')
class LauncherCacheTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='email-launcher-test-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.app = self.root / 'dist/Email.app'
        self.binary = self.app / 'Contents/MacOS/Email'
        self.cache = self.root / 'dist/.launcher-cache'
        self.metadata = self.cache / 'contract-v2.json'
        for directory in ('scripts', 'config', 'dist/.launcher-cache',
                          'apps/macos/Sources/EmailDesktop', 'bin', 'dist/Email.app/Contents/MacOS'):
            (self.root / directory).mkdir(parents=True, exist_ok=True)
        (self.root / 'scripts/run-macos.sh').write_text(SCRIPT)
        # The development profile must keep resolving to the bundle ID
        # asserted by the codesign wrapper below.
        shutil.copy2(HERE / 'distribution-profile.sh', self.root / 'scripts/distribution-profile.sh')
        # The development profile accepts only its frozen client (by digest);
        # substitute the synthetic fixture client's digest in this copy.
        resolver = (HERE / 'distribution-profile.py').read_text()
        digest = re.search(r'"([0-9a-f]{64})",\n\}\)', resolver).group(1)
        fixture = hashlib.sha256(FIXTURE_CLIENT.encode()).hexdigest()
        (self.root / 'scripts/distribution-profile.py').write_text(resolver.replace(digest, fixture))
        shutil.copytree(HERE.parent / 'config/profiles', self.root / 'config/profiles')
        (self.root / 'config/distribution-profile.local').write_text('development\n')
        (self.root / MAIN).write_text('print("fixture")\n')
        (self.root / MANIFEST).write_text(BASE)
        self.command('git', 'init', '-q')
        self.command('git', 'add', MAIN, MANIFEST)
        self.command('git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                     '-c', 'commit.gpgsign=false', 'commit', '-qm', 'legacy inputs')
        commit = self.command('git', 'rev-parse', 'HEAD').strip()
        # Compile and ad-hoc sign only this temporary app, without any Keychain.
        source = self.root / 'fixture.c'
        source.write_text('int main(void) { return 0; }\n')
        self.command('cc', str(source), '-o', str(self.binary))
        (self.app / 'Contents/Info.plist').write_bytes(plistlib.dumps({
            'CFBundleIdentifier': 'dev.email.client', 'CFBundleExecutable': 'Email',
            'CFBundlePackageType': 'APPL'}))
        self.command('/usr/bin/codesign', '--force', '--sign', '-', str(self.app))
        self.command('/usr/bin/codesign', '--verify', '--deep', '--strict', str(self.app))
        details = self.command('/usr/bin/codesign', '-dv', '--verbose=4', str(self.app))
        self.cdhash = next(line[7:] for line in details.splitlines() if line.startswith('CDHash='))
        shutil.copy2(self.binary, self.cache / 'Email')
        legacy = self.command('shasum', '-a', '256', str(self.root / MAIN), str(self.root / MANIFEST))
        (self.cache / 'source-hash').write_text(sha(legacy.encode()) + '\n')
        self.provenance = self.root / 'dist/email-build.json'
        self.provenance.write_text(json.dumps(dict(gitCommit=commit, artifacts={
            'Email.app/Contents/MacOS/Email': sha(self.binary.read_bytes())})))
        (self.root / 'config/macos-signing-identity.local').write_text('a' * 40 + '\n')
        # Only the persistent certificate requirement is simulated. All signature
        # integrity/CDHash operations use real codesign on the ad-hoc fixture.
        wrapper = self.root / 'bin/codesign'
        wrapper.write_text('''#!/usr/bin/env python3
import os, sys
from pathlib import Path
args = sys.argv[1:]
if '-R' in args:
    index = args.index('-R')
    assert args[index + 1] == '=identifier "dev.email.client" and certificate leaf = H"' + 'a' * 40 + '"'
    if Path(os.environ['FIXTURE_ROOT'], 'reject-certificate').exists():
        sys.exit(1)
    del args[index:index + 2]
os.execv('/usr/bin/codesign', ['codesign'] + args)
''')
        wrapper.chmod(0o755)
        self.env = dict(os.environ, PATH=str(self.root / 'bin') + ':' + os.environ['PATH'],
                        FIXTURE_ROOT=str(self.root))
        self.env.pop('EMAIL_ALLOW_LAUNCHER_CHANGE', None)
        for name in [name for name in self.env if name.startswith('EMAIL_')]:
            self.env.pop(name)
        self.env['EMAIL_GOOGLE_CLIENT_ID'] = FIXTURE_CLIENT
        self.before = self.snapshot()

    def command(self, *args, env=None):
        return subprocess.check_output(args, cwd=self.root, env=env, text=True, stderr=subprocess.STDOUT)

    def snapshot(self):
        return (self.binary.read_bytes(), self.binary.stat().st_ino,
                (self.app / 'Contents/_CodeSignature/CodeResources').read_bytes(),
                self.command('/usr/bin/codesign', '-dv', '--verbose=4', str(self.app)),
                self.command('/usr/bin/codesign', '-dr', '-', str(self.app)))

    def script(self, *args, success=True):
        result = subprocess.run(['bash', str(self.root / 'scripts/run-macos.sh'), *args],
                                cwd=self.root, env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def prepare(self, success=True, cdhash=None):
        result = self.script('--prepare-launcher-cache', cdhash or self.cdhash, success=success)
        self.assertEqual(self.before, self.snapshot(), 'preparation changed signed app or executable inode')
        if not success:
            self.assertFalse(self.metadata.exists())
        return result

    def select_launcher(self, reuse):
        # Exercise the actual packaging cache selection block without packaging,
        # signing, or launching. Supply a deliberately different build artifact.
        built = self.root / 'build'
        built.mkdir(exist_ok=True)
        (built / 'email-desktop').write_bytes(b'different newly built launcher')
        helpers = SCRIPT[SCRIPT.index('launcher_metadata() {'):SCRIPT.index('if [[ "${1:-}" == "--prepare-launcher-cache"')]
        selection = SCRIPT[SCRIPT.index('copy_macho_atomically() {'):SCRIPT.index('copy_macho_atomically "$bin_dir/libEmailUI.dylib"')]
        shell = 'set -euo pipefail\nrepo_root="$1"\napp_dir="$1/dist/Email.app"\nbin_dir="$1/build"\noriginal_app_hash="$2"\n'
        shell += helpers + selection + '\nprintf "reuse=%s\\n" "$reuse_launcher"\n'
        output = self.command('bash', '-c', shell, 'fixture', str(self.root), self.cdhash, env=self.env)
        self.assertIn('reuse=' + ('true' if reuse else 'false'), output)
        if reuse:
            self.assertEqual(self.before, self.snapshot())
        else:
            self.assertEqual(self.binary.read_bytes(), b'different newly built launcher')

    def test_unrelated_manifest_changes_migrate_and_reuse_exact_signature(self):
        (self.root / MANIFEST).write_text(UNRELATED)
        self.prepare()
        record = json.loads(self.metadata.read_text())
        self.assertEqual(record['version'], 2)
        self.assertEqual(record['preservedCDHash'], self.cdhash)
        self.script('--check-launcher-cache')
        self.select_launcher(reuse=True)
        # Further unrelated product/target edits also keep the contract stable.
        (self.root / MANIFEST).write_text(UNRELATED.replace('EmailUpdater', 'OtherUpdater'))
        self.script('--check-launcher-cache')
        self.select_launcher(reuse=True)

    def test_missing_metadata_stops_packaging_before_build(self):
        result = self.script('--package', success=False)
        self.assertIn('Prepare legacy launcher metadata', result.stderr)
        self.assertEqual(self.before, self.snapshot())

    def test_missing_google_client_id_stops_packaging_before_build(self):
        self.prepare()
        self.env.pop('EMAIL_GOOGLE_CLIENT_ID')
        result = self.script('--package', success=False)
        self.assertIn('EMAIL_GOOGLE_CLIENT_ID', result.stderr)
        self.assertIn('config/oauth-desktop.local.env', result.stderr)
        self.assertEqual(self.before, self.snapshot())

    def test_other_profile_cannot_repackage_existing_identity(self):
        self.prepare()
        self.env['EMAIL_DISTRIBUTION_PROFILE'] = 'unofficial'
        result = self.script('--package', success=False)
        self.assertIn('dist/Email.app is dev.email.client', result.stderr)
        self.assertEqual(self.before, self.snapshot())

    def test_main_change_rejects_migration(self):
        (self.root / MAIN).write_text('print("changed")\n')
        self.prepare(success=False)

    def test_main_change_invalidates_v2(self):
        self.prepare()
        (self.root / MAIN).write_text('print("changed")\n')
        self.script('--check-launcher-cache', success=False)
        self.select_launcher(reuse=False)

    def test_link_setting_change_invalidates_v2(self):
        self.prepare()
        (self.root / MANIFEST).write_text(BASE.replace('name: "EmailDesktop")]',
            'name: "EmailDesktop", linkerSettings: [.linkedFramework("Security")])]'))
        self.script('--check-launcher-cache', success=False)
        self.select_launcher(reuse=False)

    def test_link_dependency_is_unsupported(self):
        self.prepare()
        (self.root / MANIFEST).write_text(UNRELATED.replace('.executableTarget(name: "EmailDesktop")',
            '.executableTarget(name: "EmailDesktop", dependencies: ["EmailUpdater"])'))
        self.script('--check-launcher-cache', success=False)

    def test_added_source_is_unsupported(self):
        self.prepare()
        (self.root / MAIN).with_name('extra.swift').write_text('let extra = 1\n')
        self.script('--check-launcher-cache', success=False)

    def test_stale_legacy_hash_rejected(self):
        (self.cache / 'source-hash').write_text('0' * 64)
        self.prepare(success=False)

    def test_cache_binary_mismatch_rejected(self):
        (self.cache / 'Email').write_bytes(b'stale')
        self.prepare(success=False)

    def test_provenance_mismatch_rejected(self):
        record = json.loads(self.provenance.read_text())
        record['artifacts']['Email.app/Contents/MacOS/Email'] = '0' * 64
        self.provenance.write_text(json.dumps(record))
        self.prepare(success=False)

    def test_wrong_cdhash_rejected(self):
        self.prepare(success=False, cdhash='0' * 40)

    def test_wrong_certificate_rejected(self):
        (self.root / 'reject-certificate').touch()
        self.prepare(success=False)

    def test_v2_binary_tampering_rejected(self):
        self.prepare()
        (self.cache / 'Email').write_bytes(b'tampered')
        self.script('--check-launcher-cache', success=False)
        self.assertEqual(self.before, self.snapshot())

    def test_missing_provenance_rejected(self):
        self.provenance.unlink()
        self.prepare(success=False)

    def test_invalid_signature_rejected(self):
        (self.app / 'Contents/Info.plist').write_bytes((self.app / 'Contents/Info.plist').read_bytes() + b'\n')
        self.before = self.snapshot()
        self.prepare(success=False)

    def test_exact_cdhash_guard_restores_changed_fixture(self):
        self.prepare()
        source = self.root / 'changed.c'
        source.write_text('int main(void) { return 9; }\n')
        built = self.root / 'build'
        built.mkdir()
        self.command('cc', str(source), '-o', str(built / 'email-desktop'))
        helpers = SCRIPT[SCRIPT.index('launcher_metadata() {'):SCRIPT.index('if [[ "${1:-}" == "--prepare-launcher-cache"')]
        restoration = SCRIPT[SCRIPT.index('restore_failed_package() {'):SCRIPT.index('runtime_dir=')]
        selection = SCRIPT[SCRIPT.index('copy_macho_atomically() {'):SCRIPT.index('copy_macho_atomically "$bin_dir/libEmailUI.dylib"')]
        guard = SCRIPT[SCRIPT.index('final_app_hash='):SCRIPT.index('# Rebuilding in place otherwise')]
        shell = 'set -euo pipefail\nrepo_root="$1"\napp_dir="$1/dist/Email.app"\nbin_dir="$1/build"\noriginal_app_hash="$2"\n'
        shell += 'app_backup="$(mktemp -d)"\nditto "$app_dir" "$app_backup/Email.app"\n'
        shell += restoration + helpers + selection
        # Fixture-only ad-hoc signing, never a local identity or credential.
        shell += '/usr/bin/codesign --force --sign - "$app_dir"\n' + guard
        (self.root / MAIN).write_text('print("intentional source change")\n')
        result = subprocess.run(['bash', '-c', shell, 'fixture', str(self.root), self.cdhash],
                                cwd=self.root, env=self.env, text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Packaging changed Email's Keychain identity", result.stderr)
        self.assertIn('Restored the prior signed Email.app', result.stderr)
        after = self.snapshot()
        self.assertEqual(self.before[0], after[0])
        self.assertEqual(self.before[2:], after[2:])
        self.command('/usr/bin/codesign', '--verify', '--deep', '--strict', str(self.app))

    def test_v2_cannot_be_overwritten_by_preparation(self):
        self.prepare()
        original = self.metadata.read_bytes()
        self.script('--prepare-launcher-cache', self.cdhash, success=False)
        self.assertEqual(original, self.metadata.read_bytes())
        self.assertEqual(self.before, self.snapshot())


if __name__ == '__main__':
    unittest.main(verbosity=2)
