import Foundation
var calendar=Calendar(identifier:.gregorian);calendar.timeZone=TimeZone(secondsFromGMT:0)!
let format=DateFormatter();format.dateFormat="yyyy-MM-dd";format.timeZone=calendar.timeZone;format.calendar=calendar
let now=format.date(from:"2026-11-09")!
let cases:[(String,String?)]=[("go to 20 october","2026-10-20"),("20 October","2026-10-20"),("october 20","2026-10-20"),("20th October 2027","2027-10-20"),("in 2 weeks","2026-11-23"),("+2w","2026-11-23"),("3 days ago","2026-11-06"),("next Friday","2026-11-13"),("next Monday","2026-11-16"),("last monday","2026-11-02"),("tomorrow","2026-11-10"),("2027-01-02","2027-01-02"),("32 october",nil),("31 february",nil),("2026-02-30",nil),("2026-02-29",nil),("20 october blah",nil),("-9223372036854775808d",nil),("-9223372036854775808d ago",nil),("nonsense",nil),("2 hours",nil),("999999999999999w",nil)]
for (query,expected) in cases {let result=AgendaDateQuery.parse(query,now:now,calendar:calendar).map{format.string(from:$0)};if result != expected {fatalError("\(query): expected \(String(describing:expected)), got \(String(describing:result))")}}
print("Passed \(cases.count) calendar date query cases")
