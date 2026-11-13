import Foundation
var calendar=Calendar(identifier:.gregorian);calendar.timeZone=TimeZone(secondsFromGMT:0)!
let formatter=DateFormatter();formatter.calendar=calendar;formatter.timeZone=calendar.timeZone;formatter.dateFormat="yyyy-MM-dd HH:mm"
let now=formatter.date(from:"2026-11-09 15:00")!,selected=formatter.date(from:"2026-12-04 09:00")!
let cases:[(String,String?,String?,Int?)]=[
("Lunch tomorrow at 12 for 1h","Lunch","2026-11-10 12:00",60),
("Design review next Friday at 2:30pm for 45m","Design review","2026-11-13 14:30",45),
("lunch 12h","lunch","2026-12-04 12:00",30),
("Lunch tomorrow 12h","Lunch","2026-11-10 12:00",30),
("Lunch 12:30","Lunch","2026-12-04 12:30",30),
("Lunch 12pm","Lunch","2026-12-04 12:00",30),
("Lunch at 12h for 1h","Lunch","2026-12-04 12:00",60),
("Lunch 12","Lunch","2026-12-04 12:00",30),
("Lunch 25h",nil,nil,nil),
("Coffee at 9","Coffee","2026-12-04 09:00",30),
("Call 20 October at 12am","Call","2026-10-20 00:00",30),
("Call today at 12pm for 1.5h","Call","2026-11-09 12:00",90),
("  Pairing   in 2 weeks at 09:30 for 30 minutes ","Pairing","2026-11-23 09:30",30),
("Lunch at 25",nil,nil,nil),("Lunch at 12:99",nil,nil,nil),("Lunch at 0pm",nil,nil,nil),
("Lunch at 12 for 0h",nil,nil,nil),("Lunch at 12 for 9999999999999999999999h",nil,nil,nil),
("Lunch at 12 for 3m",nil,nil,nil),("Lunch at 12 for 2 days",nil,nil,nil),
("Lunch 32 October at 12",nil,nil,nil),("Lunch next Blursday at 12",nil,nil,nil),("Plain title",nil,nil,nil)]
for (query,title,date,minutes) in cases {
 let result=AgendaEventQuery.parse(query,selectedDay:selected,now:now,calendar:calendar)
 guard result?.title==title,result.map({formatter.string(from:$0.start)})==date,result.map({Int($0.end.timeIntervalSince($0.start)/60)})==minutes else{fatalError("Failed: \(query)")}
}
assert(AgendaEventQuery.validationHint("jan 8-12 bbq w/ Sam") != nil)
assert(AgendaEventQuery.validationHint("Lunch tomorrow 12h") == nil)
print("Passed \(cases.count) quick-event parser cases")
