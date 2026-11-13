#!/usr/bin/env python3
"""Negative tests for the Rust toolchain pin check."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('toolchain', Path(__file__).with_name('check-toolchain.py'))
toolchain = importlib.util.module_from_spec(spec)
spec.loader.exec_module(toolchain)

GOOD = {
    'rust-toolchain.toml': '[toolchain]\nchannel = "1.99.0"\nprofile = "minimal"\ncomponents = ["clippy", "rustfmt"]\n',
    'apps/linux/Dockerfile.verify': 'FROM ubuntu:24.04\nARG RUST_VERSION=1.99.0\nRUN rustup-init -c clippy -c rustfmt\n',
    '.github/workflows/validate.yml': 'steps:\n  - run: rustup toolchain install\n',
    'docs/building.md': 'Rust 1.99.0 is pinned.\n',
}


class ToolchainCheckTests(unittest.TestCase):
    def check(self, **changes):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, text in {**GOOD, **changes}.items():
                (root / name).parent.mkdir(parents=True, exist_ok=True)
                (root / name).write_text(text)
            return toolchain.failures(root)

    def test_consistent_pins_pass(self):
        self.assertEqual(self.check(), [])

    def test_floating_channel_fails(self):
        self.assertTrue(self.check(**{'rust-toolchain.toml': '[toolchain]\nchannel = "stable"\n'}))

    def test_container_drift_fails(self):
        self.assertTrue(self.check(**{'apps/linux/Dockerfile.verify': 'ARG RUST_VERSION=1.98.0\nRUN rustup-init -c clippy -c rustfmt\n'}))
        self.assertTrue(self.check(**{'apps/linux/Dockerfile.verify': 'ARG RUST_VERSION=1.99.0\nRUN rustup-init -c clippy\n'}))

    def test_workflow_with_its_own_version_fails(self):
        for workflow in ('env:\n  RUST_VERSION: 1.99.0\n', 'run: rustup toolchain install 1.98.0\n',
                         'run: rustup toolchain install --profile minimal stable\n',
                         'run: rustup toolchain install\nrun: cargo +stable test\n',
                         'run: rustup override set stable\n', 'run: cargo test\n'):
            self.assertTrue(self.check(**{'.github/workflows/validate.yml': workflow}), workflow)

    def test_stale_documented_version_fails(self):
        for text in ('Install Rust 1.98.0.\n', 'Run `rustup toolchain install 1.98.0`.\n',
                     "`toolchain '1.98.0-aarch64-apple-darwin' is not installed`\n"):
            self.assertTrue(self.check(**{'docs/building.md': text}), text)


if __name__ == '__main__':
    unittest.main()
