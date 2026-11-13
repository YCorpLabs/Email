#!/usr/bin/env python3
"""Run committed candidate checks locally, without GitHub runners or daily app artifacts."""
import argparse
import contextlib
import datetime
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
PROFILES = ('source', 'rust', 'macos', 'linux', 'audit')


def default_linux_image(root=ROOT):
    """Image built by scripts/build-linux-container.sh for the pinned Rust version."""
    spec = importlib.util.spec_from_file_location('check_toolchain', Path(__file__).with_name('check-toolchain.py'))
    toolchain = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(toolchain)
    try:
        return 'email-linux-dev:' + toolchain.pinned_version(root)
    except (OSError, ValueError):
        return 'email-linux-dev:unpinned'


def manifest(profile, work, scanner, image, engine=None, target_volume='email-linux-workspace-target'):
    # Same variable as scripts/build-linux-container.sh, so a Podman-built image is found.
    engine = engine or os.environ.get('EMAIL_CONTAINER_ENGINE') or 'docker'
    groups = {}
    source = ['check-release-readiness.py', 'check-toolchain.py', 'check-toolchain.test.py',
              'build-identity.test.py',
              'check-release-readiness.test.py', 'linux-build-config.test.py',
              'distribution-profile.test.py', 'validate-local.test.py']
    # Private-only checks run where they exist; the public repository has none.
    source += [p for p in ('check-google-release-docs.py', 'check-google-release-docs.test.py',
                           'export-public-source.test.py') if (Path(work) / 'scripts' / p).is_file()]
    # The private repository's history needs its own reviewed exceptions.
    history_config = ['--config', 'history.gitleaks.toml'] if (Path(work) / 'history.gitleaks.toml').is_file() else []
    groups['source'] = [(p, [sys.executable, 'scripts/' + p], '.') for p in source] + [
        ('notices', [sys.executable, 'scripts/generate-third-party-notices.py', '--check'], '.'),
        ('secrets-history', [scanner, 'git', *history_config, '--log-opts=--all --full-history', '--redact=100', '--no-banner'], '.')]
    groups['rust'] = [
        ('rust-workspace', ['cargo', 'test', '--workspace', '--locked'], '.'),
        ('rust-release', ['cargo', 'build', '--release', '--locked', '-p', 'mail-service', '-p', 'mail-mcp'], '.'),
        ('mcp-permissions', [sys.executable, 'crates/mail-mcp/tests/permissions.py', str(work / 'target/release/mail-mcp')], '.')]
    identity = "import pathlib,subprocess; value=subprocess.check_output(['python3','scripts/build-identity.py'],text=True).strip(); pathlib.Path('apps/macos/Sources/EmailUI/BuildIdentity.generated.swift').write_text('import Foundation\\nenum EmailBuildIdentity { static let value = '+repr(value).replace(chr(39),chr(34))+' }\\n')"
    smoke = "import pathlib,subprocess,tempfile,os; b=pathlib.Path(subprocess.check_output(['swift','build','--package-path','apps/macos','--disable-sandbox','-c','release','--show-bin-path'],text=True).strip()); subprocess.run([str(b/'email-update-policy-checks')],check=True); t=tempfile.TemporaryDirectory(); e=dict(os.environ,EMAIL_STORE_PATH=str(pathlib.Path(t.name)/'smoke.sqlite'),MAIL_SERVICE_LIBRARY=str(pathlib.Path('target/release/libmail_service.dylib').resolve())); subprocess.run([str(b/'email-desktop'),'--smoke-service'],env=e,check=True)"
    demo = "import subprocess,sys; r=subprocess.run(['bash','scripts/dev.sh','--smoke','--release'],capture_output=True,text=True); print(r.stdout+r.stderr); sys.exit(r.returncode or int('1 accounts, 3 inbox threads' not in r.stdout))"
    groups['macos'] = [
        ('swift-rules', [sys.executable, 'scripts/check-release-swift.py'], '.'),
        ('launcher-cache', [sys.executable, 'scripts/run-macos-launcher-cache.test.py'], '.'),
        ('build-identity', [sys.executable, '-c', identity], '.'),
        ('swift-build', ['swift', 'build', '--package-path', 'apps/macos', '--disable-sandbox', '-c', 'release', '--only-use-versions-from-resolved-file'], '.'),
        ('macos-bridge-policy', [sys.executable, '-c', smoke], '.'),
        ('contributor-demo', [sys.executable, '-c', demo], '.'),
        ('macos-updates', ['bash', 'scripts/test-macos-updates.sh'], '.')]
    # The root and apps/linux workspaces resolve dependencies separately. Mixing
    # their artifacts in one target directory links mismatched crate builds.
    def container(target):
        return [engine, 'run', '--rm', '--mount', f'type=bind,source={work},target=/src,readonly',
                '--mount', 'type=volume,source=email-linux-cargo,target=/cargo',
                '--mount', f'type=volume,source={target_volume},target=/target',
                '-e', 'CARGO_HOME=/cargo', '-e', f'CARGO_TARGET_DIR={target}', '-e', 'CARGO_BUILD_JOBS=2', '-w', '/src', image]
    groups['linux'] = [('linux-image', [engine, 'image', 'inspect', image, '--format', '{{.Id}}'], '.')] + [
        ('linux-workspace', container('/target/workspace') + ['cargo', 'test', '--workspace', '--locked'], '.'),
        ('linux-shell-tests', container('/target/linux') + ['cargo', 'test', '--manifest-path', 'apps/linux/Cargo.toml', '--locked'], '.'),
        ('linux-shell-build', container('/target/linux') + ['cargo', 'build', '--manifest-path', 'apps/linux/Cargo.toml', '--release', '--locked'], '.')]
    groups['audit'] = [
        ('rust-audit', ['cargo', 'audit', '--file', 'Cargo.lock'], '.'),
        ('linux-audit', ['cargo', 'audit', '--file', 'apps/linux/Cargo.lock'], '.')]
    selected = list(PROFILES) if profile == 'all' else [profile]
    if profile == 'macos': selected.insert(0, 'rust')
    return [(group, *step) for group in selected for step in groups[group]]


