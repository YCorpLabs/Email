import Foundation

@main enum DistributionProfileChecks {
    static func profile(_ object: [String: Any?]) -> DistributionProfile? {
        var json: [String: Any] = ["profile": "fixture", "macos_data_directory": "invalid.email.fixture"]
        for (key, value) in object { json[key] = value ?? NSNull() }
        return DistributionProfile(json: try! JSONSerialization.data(withJSONObject: json))
    }

    static func main() {
        let configured = profile(["support_address": "help@example.test", "feedback_address": "ideas@example.test",
                                  "scheduling_origin": "https://scheduling.example.test"])!
        precondition(configured.supportAddress == "help@example.test" && configured.feedbackAddress == "ideas@example.test")
        precondition(configured.isFeedbackRecipient("Ideas <IDEAS@example.test>"))
        precondition(configured.isFeedbackRecipient("other@example.test, ideas@example.test"))
        precondition(!configured.isFeedbackRecipient("help@example.test"))
        precondition(!configured.isFeedbackRecipient("ideas@example.test.evil.test"))
        precondition(configured.hasScheduling)
        // Open tracking is off unless the engine says so, and never without Scheduling.
        precondition(!configured.openTracking)
        precondition(profile(["scheduling_origin": "https://scheduling.example.test", "open_tracking": true])!.openTracking)
        precondition(!profile(["scheduling_origin": nil, "open_tracking": true])!.openTracking)
        precondition(configured.schedulingURL(path: "/studio", fragment: "analytics")?.absoluteString == "https://scheduling.example.test/studio#analytics")
        precondition(configured.isSchedulingURL(URL(string: "https://scheduling.example.test/device#code=ABCD")!))
        precondition(!configured.isSchedulingURL(URL(string: "https://scheduling.example.test.evil.test/device")!))
        precondition(!configured.isSchedulingURL(URL(string: "http://scheduling.example.test/device")!))
        precondition(!configured.isSchedulingURL(URL(string: "https://scheduling.example.test:8443/device")!))
        precondition(configured.isSchedulingURL(URL(string: "https://scheduling.example.test:443/device")!))

        // The setup guide link needs the engine's https URL and a support channel.
        let guided = profile(["support_address": "help@example.test", "feedback_address": "ideas@example.test",
                              "setup_guide_url": "https://guide.example.test/email/guide/"])!
        precondition(guided.setupGuideURL("gmail")?.absoluteString == "https://guide.example.test/email/guide/#gmail")
        precondition(configured.setupGuideURL("gmail") == nil)
        precondition(profile(["support_address": "help@example.test", "feedback_address": "ideas@example.test",
                              "setup_guide_url": "http://guide.example.test/"])!.setupGuide == nil)
        precondition(profile(["setup_guide_url": "https://guide.example.test/"])!.setupGuide == nil, "no support channel, no guide")

        // Unconfigured profiles never fall back to any address or service.
        let neutral = profile(["support_address": nil, "feedback_address": nil, "scheduling_origin": nil])!
        precondition(neutral.supportAddress == nil && neutral.feedbackAddress == nil && !neutral.hasScheduling)
        precondition(!neutral.isFeedbackRecipient("feedback@example.test"))
        precondition(neutral.schedulingURL(path: "/studio") == nil)
        precondition(!neutral.isSchedulingURL(URL(string: "https://scheduling.example.test/device")!))

        // Partial or malformed values are treated as absent.
        precondition(profile(["support_address": "help@example.test"])!.supportAddress == nil)
        precondition(profile(["support_address": "same@example.test", "feedback_address": "SAME@example.test"])!.supportAddress == nil)
        precondition(profile(["support_address": "a@example.test, b@example.test", "feedback_address": "c@example.test"])!.supportAddress == nil)
        precondition(profile(["scheduling_origin": "http://scheduling.example.test"])!.schedulingOrigin == nil)
        precondition(profile(["scheduling_origin": "https://scheduling.example.test/path"])!.schedulingOrigin == nil)
        precondition(profile(["macos_data_directory": "../Email"]) == nil)
        precondition(profile(["macos_data_directory": ".hidden"]) == nil)
        precondition(profile(["profile": ""]) == nil)
        precondition(DistributionProfile(json: Data("not json".utf8)) == nil)
        print("Distribution profile checks passed (30 scenarios)")
    }
}
