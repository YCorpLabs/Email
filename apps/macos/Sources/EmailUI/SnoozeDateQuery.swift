import Foundation

/// Local calendar phrases. Never infer a different date from an expired explicit time.
enum SnoozeDateQuery {
    static func normalize(_ input: String) -> String {
        var words = input.lowercased().split(whereSeparator: { $0.isWhitespace }).map(String.init)
        if let verb = words.first, ["remind", "snooze", "snooz", "snoz", "snoze"].contains(verb) {
            words.removeFirst()
            if words.first == "me" { words.removeFirst() }
            if let word = words.first, ["in", "for", "at", "on"].contains(word) { words.removeFirst() }
        }
        return words.joined(separator: " ")
    }
    static func parse(_ input: String, now: Date = Date(), calendar: Calendar = .current) -> Date? {
        let query = normalize(input)
        let names = ["tonight", "this evening", "tomorrow morning", "tomorrow evening", "tomorrow", "today"]
        guard let name = names.first(where: { query == $0 || query.hasPrefix($0 + " ") }) else { return nil }
        var clock = String(query.dropFirst(name.count)).trimmingCharacters(in: .whitespaces)
        if clock.hasPrefix("at ") { clock = String(clock.dropFirst(3)) }
        let explicit = !clock.isEmpty
        var hour = name.contains("evening") || name == "tonight" ? 20 : 9
        var minute = 0
        if explicit {
            clock = clock.replacingOccurrences(of: " ", with: "")
            let meridiem = clock.hasSuffix("am") ? "am" : (clock.hasSuffix("pm") ? "pm" : "")
            if !meridiem.isEmpty { clock = String(clock.dropLast(2)) }
            let parts = clock.split(separator: ":", omittingEmptySubsequences: false)
            guard (1...2).contains(parts.count), let h = Int(parts[0]),
                  let m = parts.count == 2 ? Int(parts[1]) : 0, (0...59).contains(m),
                  (meridiem.isEmpty ? (0...23).contains(h) : (1...12).contains(h)) else { return nil }
            hour = meridiem.isEmpty ? h : h % 12 + (meridiem == "pm" ? 12 : 0)
            minute = m
        }
        guard let day = calendar.date(byAdding: .day, value: name.hasPrefix("tomorrow") ? 1 : 0, to: now),
              let date = calendar.date(bySettingHour: hour, minute: minute, second: 0, of: day) else { return nil }
        if date > now { return date }
        // Late evening: offer the next whole hour, only while it is still tonight.
        if !explicit && (name == "tonight" || name == "this evening"),
           let next = calendar.nextDate(after: now, matching: DateComponents(minute: 0, second: 0), matchingPolicy: .nextTime),
           calendar.isDate(next, inSameDayAs: now) { return next }
        return nil
    }
}