def execute(steps, work, output, env, timeout):
    results = []
    failed = False
    for group, name, command, relative in steps:
        result = {'profile': group, 'name': name, 'status': 'not_run'}
        results.append(result)
        if failed:
            continue
        started = time.monotonic()
        log = output / (name + '.log')
        try:
            with log.open('w') as stream:
                process = subprocess.Popen(command, cwd=work / relative, env=env,
                                           stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
                try:
                    process.wait(timeout=timeout)
                except (subprocess.TimeoutExpired, KeyboardInterrupt):
                    with contextlib.suppress(ProcessLookupError): os.killpg(process.pid, signal.SIGTERM)
                    try: process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        with contextlib.suppress(ProcessLookupError): os.killpg(process.pid, signal.SIGKILL)
                        process.wait()
                    raise
            result['exit_code'] = process.returncode
            result['status'] = 'passed' if process.returncode == 0 else 'failed'
        except (OSError, subprocess.TimeoutExpired) as error:
            # Fixed error type only: no environment or secret values in report.
            result['status'] = 'failed'
            result['error'] = type(error).__name__
        result['seconds'] = round(time.monotonic() - started, 2)
        result['log'] = log.name
        if log.exists(): result['log_sha256'] = hashlib.sha256(log.read_bytes()).hexdigest()
        print(result['status'].upper() + ' ' + name, flush=True)
        failed = result['status'] != 'passed'
    return results


def report_status(results):
    return 'passed' if results and all(r['status'] == 'passed' for r in results) else 'failed'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=(*PROFILES, 'all'), default='source')
    parser.add_argument('--output', type=Path, help='New directory outside the checkout for report and logs')
    parser.add_argument('--cache', type=Path, default=Path(tempfile.gettempdir()) / f'email-local-validation-cache-{os.getuid()}')
    parser.add_argument('--gitleaks', default='gitleaks')
    parser.add_argument('--linux-image', default=default_linux_image(),
                        help='Linux build image (default: built by scripts/build-linux-container.sh --image-only)')
    parser.add_argument('--linux-target-volume', default='email-linux-workspace-target',
                        help='Docker volume for Linux build output; a new name starts from an empty target')
    parser.add_argument('--timeout', type=int, default=1800)
    parser.add_argument('--list', action='store_true', help='List checks without executing them')
    args = parser.parse_args()
    if args.timeout <= 0: parser.error('--timeout must be positive')
    if args.list:
        for group, name, _, _ in manifest(args.profile, ROOT, args.gitleaks, args.linux_image):
            print(group + ': ' + name)
        return 0
    if subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT).strip():
        parser.error('Commit or stash working changes first; validation only runs committed source')
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    output = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix='email-local-validation-'))
    if output == ROOT or ROOT in output.parents: parser.error('--output must be outside the checkout')
    if args.output:
        if output.exists(): parser.error('--output must be a new directory')
        output.mkdir(parents=True, mode=0o700)
    cache = args.cache.resolve()
    primary = Path(subprocess.check_output(['git', 'rev-parse', '--path-format=absolute', '--git-common-dir'], cwd=ROOT, text=True).strip()).parent
    if cache == primary or primary in cache.parents or cache == ROOT or ROOT in cache.parents:
        parser.error('--cache must be outside the source and primary checkout')
    cache.mkdir(parents=True, exist_ok=True, mode=0o700)
    info = cache.stat()
    if info.st_uid != os.getuid() or info.st_mode & 0o077:
        parser.error('--cache must be private and owned by the current user')
    with (cache / 'lock').open('w') as lock:
        try: fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError: parser.error('Another local validation run is using this cache')
        with tempfile.TemporaryDirectory(prefix='email-validation-source-') as scratch:
            work = Path(scratch) / 'Email'
            subprocess.run(['git', 'clone', '--shared', '--no-checkout', '--quiet', '--', str(ROOT), str(work)], check=True)
            subprocess.run(['git', 'checkout', '--detach', '--quiet', head], cwd=work, check=True)
            # Hardcoded packaging paths resolve inside the disposable checkout.
            # Never build/sign over the daily app or its MCP executable.
            target = cache / 'target'
            target.mkdir(exist_ok=True)
            (work / 'target').symlink_to(target, target_is_directory=True)
            env = {k: v for k, v in os.environ.items() if not k.startswith(('EMAIL_', 'MAIL_', 'CLOUDFLARE_', 'CF_')) and k not in ('GH_TOKEN', 'GITHUB_TOKEN', 'CARGO_TARGET_DIR')}
            env['PATH'] = str(Path.home() / '.cargo/bin') + os.pathsep + env.get('PATH', '')
            env['CARGO_BUILD_JOBS'] = '2'
            env['WRANGLER_SEND_METRICS'] = 'false'
            # Explicit browser selection is retained; no browser installation is automatic.
            for key in ('BOOKING_BROWSER', 'BOOKING_ENGINE'):
                if key in os.environ: env[key] = os.environ[key]
            scanner = shutil.which(args.gitleaks) or args.gitleaks
            steps = manifest(args.profile, work, scanner, args.linux_image, None, args.linux_target_volume)
            results = execute(steps, work, output, env, args.timeout)
            versions = {'python': platform.python_version()}
            for tool in ('rustc', 'node', 'swift'):
                try:
                    version = subprocess.run([tool, '--version'], env=env, capture_output=True, text=True, timeout=20)
                    if version.returncode == 0: versions[tool] = version.stdout.strip()
                except (OSError, subprocess.TimeoutExpired): pass
            report = {'schema': 1, 'commit': head, 'profile': args.profile, 'tool_versions': versions,
                      'status': report_status(results), 'host_system': platform.system(),
                      'host_architecture': platform.machine(), 'finished_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                      'steps': results, 'scope': 'Local candidate validation only. Other architectures, real providers, installed UI and signed distribution require separate acceptance.'}
            # A changing lockfile invalidates the candidate instead of silently qualifying it.
            changed = subprocess.check_output(['git', 'diff', '--name-only', 'HEAD'], cwd=work, text=True).splitlines()
            if changed:
                report['status'] = 'failed'
                report['changed_tracked_files'] = changed
            (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
            print('Report: ' + str(output / 'report.json'))
            return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, subprocess.CalledProcessError) as error:
        print('Local validation could not complete: ' + type(error).__name__, file=sys.stderr)
        sys.exit(1)
