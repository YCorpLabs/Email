#!/usr/bin/env python3
"""Failure propagation and exact-commit/isolation checks for the local runner."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('local_validation', Path(__file__).with_name('validate-local.py'))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class LocalValidationTests(unittest.TestCase):
    def test_failure_stops_later_steps_and_never_reports_green(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            steps = [('source', 'bad', [sys.executable, '-c', 'raise SystemExit(7)'], '.'),
                     ('rust', 'later', [sys.executable, '-c', "open('must-not-exist','w').close()"], '.')]
            result = runner.execute(steps, root, root, dict(os.environ), 5)
            self.assertEqual([r['status'] for r in result], ['failed', 'not_run'])
            self.assertEqual(result[0]['exit_code'], 7)
            self.assertFalse((root / 'must-not-exist').exists())
            self.assertEqual(runner.report_status(result), 'failed')

    def test_missing_executable_and_timeout_fail(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            for command in [['/does-not-exist/local-validation-test'], [sys.executable, '-c', 'import time; time.sleep(2)']]:
                result = runner.execute([('source', 'bad', command, '.')], root, root, dict(os.environ), 0.05)
                self.assertEqual(runner.report_status(result), 'failed')
                self.assertIn(result[0]['error'], ['FileNotFoundError', 'TimeoutExpired'])

    def test_empty_or_incomplete_results_cannot_pass(self):
        for results in [[], [{'status': 'not_run'}], [{'status': 'passed'}, {'status': 'failed'}]]:
            self.assertEqual(runner.report_status(results), 'failed')

    def test_macos_includes_rust_and_all_profiles_keep_audits(self):
        work = Path('/fixture')
        mac = runner.manifest('macos', work, 'gitleaks', 'local-image')
        self.assertIn('rust-release', [s[1] for s in mac])
        self.assertIn('macos-updates', [s[1] for s in mac])
        all_steps = runner.manifest('all', work, 'gitleaks', 'local-image')
        self.assertEqual(set(s[0] for s in all_steps), set(runner.PROFILES))
        self.assertIn('secrets-history', [s[1] for s in all_steps])
        self.assertIn('linux-shell-build', [s[1] for s in all_steps])
        self.assertNotIn('booking-browser', [s[1] for s in all_steps])

    def test_source_checks_toolchain_and_macos_checks_contributor_demo(self):
        work = Path('/fixture')
        self.assertIn('check-toolchain.py', [s[1] for s in runner.manifest('source', work, 'gitleaks', 'image')])
        self.assertIn('contributor-demo', [s[1] for s in runner.manifest('macos', work, 'gitleaks', 'image')])

    def test_linux_workspaces_use_separate_target_directories(self):
        steps = {s[1]: s[2] for s in runner.manifest('linux', Path('/fixture'), 'gitleaks', 'image')}
        self.assertIn('CARGO_TARGET_DIR=/target/workspace', steps['linux-workspace'])
        self.assertIn('CARGO_TARGET_DIR=/target/linux', steps['linux-shell-tests'])
        self.assertIn('CARGO_TARGET_DIR=/target/linux', steps['linux-shell-build'])

    def test_linux_target_volume_can_be_isolated(self):
        steps = runner.manifest('linux', Path('/fixture'), 'gitleaks', 'image', target_volume='fresh-target')
        self.assertTrue(all('type=volume,source=fresh-target,target=/target' in step[2] for step in steps[1:]))

    def test_linux_steps_use_the_selected_container_engine(self):
        steps = runner.manifest('linux', Path('/fixture'), 'gitleaks', 'image', engine='podman')
        self.assertTrue(all(step[2][0] == 'podman' for step in steps))
        with patch.dict(os.environ, {'EMAIL_CONTAINER_ENGINE': 'podman'}):
            self.assertEqual(runner.manifest('linux', Path('/fixture'), 'gitleaks', 'image')[0][2][0], 'podman')

    def test_default_linux_image_follows_pinned_toolchain(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / 'rust-toolchain.toml').write_text('[toolchain]\nchannel = "1.2.3"\n')
            self.assertEqual(runner.default_linux_image(root), 'email-linux-dev:1.2.3')
            (root / 'rust-toolchain.toml').write_text('[toolchain]\nchannel = "stable"\n')
            self.assertEqual(runner.default_linux_image(root), 'email-linux-dev:unpinned')

    def test_swift_identity_generator_writes_real_newlines(self):
        with tempfile.TemporaryDirectory() as d:
            work = Path(d)
            (work / 'scripts').mkdir()
            (work / 'apps/macos/Sources/EmailUI').mkdir(parents=True)
            (work / 'scripts/build-identity.py').write_text("print('fixture-identity')\n")
            step = next(s for s in runner.manifest('macos', work, 'gitleaks', 'image') if s[1] == 'build-identity')
            subprocess.run(step[2], cwd=work, check=True)
            text = (work / 'apps/macos/Sources/EmailUI/BuildIdentity.generated.swift').read_text()
            self.assertEqual(text, 'import Foundation\nenum EmailBuildIdentity { static let value = "fixture-identity" }\n')

    def test_full_run_uses_exact_commit_in_separate_checkout(self):
        with tempfile.TemporaryDirectory() as d:
            base = Path(d)
            root = base / 'original'
            root.mkdir()
            self.git(root, 'init', '-q')
            (root / 'tracked').write_text('committed')
            self.git(root, 'add', '.')
            self.git(root, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.com', 'commit', '-qm', 'fixture')
            head = self.git(root, 'rev-parse', 'HEAD')
            output = base / 'report'
            def fixture_steps(profile, work, *_):
                self.assertNotEqual(work, root)
                self.assertEqual((work / 'tracked').read_text(), 'committed')
                return [('source', 'fixture', [sys.executable, '-c', "print('fixture check passed')"], '.')]
            with patch.object(runner, 'ROOT', root), patch.object(runner, 'manifest', fixture_steps), patch.object(sys, 'argv', ['validate-local.py', '--output', str(output), '--cache', str(base / 'cache')]):
                self.assertEqual(runner.main(), 0)
            report = json.loads((output / 'report.json').read_text())
            self.assertEqual(report['commit'], head)
            self.assertEqual(report['status'], 'passed')
            self.assertEqual(report['profile'], 'source')
            self.assertEqual(self.git(root, 'status', '--porcelain'), '')
            self.assertEqual(len(report['steps'][0]['log_sha256']), 64)

    def test_dirty_source_is_rejected_before_any_report(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.git(root, 'init', '-q')
            (root / 'uncommitted').write_text('dirty')
            output = root.parent / (root.name + '-report')
            with patch.object(runner, 'ROOT', root), patch.object(sys, 'argv', ['validate-local.py', '--output', str(output)]), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error: runner.main()
            self.assertEqual(error.exception.code, 2)
            self.assertFalse(output.exists())

    @staticmethod
    def git(root, *args):
        return subprocess.check_output(['git', '-C', str(root), *args], text=True, stderr=subprocess.DEVNULL).strip()


if __name__ == '__main__':
    unittest.main()
