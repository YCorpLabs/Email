import Foundation

/// Calendar commands are resolved locally; typing never starts a provider request.
enum AgendaDateQuery {
  static func parse(_ query: String, now: Date = Date(), calendar: Calendar = .current) -> Date? {
    var raw = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
      .replacingOccurrences(of: #"\s+"#, with: " ", options: .regularExpression)
    raw = raw.replacingOccurrences(of: #"^(?:go to|goto|jump to|jump|date|on)\s+"#, with: "", options: .regularExpression)
    let today = calendar.startOfDay(for: now)
    func add(_ component: Calendar.Component, _ count: Int) -> Date? {
      guard (-100_000...100_000).contains(count) else { return nil }
      return calendar.date(byAdding: component, value: count, to: today)
    }
    switch raw {
    case "today", "now": return today
    case "tomorrow": return add(.day, 1)
    case "yesterday": return add(.day, -1)
    default: break
    }
    func captures(_ pattern: String) -> [String]? {
      guard let regex = try? NSRegularExpression(pattern: pattern),
            let match = regex.firstMatch(in: raw, range: NSRange(raw.startIndex..., in: raw)) else { return nil }
      return (1..<match.numberOfRanges).map { Range(match.range(at: $0), in: raw).map { String(raw[$0]) } ?? "" }
    }
    func component(_ unit: String) -> Calendar.Component {
      switch unit.first { case "w": return .weekOfYear; case "m": return .month; case "y": return .year; default: return .day }
    }
    if let parts = captures(#"^(next|last) (day|week|month|year)$"#) {
      return add(component(parts[1]), parts[0] == "next" ? 1 : -1)
    }
    if let parts = captures(#"^(?:in )?([+-]?\d+)\s*(d|days?|w|weeks?|m|months?|y|years?)( ago)?$"#), let count = Int(parts[0]), (-100_000...100_000).contains(count) {
      return add(component(parts[1]), parts[2].isEmpty ? count : -count)
    }
    let weekdays = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"]
    if let parts = captures(#"^(?:(next|last) )?([a-z]+)$"#),
       let index = weekdays.firstIndex(where: { $0 == parts[1] || String($0.prefix(3)) == parts[1] }) {
      let current = calendar.component(.weekday, from: today) - 1
      var delta = (index - current + 7) % 7
      if parts[0] == "next", delta == 0 { delta = 7 }
      if parts[0] == "last" { delta = delta == 0 ? -7 : delta - 7 }
      return add(.day, delta)
    }
    raw = raw.replacingOccurrences(of: #"(?<=\d)(st|nd|rd|th)\b"#, with: "", options: .regularExpression)
      .replacingOccurrences(of: ",", with: "")
    let formatter = DateFormatter(); formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.calendar = calendar; formatter.timeZone = calendar.timeZone; formatter.isLenient = false
    let year = calendar.component(.year, from: today)
    for pattern in ["yyyy-MM-dd", "d.M.yyyy", "d/M/yyyy", "d MMM yyyy", "d MMMM yyyy", "MMM d yyyy", "MMMM d yyyy", "d MMM", "d MMMM", "MMM d", "MMMM d"] {
      let hasYear = pattern.contains("yyyy")
      formatter.dateFormat = hasYear ? pattern : pattern + " yyyy"
      let input = hasYear ? raw : raw + " \(year)"
      if let date = formatter.date(from: input), calendar.component(.year, from: date) > 0 {
        return calendar.startOfDay(for: date)
      }
    }
    return nil
  }
}
