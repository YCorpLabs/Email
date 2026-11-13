import Foundation
var calendar = Calendar(identifier: .gregorian)
calendar.timeZone = TimeZone(secondsFromGMT: 0)!
let formatter = DateFormatter(); formatter.dateFormat = "yyyy-MM-dd HH:mm"; formatter.timeZone = calendar.timeZone
let now = formatter.date(from: "2026-11-10 10:00")!
let cases: [(String, String?)] = [("remind tonight", "2026-11-10 20:00"), ("tonight", "2026-11-10 20:00"), ("remind me tonight at 8pm", "2026-11-10 20:00"), ("snooze tomorrow 9", "2026-11-11 09:00"), ("tomorrow morning", "2026-11-11 09:00"), ("today at 09:00", nil), ("tonight 25:00", nil), ("remind tonight maybe", nil), ("tonight 8:99pm", nil)]
for (input, expected) in cases {
    let result = SnoozeDateQuery.parse(input, now: now, calendar: calendar).map { formatter.string(from: $0) }
    precondition(result == expected, "\(input): \(String(describing: result)) != \(String(describing: expected))")
}
let late = formatter.date(from: "2026-11-10 21:30")!
precondition(formatter.string(from: SnoozeDateQuery.parse("tonight", now: late, calendar: calendar)!) == "2026-11-10 22:00")
print("Passed 10 snooze calendar phrase cases")
