import Foundation

@main enum RemoteImagePreferenceChecks {
    static func main() {
        func fresh(_ seed: [String: Any] = [:]) -> (UserDefaults, String) {
            let suite = "email.remote-image-checks.\(UUID().uuidString)"
            let defaults = UserDefaults(suiteName: suite)!
            for (key, value) in seed { defaults.set(value, forKey: key) }
            return (defaults, suite)
        }
        func done(_ suite: String) { UserDefaults.standard.removePersistentDomain(forName: suite) }
        let enabled = RemoteImagePreference.enabledKey, choice = RemoteImagePreference.explicitChoiceKey

        // A new install blocks remote images.
        var (defaults, suite) = fresh()
        RemoteImagePreference.prepare(defaults)
        precondition(!RemoteImagePreference.isEnabled(defaults), "new install blocks remote images")
        precondition(!defaults.bool(forKey: choice) && defaults.object(forKey: enabled) == nil, "a new install records no choice")
        done(suite)

        // The earlier one-time migration stored true for everyone: not a choice.
        (defaults, suite) = fresh([enabled: true, "EmailRemoteImagesDefaultV2": true])
        RemoteImagePreference.prepare(defaults)
        precondition(!RemoteImagePreference.isEnabled(defaults), "forced legacy default falls back to blocking")
        precondition(defaults.persistentDomain(forName: suite)?[enabled] == nil, "forced legacy value is removed")
        done(suite)

        // Before the forced default, the value was stored only by the user's toggle.
        (defaults, suite) = fresh([enabled: true])
        RemoteImagePreference.prepare(defaults)
        precondition(RemoteImagePreference.isEnabled(defaults) && defaults.bool(forKey: choice), "a pre-November choice to load is kept")
        done(suite)

        // Only the user could have stored false; keep it and remember it as a choice.
        (defaults, suite) = fresh([enabled: false, "EmailRemoteImagesDefaultV2": true])
        RemoteImagePreference.prepare(defaults)
        precondition(!RemoteImagePreference.isEnabled(defaults) && defaults.bool(forKey: choice), "explicit block is kept")
        done(suite)

        // An explicit choice to load images survives the migration and relaunches.
        (defaults, suite) = fresh()
        RemoteImagePreference.prepare(defaults)
        RemoteImagePreference.setEnabled(true, in: defaults)
        RemoteImagePreference.prepare(defaults)
        precondition(RemoteImagePreference.isEnabled(defaults), "explicit load survives relaunch")
        done(suite)
        (defaults, suite) = fresh([enabled: true, choice: true])
        RemoteImagePreference.prepare(defaults)
        precondition(RemoteImagePreference.isEnabled(defaults), "recorded explicit load survives the migration")
        done(suite)

        // The migration runs once: a later explicit choice is never reset.
        (defaults, suite) = fresh([enabled: true, "EmailRemoteImagesDefaultV2": true])
        RemoteImagePreference.prepare(defaults)
        defaults.set(true, forKey: enabled)
        defaults.removeObject(forKey: choice)
        RemoteImagePreference.prepare(defaults)
        precondition(RemoteImagePreference.isEnabled(defaults), "migration runs once")
        done(suite)

        // Detection matches what the reader's content security policy would block.
        for html in [#"<img src="https://example.com/a.png">"#, #"<IMG alt="x" SRC='https://example.com/a.png'>"#,
                     #"<td style="background:url(https://example.com/b.png)">"#,
                     #"<td style="background-image: url(&quot;https://example.com/b.png&quot;)">"#,
                     #"<td style="background-image: image-set(&quot;https://example.com/b.png&quot; 1x)">"#,
                     #"<td style="background:u\72l(https://example.com/b.png)">"#,
                     #"<img alt="Save > 50%" src="https://cdn.example/x.png">"#] {
            precondition(RemoteImagePreference.containsRemoteImages(html), "detects \(html)")
        }
        for html in [#"<img src="data:image/png;base64,AAAA">"#, #"<img src="cid:logo@example">"#,
                     #"<td style="background:url(http://example.com/never-allowed.png)">"#,
                     #"<a href="https://example.com">https://example.com</a>"#, "<p>Plain text</p>",
                     #"<p style="color:red">See url(https://example.com) in text</p>"#,
                     #"<img alt="Use src=https://cdn.example/x" src="cid:logo">"#] {
            precondition(!RemoteImagePreference.containsRemoteImages(html), "ignores \(html)")
        }
        precondition(RemoteImagePreference.controlTooltip(allowedHere: true, loadByDefault: false) == "Remote images are loaded in this email. Image options · I to always load remote images by default")
        precondition(RemoteImagePreference.controlTooltip(allowedHere: false, loadByDefault: true) == "Remote images are blocked. Image options · I to block remote images by default")
        precondition(RemoteImagePreference.loadButtonTooltip(loadByDefault: true) == "Load remote images in this email. I to block remote images by default")
        print("Remote image preference checks passed")
    }
}
