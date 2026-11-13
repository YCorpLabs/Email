import Foundation

/// The remembered remote-image setting. Remote images are blocked unless the
/// user chose to load them by default.
///
/// The 7 November 2026 build wrote `true` for everyone once and set
/// `EmailRemoteImagesDefaultV2`; with that marker, a stored `true` without the
/// explicit-choice marker is not a user decision and falls back to blocking.
/// Earlier builds stored the value only when the user toggled it, so without
/// the marker a stored value is a choice. A stored `false` is always kept.
enum RemoteImagePreference {
    static let enabledKey = "EmailLoadRemoteImages"
    static let explicitChoiceKey = "EmailRemoteImagesUserChoice"
    static let migrationKey = "EmailRemoteImagesDefaultV3"
    /// Set by the 7 November 2026 build when it forced `true` for everyone.
    static let forcedDefaultKey = "EmailRemoteImagesDefaultV2"

    /// Runs once. Nothing is registered for `enabledKey`: an unset value reads
    /// as blocked and stays distinguishable from a stored choice.
    static func prepare(_ defaults: UserDefaults) {
        guard !defaults.bool(forKey: migrationKey) else { return }
        if !defaults.bool(forKey: explicitChoiceKey), let stored = defaults.object(forKey: enabledKey) as? Bool {
            if stored && defaults.bool(forKey: forcedDefaultKey) { defaults.removeObject(forKey: enabledKey) }
            else { defaults.set(true, forKey: explicitChoiceKey) }
        }
        defaults.set(true, forKey: migrationKey)
    }

    static func isEnabled(_ defaults: UserDefaults) -> Bool { defaults.bool(forKey: enabledKey) }

    /// Tooltip of the notice's Load images button, identical on Linux.
    static func loadButtonTooltip(loadByDefault: Bool) -> String {
        "Load remote images in this email. I to \(loadByDefault ? "block" : "always load") remote images by default"
    }

    /// Image-control tooltip, identical on Linux. `I` always changes the
    /// remembered default, never just the current email.
    static func controlTooltip(allowedHere: Bool, loadByDefault: Bool) -> String {
        let state = allowedHere ? (loadByDefault ? "Remote images load automatically" : "Remote images are loaded in this email") : "Remote images are blocked"
        return "\(state). Image options · I to \(loadByDefault ? "block" : "always load") remote images by default"
    }

    static func setEnabled(_ enabled: Bool, in defaults: UserDefaults) {
        defaults.set(enabled, forKey: enabledKey)
        defaults.set(true, forKey: explicitChoiceKey)
    }

    /// Sanitized HTML with images that "Load images" would fetch: an `<img>`
    /// with an HTTPS source, or an HTTPS URL anywhere in a `style` attribute
    /// (`url()`, `image-set()`, escaped forms). The sanitizer drops `<style>`
    /// elements, and the reader allows only `https:` (the service upgrades
    /// `<img>` http sources). Same-message `cid:` images arrive as `data:` URLs.
    static func containsRemoteImages(_ html: String) -> Bool {
        html.range(of: #"(?is)<img\b(?:[^>"']|"[^"]*"|'[^']*')*?\bsrc\s*=\s*["']?\s*https://|\bstyle\s*=\s*(?:"[^"]*https://|'[^']*https://)"#, options: .regularExpression) != nil
    }
}
