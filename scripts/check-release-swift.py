#!/usr/bin/env python3
"""Compile existing isolated Swift rule harnesses with assertions enabled."""

from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "apps/macos/Sources/EmailUI"
TESTS = ROOT / "apps/macos/Tests"
CASES = (
    ("ComposerAddressChecks", ("ComposerAddressRules.swift",), "ComposerAddressChecks.swift"),
    ("DistributionProfileChecks", ("ComposerAddressRules.swift", "DistributionProfile.swift"),
     "DistributionProfileChecks.swift"),
    ("RecipientCompletionChecks", ("ComposerAddressRules.swift", "RecipientCompletionRules.swift"),
     "RecipientCompletionChecks.swift"),
    ("NotificationTrackerChecks", ("MailArrivalTracker.swift",), "NotificationTrackerChecks.swift"),
    ("SearchFilterChecks", ("SearchFilterRules.swift",), "SearchFilterChecks.swift"),
    ("ReaderLinkPolicyChecks", ("ReaderLinkPolicy.swift",), "ReaderLinkPolicyChecks.swift"),
    ("RemoteImagePreferenceChecks", ("RemoteImagePreference.swift",), "RemoteImagePreferenceChecks.swift"),
    ("CalendarDateQuery", ("AgendaDateQuery.swift",), "CalendarDateQuery/main.swift"),
    ("CalendarEventQuery", ("AgendaDateQuery.swift", "AgendaEventQuery.swift"),
     "CalendarEventQuery/main.swift"),
    ("SnoozeDateQuery", ("AgendaDateQuery.swift", "SnoozeDateQuery.swift"), "SnoozeDateQuery/main.swift"),
)


def main():
    with tempfile.TemporaryDirectory(prefix="email-swift-checks-") as scratch:
        for name, sources, harness in CASES:
            output = str(Path(scratch) / name)
            print(f"Compiling {name}", flush=True)
            subprocess.run(["swiftc", *(str(SOURCE / path) for path in sources),
                            str(TESTS / harness), "-o", output], check=True)
            subprocess.run([output], check=True)
    print(f"All {len(CASES)} isolated Swift harnesses passed")


if __name__ == "__main__":
    main()
