#!/usr/bin/env python3
"""Upload planning: order, caching, manifest and the published-history check.

Nothing here runs Wrangler or contacts the network."""
import importlib.util
from io import StringIO
from pathlib import Path, PurePosixPath
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("upload_updates", HERE / "upload-updates.py")
upload = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upload)
apt = upload.apt
OFFICIAL_KEY = HERE.parent / "config/profiles/official.apt.asc"


class Planning(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.dir = Path(scratch.name)

    def apt_repository(self):
        """A repository with two packages, built with the real index code."""
        root = self.dir / "apt"
        keyring = self.dir / "fixture.apt.asc"
        keyring.write_bytes(OFFICIAL_KEY.read_bytes())
        profile = {"LINUX_APT_REPOSITORY": "https://updates.example.test/email/apt", "keyring": str(keyring),
                   "LINUX_APP_ID": "com.example.Mail"}
        for index, version in enumerate(("1:0.2.0~beta.1", "1:0.2.0")):
            release = apt.release_version(version)
            deb = self.dir / f"in-{index}" / f"email-linux_{release}_arm64.deb"
            deb.parent.mkdir()
            deb.write_bytes(version.encode())
            inspected = self.dir / f"inspected-{index}"
            inspected.mkdir()
            (inspected / "control").write_text(f"Package: email-linux\nVersion: {version}\nArchitecture: arm64\n")
            (inspected / "conffiles").write_text(f"{apt.SOURCES_PATH}\n{apt.KEYRING_PATH}\n")
            (inspected / "sources").write_text(apt.sources_text(profile["LINUX_APT_REPOSITORY"], apt.suite_for(release), "arm64"))
            (inspected / "keyring").write_bytes(keyring.read_bytes())
            (inspected / "channel").write_text("distribution\n")
            (inspected / "contents").write_text("./usr/share/applications/com.example.Mail.desktop\n")
            apt.add(root, [(str(deb), str(inspected))], profile, out=StringIO())
            for suite in ("beta", "stable"):
                if (root / "dists" / suite / "Release").is_file():
                    (root / "dists" / suite / "InRelease").write_text("signed\n")
                    (root / "dists" / suite / "Release.gpg").write_text("signature\n")
        return root

    def test_apt_order_and_caching(self):
        root = self.apt_repository()
        steps, skipped = upload.plan(root, "apt", "bucket/email/apt", {})
        self.assertEqual(skipped, 0)
        names = [name for _, name, _, _, _ in steps]
        phases = [phase for phase, *_ in steps]
        self.assertEqual(phases, sorted(phases))
        immutable = [name for phase, name, cache, _, _ in steps if cache == upload.IMMUTABLE]
        self.assertTrue(all(n.startswith("pool/") or "/by-hash/" in n for n in immutable))
        self.assertEqual(sum(n.startswith("pool/") for n in names), 2)
        last_immutable = max(names.index(n) for n in immutable)
        first_index = min(i for i, n in enumerate(names) if n.endswith(("/Packages", "/Packages.gz")))
        first_release = min(i for i, n in enumerate(names) if n.endswith(("/Release", "/Release.gpg")))
        inreleases = [i for i, n in enumerate(names) if n.endswith("/InRelease")]
        self.assertLess(last_immutable, first_index)
        self.assertLess(first_index, first_release)
        self.assertLess(first_release, min(inreleases))
        self.assertEqual(max(inreleases), len(names) - 1)
        self.assertIn("email-linux-archive-keyring.asc", names)
        types = {name: content for _, name, _, content, _ in steps}
        self.assertEqual(types["pool/main/e/email-linux/email-linux_0.2.0_arm64.deb"], "application/vnd.debian.binary-package")
        self.assertEqual(types["dists/stable/main/binary-arm64/Packages.gz"], "application/gzip")
        self.assertEqual(types["dists/stable/InRelease"], "text/plain; charset=utf-8")
        self.assertEqual(types["dists/stable/Release.gpg"], "application/pgp-signature")
        caches = {name: cache for _, name, cache, _, _ in steps}
        self.assertEqual(caches["dists/stable/InRelease"], "public, max-age=300")
        self.assertEqual(caches["dists/stable/main/binary-arm64/Packages"], "public, max-age=300")

    def test_manifest_skips_uploaded_versioned_files_and_guards_them(self):
        root = self.apt_repository()
        steps, _ = upload.plan(root, "apt", "bucket/email/apt", {})
        manifest = {"bucket/email/apt": {name: digest for _, name, cache, _, digest in steps if cache == upload.IMMUTABLE}}
        again, skipped = upload.plan(root, "apt", "bucket/email/apt", manifest)
        self.assertEqual(skipped, len(manifest["bucket/email/apt"]))
        self.assertTrue(all(cache == upload.SHORT for _, _, cache, _, _ in again))
        # Another destination has its own record.
        self.assertEqual(upload.plan(root, "apt", "bucket/other/apt", manifest)[1], 0)
        pool = next(n for n in manifest["bucket/email/apt"] if n.startswith("pool/"))
        (root / pool).write_bytes(b"replaced")
        with self.assertRaises(upload.UploadError):
            upload.plan(root, "apt", "bucket/email/apt", manifest)

    def test_unknown_files_and_dotfiles(self):
        root = self.apt_repository()
        (root / ".upload-manifest.json").write_text("{}")
        (root / ".DS_Store").write_text("")
        self.assertNotIn(PurePosixPath(".upload-manifest.json"), upload.files(root))
        (root / "notes.txt").write_text("stray")
        with self.assertRaises(upload.UploadError):
            upload.plan(root, "apt", "bucket/email/apt", {})

    def test_macos_feed_order(self):
        feed = self.dir / "macos"
        feed.mkdir()
        for name in ("Email-0.2.0-3-arm64.zip", "Email-0.2.0-3-arm64.md", "Email2-3.delta", "appcast.xml"):
            (feed / name).write_text(name)
        steps, _ = upload.plan(feed, "macos", "bucket/email/macos", {})
        self.assertEqual([name for _, name, _, _, _ in steps],
                         ["Email-0.2.0-3-arm64.zip", "Email2-3.delta", "Email-0.2.0-3-arm64.md", "appcast.xml"])
        self.assertEqual(steps[0][2], upload.IMMUTABLE)
        self.assertEqual(steps[-1][2], upload.SHORT)
        (feed / "sub").mkdir()
        (feed / "sub/x.zip").write_text("x")
        with self.assertRaises(upload.UploadError):
            upload.plan(feed, "macos", "bucket/email/macos", {})

    def test_published_history_must_be_staged(self):
        root = self.apt_repository()
        published = (root / "dists/beta/main/binary-arm64/Packages").read_bytes()
        original = upload.fetch
        try:
            upload.fetch = lambda url: published if url.endswith("dists/beta/main/binary-arm64/Packages") else None
            upload.check_remote_apt(root, "https://updates.example.test/email/apt")
            extra = published + b"\nPackage: email-linux\nVersion: 1:0.1.9\nArchitecture: arm64\n" \
                b"Filename: pool/main/e/email-linux/email-linux_0.1.9_arm64.deb\nSize: 1\nSHA256: 00\n"
            upload.fetch = lambda url: extra if url.endswith("dists/beta/main/binary-arm64/Packages") else None
            with self.assertRaises(upload.UploadError) as caught:
                upload.check_remote_apt(root, "https://updates.example.test/email/apt")
            self.assertIn("email-linux_0.1.9_arm64.deb", str(caught.exception))
            feed = self.dir / "feed"
            feed.mkdir()
            item = ('<item><sparkle:version>{v}</sparkle:version><enclosure url="https://u.test/E-{v}.zip" length="{n}" '
                    'sparkle:edSignature="sig{v}"/></item>')
            appcast = ('<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel>{}</channel></rss>')
            (feed / "appcast.xml").write_text(appcast.format(item.format(v=2, n=10) + item.format(v=3, n=11)))
            upload.fetch = lambda url: appcast.format(item.format(v=2, n=10)).encode()
            upload.check_remote_macos(feed, "https://u.test/email/macos")
            upload.fetch = lambda url: appcast.format(item.format(v=1, n=9)).encode()
            with self.assertRaises(upload.UploadError):
                upload.check_remote_macos(feed, "https://u.test/email/macos")
            upload.fetch = lambda url: appcast.format(item.format(v=2, n=99)).encode()
            with self.assertRaises(upload.UploadError):
                upload.check_remote_macos(feed, "https://u.test/email/macos")
        finally:
            upload.fetch = original

    def test_wrangler_is_pinned_exactly(self):
        self.assertRegex(upload.WRANGLER, r"^wrangler@4\.\d+\.\d+$")


if __name__ == "__main__":
    unittest.main()
