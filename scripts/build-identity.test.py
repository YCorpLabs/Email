#!/usr/bin/env python3
import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location('build_identity', Path(__file__).with_name('build-identity.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class BuildIdentityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.saved_root = module.ROOT
        module.ROOT = self.root
        for name in module.SOURCE_DIRS:
            (self.root / name).mkdir()
        self.source = self.root / 'crates/client.rs'
        self.source.write_text('pub fn client() {}')

    def tearDown(self):
        module.ROOT = self.saved_root
        self.temp.cleanup()

    def test_ignored_local_files_do_not_change_identity(self):
        subprocess.run(['git', 'init', '--quiet', str(self.root)], check=True)
        (self.root / '.gitignore').write_text('.secrets/\n.wrangler/\n*.local.json\n')
        original = module.build_id()
        for name in ['apps/.secrets/client.json', 'apps/.wrangler/cache.json', 'apps/auth.local.json']:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('{"test_only": "not-a-real-credential"}')
        self.assertEqual(original, module.build_id())
        self.source.write_text('pub fn changed() {}')
        self.assertNotEqual(original, module.build_id())

    def test_archive_excludes_secret_and_runtime_directories(self):
        original = module.build_id()
        for name in ['apps/.secrets/client.json', 'apps/.wrangler/cache.json', 'crates/target/generated.rs']:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('fixture')
        self.assertEqual(original, module.build_id())
        (self.root / 'apps/feature.swift').write_text('struct Feature {}')
        self.assertNotEqual(original, module.build_id())

if __name__ == '__main__':
    unittest.main()
