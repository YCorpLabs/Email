#!/usr/bin/env python3
"""Add synthetic reader-privacy fixtures to a disposable Email test store.

Adds a newsletter whose remote images are blocked by default next to an inline
cid: image reference. With --tracked it also adds a
sent message and a draft carrying open-tracking records, as a development build
would leave them. Uses reserved example domains; makes no network request.

Launch the app once on the store first so its schema exists. Refuses the
default macOS and Linux stores.
"""
import argparse
import base64
import json
from pathlib import Path
import sqlite3
import struct
import time
import zlib

# The demo: prefix keeps both shells from contacting a provider for it.
ACCOUNT = ("demo:privacy", "gmail", "alex@example.com", "Alex Morgan (fixture)")


def solid_png(size=24, rgb=(214, 92, 64)):
    """A small solid PNG: the shape of an inline cid: logo after bounded inlining."""
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    rows = b"".join(b"\0" + bytes(rgb) * size for _ in range(size))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


INLINE_PNG = base64.b64encode(solid_png()).decode()
# Demo accounts have no provider attachments, so the cid: part stays unresolved
# unless a harness supplies INLINE_PNG the way the bounded inliner would.
INLINE_CID = "logo@fixture.example"
NEWSLETTER = f"""<h2>Reader privacy fixture</h2>
<p>This synthetic newsletter mixes an inline image with remote images.</p>
<p><img src="cid:{INLINE_CID}" alt="Inline logo" width="24" height="24"> Inline cid: images render while remote images are blocked.</p>
<p><img src="https://images.example/hero.png" alt="Remote hero image" width="320" height="80"></p>
<table><tr><td style="background-image:url(https://images.example/band.png);padding:8px">Remote CSS background</td></tr></table>
<p>Reserved example domains only; nothing loads from a real server.</p>"""


def refuse_default(store):
    # Linux test runs isolate with XDG_DATA_HOME, so only the HOME-based
    # defaults identify a real everyday store.
    home = Path.home()
    defaults = {home / "Library/Application Support/Email/mail.sqlite", home / ".local/share/email-client/mail.sqlite"}
    if store.resolve() in {d.resolve() for d in defaults}:
        raise SystemExit("Refusing a default Email store; use a disposable EMAIL_STORE_PATH or XDG_DATA_HOME.")


def message(db, ident, subject, body_plain, body_html, at, *, sent=False, draft=False, unread=1):
    sender = f"{ACCOUNT[3]} <{ACCOUNT[2]}>" if sent or draft else "Studio Fixture <hello@studio.example>"
    to = ["casey@example.org"] if sent or draft else [ACCOUNT[2]]
    db.execute("INSERT OR REPLACE INTO threads(id,account_id,subject,sender,snippet,last_message_at,unread_count,is_inbox,folder)"
               " VALUES(?,?,?,?,?,?,?,?,?)", (ident, ACCOUNT[0], subject, sender, body_plain[:120], at,
                                               0 if sent or draft else unread, 0 if sent or draft else 1,
                                               "Sent" if sent else ("Drafts" if draft else "Inbox")))
    db.execute("INSERT OR REPLACE INTO messages(id,thread_id,account_id,sender,to_json,subject,body_plain,body_html,"
               "unsubscribe_header,has_attachments,received_at,is_read,is_draft,is_sent) VALUES(?,?,?,?,?,?,?,?,NULL,0,?,?,?,?)",
               (ident, ident, ACCOUNT[0], sender, json.dumps(to), subject, body_plain, body_html, at,
                0 if unread and not (sent or draft) else 1, int(draft), int(sent)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--store", type=Path, required=True, help="Disposable mail.sqlite created by the app")
    parser.add_argument("--tracked", action="store_true", help="Also add tracked sent mail and a tracked draft")
    args = parser.parse_args()
    refuse_default(args.store)
    if not args.store.is_file():
        raise SystemExit("Launch Email once on this store so its schema exists.")
    now = int(time.time() * 1000)
    with sqlite3.connect(args.store) as db:
        db.execute("INSERT OR IGNORE INTO accounts(id,provider,email,display_name) VALUES(?,?,?,?)", ACCOUNT)
        message(db, "demo:privacy-newsletter", "Reader privacy fixture",
                "This synthetic newsletter mixes an inline image with remote images.", NEWSLETTER, now)
        message(db, "demo:privacy-plain", "Plain text fixture",
                "A message without remote images, for list navigation checks.", None, now - 120_000)
        if args.tracked:
            db.execute("CREATE TABLE IF NOT EXISTS email_tracking(message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE ON UPDATE CASCADE,"
                       "tracking_id TEXT NOT NULL UNIQUE,enabled INTEGER NOT NULL DEFAULT 0,host_id TEXT,pixel_requests INTEGER NOT NULL DEFAULT 0,"
                       "first_requested_at INTEGER,last_requested_at INTEGER)")
            message(db, "demo:tracked-sent", "Tracked proposal (fixture)", "Sent from a development build with Track opens.",
                    None, now - 3_600_000, sent=True)
            message(db, "demo:tracked-draft", "Tracked draft (fixture)", "Draft saved by a development build with Track opens.",
                    None, now - 60_000, draft=True)
            for ident, tracking, count in (("demo:tracked-sent", "fixture-tracking-sent", 3),
                                           ("demo:tracked-draft", "fixture-tracking-draft", 0)):
                db.execute("INSERT OR REPLACE INTO email_tracking(message_id,tracking_id,enabled,host_id,pixel_requests,first_requested_at,last_requested_at)"
                           " VALUES(?,?,1,'fixture-host',?,?,?)", (ident, tracking, count, now - 1_800_000 if count else None,
                                                                 now - 600_000 if count else None))
    print(f"Seeded reader privacy fixture into {args.store}{' with tracked mail' if args.tracked else ''}")


if __name__ == "__main__":
    main()
