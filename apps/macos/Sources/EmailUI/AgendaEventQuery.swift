import Foundation

/// Explicit, local quick-event syntax. Parsing never creates or changes an event.
enum AgendaEventQuery {
  struct Result {
    let title: String
    let start: Date
    let end: Date
  }
  static func validationHint(_ query:String)->String? {
    let rangePattern = #"^\s*(?:jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|may|jun(?:e)?|jul(?:y)?|aug(?:ust)?|sep(?:tember)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?)\s+\d{1,2}\s*[-–]\s*\d{1,2}\b"#
    if query.range(of:rangePattern,options:[.regularExpression,.caseInsensitive]) != nil {
      return "Date range or time range? Use a date and an explicit time window."
    }
    return nil
  }
  static func parse(_ query: String, selectedDay: Date, now: Date = Date(), calendar: Calendar = .current) -> Result? {
    let raw=query.split(whereSeparator: { $0.isWhitespace }).joined(separator:" ")
    let pattern = #"^(.+?) (?:at )?(\d{1,2})(?::(\d{2}))?\s*(am|pm|h)?(?: for (\d+(?:\.\d+)?)\s*(m|min|mins|minute|minutes|h|hr|hrs|hour|hours))?$"#
    guard let regex=try? NSRegularExpression(pattern:pattern,options:.caseInsensitive),
      let match=regex.firstMatch(in:raw,range:NSRange(raw.startIndex...,in:raw)) else{return nil}
    func part(_ i:Int)->String{Range(match.range(at:i),in:raw).map{String(raw[$0])} ?? ""}
    guard var hour=Int(part(2)),let minute=Int(part(3).isEmpty ? "0" : part(3)),(0...59).contains(minute) else{return nil}
    let meridiem=part(4).lowercased()
    if meridiem.isEmpty || meridiem == "h" {guard (0...23).contains(hour) else{return nil}}
    else {guard (1...12).contains(hour) else{return nil};hour=hour%12+(meridiem == "pm" ? 12 : 0)}
    let amount=part(5).isEmpty ? 30 : (Double(part(5)) ?? .nan)
    let duration=amount*(part(6).lowercased().hasPrefix("h") ? 60 : 1)
    guard duration.isFinite,(5...1440).contains(duration) else{return nil}
    let words=part(1).split(separator:" ").map(String.init)
    var title=words.joined(separator:" "),date=calendar.startOfDay(for:selectedDay)
    var resolvedDate=false
    if words.count>1 {
      for index in 1..<words.count {
        let suffix=words[index...].joined(separator:" ")
        if let parsed=AgendaDateQuery.parse(suffix,now:now,calendar:calendar){
          title=words[..<index].joined(separator:" ");date=parsed;resolvedDate=true;break
        }
      }
    }
    if !resolvedDate,words.dropFirst().contains(where: { word in
      word.range(of:#"^\d+$"#,options:.regularExpression) != nil || ["today","tomorrow","yesterday","next","last"].contains(word.lowercased())
    }) {return nil}
    guard !title.isEmpty,let start=calendar.date(bySettingHour:hour,minute:minute,second:0,of:date),
      calendar.component(.hour,from:start)==hour,calendar.component(.minute,from:start)==minute else{return nil}
    return Result(title:title,start:start,end:start.addingTimeInterval(duration*60))
  }
}
