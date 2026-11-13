#!/usr/bin/env python3
"""Compile actual AppKit activity UI with an isolated fixture bridge, never Email's bridge."""
from pathlib import Path
import subprocess
import platform
import os
import tempfile

root = Path(__file__).resolve().parents[1]
sdk = os.environ.get('EMAIL_TEST_MACOS_SDK') or subprocess.check_output(['xcrun', '--sdk', 'macosx', '--show-sdk-path'], text=True).strip()
architecture = platform.machine()
with tempfile.TemporaryDirectory(prefix='email-activity-appkit-') as directory:
    target = Path(directory)
    source = target / 'Checks.swift'
    source.write_text((root / 'Sources/EmailUI/TrackingPanel.swift').read_text() + '\n' + (root / 'Sources/EmailUI/OpenActivitySummary.swift').read_text() + '\n' + (root / 'Tests/TrackingActivityChecks.swift').read_text())
    args = ['swiftc', '-parse-as-library', '-swift-version', '6', '-target', f'{architecture}-apple-macos13.0', '-sdk', str(sdk), str(source), '-o', str(target / 'checks')]
    subprocess.run(args, check=True)
    subprocess.run([str(target / 'checks')], check=True)
