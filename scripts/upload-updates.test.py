#!/usr/bin/env python3
"""Upload planning: order, caching, manifest and the published-history check.

Nothing here runs Wrangler or contacts the network."""
import importlib.util
from io import StringIO
from pathlib import Path, PurePosixPath
import shutil
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("upload_updates", HERE / "upload-updates.py")
upload = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upload)
apt = upload.apt
arch = upload.arch
OFFICIAL_KEY = HERE.parent / "config/profiles/official.apt.asc"
# The repo-add stand-in from the pacman repository tests.
spec = importlib.util.spec_from_file_location("arch_repository_test", HERE / "arch-repository.test.py")
arch_test = importlib.util.module_from_spec(spec)
spec.loader.exec_module(arch_test)


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

    def arch_repository(self, *versions):
        """A staged pacman repository, written the way publish-arch-repo.sh does."""
        root = self.dir / "arch"
        root.mkdir(exist_ok=True)
        (root / arch.KEY_FILE).write_bytes(OFFICIAL_KEY.read_bytes())
        for version in versions:
            for architecture in arch.ARCHITECTURES:
                directory = root / architecture
                directory.mkdir(exist_ok=True)
                name = f"{arch.PACKAGE}-{version}-{architecture}.pkg.tar.zst"
                (directory / name).write_bytes(f"{version} {architecture}".encode())
                (directory / f"{name}.sig").write_bytes(b"signature")
                arch_test.repo_add(root, architecture, name)
                arch.finalize(root, [architecture])
                for kind in arch.DATABASES:
                    for signature in (f"ycorp.{kind}.sig", f"ycorp.{kind}.tar.gz.sig"):
                        (directory / signature).write_bytes(b"database signature")
        arch.verify(root)
        return root

    def test_arch_order_and_caching(self):
        root = self.arch_repository("0.1.0beta2-1", "0.1.0beta3-1")
        steps, skipped = upload.plan(root, "arch", "bucket/email/arch", {})
        self.assertEqual(skipped, 0)
        names = [name for _, name, _, _, _ in steps]
        phases = [phase for phase, *_ in steps]
        self.assertEqual(phases, sorted(phases))
        caches = {name: cache for _, name, cache, _, _ in steps}
        packages = [n for n in names if ".pkg.tar.zst" in n]
        self.assertEqual(len(packages), 8)  # two versions, two architectures, each with its signature
        self.assertTrue(all(caches[n] == upload.IMMUTABLE for n in packages))
        self.assertEqual({n for n in names if caches[n] == upload.IMMUTABLE}, set(packages))
        last_package = max(names.index(n) for n in packages)
        databases = [i for i, n in enumerate(names) if "/ycorp." in n]
        self.assertLess(last_package, min(databases))
        self.assertLess(last_package, names.index(arch.KEY_FILE))
        # pacman -Sy reads ycorp.db and ycorp.db.sig, so they go last.
        self.assertEqual(sorted(names[-4:]), ["aarch64/ycorp.db", "aarch64/ycorp.db.sig", "x86_64/ycorp.db",
                                              "x86_64/ycorp.db.sig"])
        self.assertEqual(len(databases), 16)
        self.assertTrue(all(caches[names[i]] == upload.SHORT for i in databases))
        types = {name: content for _, name, _, content, _ in steps}
        self.assertEqual(types["x86_64/email-ycorp-bin-0.1.0beta3-1-x86_64.pkg.tar.zst"], "application/zstd")
        self.assertEqual(types["x86_64/email-ycorp-bin-0.1.0beta3-1-x86_64.pkg.tar.zst.sig"], "application/pgp-signature")
        self.assertEqual(types["x86_64/ycorp.db"], "application/gzip")
        self.assertEqual(types["x86_64/ycorp.files.tar.gz"], "application/gzip")
        self.assertEqual(types["x86_64/ycorp.db.sig"], "application/pgp-signature")
        self.assertEqual(types[arch.KEY_FILE], "application/pgp-keys")
        # The manifest guards packages here too.
        manifest = {"bucket/email/arch": {n: d for _, n, c, _, d in steps if c == upload.IMMUTABLE}}
        again, skipped = upload.plan(root, "arch", "bucket/email/arch", manifest)
        self.assertEqual(skipped, 8)
        (root / packages[0]).write_bytes(b"replaced")
        with self.assertRaises(upload.UploadError):
            upload.plan(root, "arch", "bucket/email/arch", manifest)

    def test_arch_unknown_files(self):
        root = self.arch_repository("0.1.0beta3-1")
        for name in ("x86_64/ycorp.db.tar.gz.old", "x86_64/email-ycorp-bin-0.1.0beta3-1-aarch64.pkg.tar.zst",
                     "x86_64/email-ycorp-0.1.0beta3-1-x86_64.pkg.tar.zst", "i686/ycorp.db", "ycorp.db"):
            with self.subTest(name):
                path = root / name
                path.parent.mkdir(exist_ok=True)
                path.write_text("stray")
                with self.assertRaises(upload.UploadError):
                    upload.plan(root, "arch", "bucket/email/arch", {})
                path.unlink()

    def test_arch_published_history_must_be_staged(self):
        published_root = self.arch_repository("0.1.0beta2-1")
        published = {a: (published_root / a / "ycorp.db").read_bytes() for a in arch.ARCHITECTURES}
        fetch = lambda url: next((data for a, data in published.items() if url.endswith(f"/{a}/ycorp.db")), None)
        base = "https://updates.example.test/email/arch"
        original = upload.fetch
        try:
            upload.fetch = fetch
            # The same repository, and one with a newer release added, both pass.
            upload.check_remote_arch(published_root, base)
            newer = self.arch_repository("0.1.0beta3-1")
            self.assertEqual(newer, published_root)
            upload.check_remote_arch(newer, base)
            # A staging directory without the published package...
            (newer / "x86_64/email-ycorp-bin-0.1.0beta2-1-x86_64.pkg.tar.zst").unlink()
            with self.assertRaises(upload.UploadError) as caught:
                upload.check_remote_arch(newer, base)
            self.assertIn("email-ycorp-bin-0.1.0beta2-1-x86_64.pkg.tar.zst", str(caught.exception))
            # ...or older than what is published, or with other bytes at the same version, is refused.
            for label, versions in (("older", ("0.1.0beta1-1",)), ("different", ("0.1.0beta2-1",))):
                with self.subTest(label):
                    shutil.rmtree(self.dir / "arch")
                    stale = self.arch_repository(*versions)
                    if label == "different":
                        for a in arch.ARCHITECTURES:
                            name = f"{arch.PACKAGE}-0.1.0beta2-1-{a}.pkg.tar.zst"
                            (stale / a / name).write_bytes(b"rebuilt")
                            arch_test.repo_add(stale, a, name)
                            arch.finalize(stale, [a])
                    with self.assertRaises(upload.UploadError):
                        upload.check_remote_arch(stale, base)
            # Nothing published yet: nothing to compare.
            upload.fetch = lambda url: None
            upload.check_remote_arch(stale, base)
        finally:
            upload.fetch = original

    def test_wrangler_is_pinned_exactly(self):
        self.assertRegex(upload.WRANGLER, r"^wrangler@4\.\d+\.\d+$")


if __name__ == "__main__":
    unittest.main()
