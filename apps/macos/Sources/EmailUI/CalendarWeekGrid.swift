import AppKit

/// A native, inert calendar canvas. Clicking an event only opens its details.
@MainActor final class CalendarWeekGrid: NSView {
  let start: Date
  let days: Int
  let events: [[String: Any]]
  let onOpen: ([String: Any]) -> Void
  var onCreate:((Date)->Void)?
  private var draftBlock:CalendarDraftBlock?
  private var draftValue:(String,Date,Date)?
  func setDraft(_ value:(String,Date,Date)?){
    draftValue=value;draftBlock?.removeFromSuperview();draftBlock=nil
    if let value {let block=CalendarDraftBlock();block.title=value.0.isEmpty ? "New event" : value.0;addSubview(block);draftBlock=block}
    needsLayout=true
  }
  private let hourHeight: CGFloat
  private let gutter: CGFloat = 48
  private var firstHour = 6
  private var header: CGFloat = 80
  private var placements: [(NSButton, Int, CGFloat, CGFloat, Int, Int)] = []
  private var axisLabels: [(NSTextField, Int)] = []
  private var dayLabels: [(NSTextField, NSTextField, Int)] = []
  private var allDayLabel: NSTextField?
  private var eventButtons: [(CalendarEventBlock, String)] = []
  private var allDays: [(NSButton, Int, Int)] = []
  override var isFlipped: Bool { true }
  private var calendar: Calendar { Calendar.current }

  init(start: Date, days: Int, events: [[String: Any]], hourHeight: CGFloat = 58, onOpen: @escaping ([String: Any]) -> Void) {
    self.start = start; self.days = days; self.events = events; self.hourHeight=hourHeight; self.onOpen = onOpen
    super.init(frame: .zero)
    translatesAutoresizingMaskIntoConstraints = false
    prepare()
    heightAnchor.constraint(equalToConstant: header + CGFloat(24-firstHour)*hourHeight + 12).isActive = true
    widthAnchor.constraint(greaterThanOrEqualToConstant: days == 7 ? 650 : 320).isActive = true
    setAccessibilityElement(false)
  }
  required init?(coder: NSCoder) { fatalError() }
  func selectEvent(_ id: String?, scroll: Bool) {
    for (button, eventID) in eventButtons {
      button.selected = eventID == id
      button.state = button.selected ? .on : .off
      button.needsDisplay = true
      if button.selected && scroll { button.scrollToVisible(button.bounds.insetBy(dx:-8,dy:-18)) }
    }
  }
  func anchor(for id:String)->NSView?{eventButtons.first(where:{$0.1 == id})?.0}
  private func date(_ raw: Any?) -> Date {
    Date(timeIntervalSince1970: ((raw as? NSNumber)?.doubleValue ?? 0)/1000)
  }
  private func calendarDate(_ raw: Any?) -> Date {
    var utc = Calendar(identifier: .gregorian); utc.timeZone = TimeZone(secondsFromGMT: 0)!
    return calendar.date(from: utc.dateComponents([.year,.month,.day], from: date(raw))) ?? start
  }
  private func minutes(_ date: Date, from day: Date) -> CGFloat {
    if date <= day { return 0 }
    let end = calendar.date(byAdding: .day, value: 1, to: day)!
    if date >= end { return 1440 }
    let parts = calendar.dateComponents([.hour,.minute], from: date)
    return CGFloat((parts.hour ?? 0)*60+(parts.minute ?? 0))
  }
  private func button(_ event: [String:Any], stamp: String) -> NSButton {
    let title = event["title"] as? String ?? "Untitled event"
    let item = CalendarEventBlock(title: title, stamp: stamp, tint: CalendarEventTint.color(event))
    eventButtons.append((item,event["id"] as? String ?? ""))
    item.onOpen = { [weak self] in self?.onOpen(event) }
    let dayFormat = DateFormatter(); dayFormat.dateStyle = .full
    let eventDate = event["all_day"] as? Bool == true ? calendarDate(event["start_ms"]) : date(event["start_ms"])
    let fullStamp = dayFormat.string(from: eventDate) + ", " + stamp
    item.toolTip = title + "\n" + fullStamp + "\nClick to view event"
    item.setAccessibilityLabel(title + ", " + fullStamp + ", open event details")
    addSubview(item)
    return item
  }
  private func prepare() {
    let format = DateFormatter(); format.dateFormat = "HH:mm"
    var counts = [Int](repeating: 0, count: days)
    for column in 0..<days {
      let day = calendar.date(byAdding: .day, value: column, to: start)!
      let next = calendar.date(byAdding: .day, value: 1, to: day)!
      var timed: [([String:Any],CGFloat,CGFloat)] = []
      for event in events {
        let allDay = event["all_day"] as? Bool == true
        let begin = allDay ? calendarDate(event["start_ms"]) : date(event["start_ms"])
        let finish = allDay ? calendarDate(event["end_ms"]) : date(event["end_ms"])
        guard begin < next && finish > day else { continue }
        if allDay {
          let item = button(event, stamp: "All day")
          allDays.append((item,column,counts[column])); counts[column] += 1
        } else {
          let a = minutes(begin, from: day), b = minutes(finish, from: day)
          firstHour = min(firstHour, Int(a/60))
          timed.append((event,a,max(a+1,b)))
        }
      }
      timed.sort { $0.1 == $1.1 ? $0.2 < $1.2 : $0.1 < $1.1 }
      // Connected overlap groups share available width; greedy lanes never overlap.
      var index = 0
      while index < timed.count {
        var endIndex = index+1, groupEnd = timed[index].2
        while endIndex < timed.count && timed[endIndex].1 < groupEnd {
          groupEnd = max(groupEnd,timed[endIndex].2); endIndex += 1
        }
        var lanes: [CGFloat] = [], assigned: [Int] = []
        for i in index..<endIndex {
          let lane = lanes.firstIndex(where: { $0 <= timed[i].1 }) ?? lanes.count
          if lane == lanes.count { lanes.append(timed[i].2) } else { lanes[lane] = timed[i].2 }
          assigned.append(lane)
        }
        for i in index..<endIndex {
          let e = timed[i]
          let stamp = format.string(from: date(e.0["start_ms"])) + "–" + format.string(from: date(e.0["end_ms"]))
          placements.append((button(e.0,stamp: stamp),column,e.1,e.2,assigned[i-index],lanes.count))
        }
        index = endIndex
      }
    }
    header = 58 + CGFloat(max(1,counts.max() ?? 0))*24
    func caption(_ value: String, size: CGFloat, color: NSColor) -> NSTextField {
      let item = NSTextField(labelWithString: value)
      item.font = .systemFont(ofSize: size, weight: .medium)
      item.textColor = color
      addSubview(item)
      return item
    }
    for hour in firstHour..<24 { axisLabels.append((caption(String(format: "%02d:00",hour),size:10,color:EmailTheme.muted),hour)) }
    let weekday = DateFormatter(); weekday.dateFormat = "EEE"
    let number = DateFormatter(); number.dateFormat = "d"
    for column in 0..<days {
      let date = calendar.date(byAdding:.day,value:column,to:start)!
      dayLabels.append((caption(weekday.string(from:date).uppercased(),size:10,color:EmailTheme.muted),caption(number.string(from:date),size:20,color:EmailTheme.ink),column))
    }
    allDayLabel = caption("All day",size:9,color:EmailTheme.muted)
  }
  override func layout() {
    super.layout()
    let columnWidth = (bounds.width-gutter)/CGFloat(days)
    for (item,column,a,b,lane,count) in placements {
      let width = (columnWidth-8)/CGFloat(count)
      item.frame = NSRect(x:gutter+CGFloat(column)*columnWidth+4+CGFloat(lane)*width,
        y:header+(a/60-CGFloat(firstHour))*hourHeight+1,
        width:max(12,width-2),height:max(20,(b-a)/60*hourHeight-2))
    }
    for (item,column,index) in allDays {
      item.frame = NSRect(x:gutter+CGFloat(column)*columnWidth+4,y:55+CGFloat(index)*24,width:columnWidth-8,height:21)
    }
    for (label,hour) in axisLabels { label.frame = NSRect(x:0,y:header+CGFloat(hour-firstHour)*hourHeight+3,width:gutter-4,height:18) }
    for (name,number,column) in dayLabels {
      let x = gutter+CGFloat(column)*columnWidth+9
      name.frame = NSRect(x:x,y:5,width:columnWidth-12,height:16)
      number.frame = NSRect(x:x,y:22,width:columnWidth-12,height:28)
    }
    allDayLabel?.frame = NSRect(x:0,y:58,width:gutter-4,height:18)
    if let value=draftValue,let block=draftBlock {
      let offset=calendar.dateComponents([.day],from:calendar.startOfDay(for:start),to:calendar.startOfDay(for:value.1)).day ?? -1
      block.isHidden=offset<0 || offset>=days
      let a=max(CGFloat(firstHour*60),minutes(value.1,from:calendar.startOfDay(for:value.1)))
      let duration=max(15,value.2.timeIntervalSince(value.1)/60)
      block.frame=NSRect(x:gutter+CGFloat(offset)*columnWidth+3,y:header+(a/60-CGFloat(firstHour))*hourHeight,width:columnWidth-6,height:max(24,CGFloat(duration)/60*hourHeight-2))
    }
    needsDisplay = true
  }
  override func draw(_ dirtyRect: NSRect) {
    EmailTheme.reader.setFill(); bounds.fill()
    let columnWidth = (bounds.width-gutter)/CGFloat(days)
    let weekday = DateFormatter(); weekday.dateFormat = "EEE"
    let number = DateFormatter(); number.dateFormat = "d"
    for column in 0..<days {
      let date = calendar.date(byAdding:.day,value:column,to:start)!
      let x = gutter+CGFloat(column)*columnWidth
      if calendar.isDateInToday(date) {
        EmailTheme.ink.withAlphaComponent(0.025).setFill()
        NSRect(x:x,y:0,width:columnWidth,height:bounds.height).fill()
      }
      line(NSPoint(x:x,y:0),NSPoint(x:x,y:bounds.height))
    }
    for hour in firstHour...24 {
      let y = header+CGFloat(hour-firstHour)*hourHeight
      line(NSPoint(x:gutter,y:y),NSPoint(x:bounds.width,y:y))
    }
    let now = Date()
    if let column = (0..<days).first(where: { calendar.isDate(now,inSameDayAs:calendar.date(byAdding:.day,value:$0,to:start)!) }) {
      let m = minutes(now,from:calendar.startOfDay(for:now))
      if m >= CGFloat(firstHour*60) {
        let y = header+(m/60-CGFloat(firstHour))*hourHeight
        EmailTheme.ink.withAlphaComponent(0.6).setStroke()
        let path=NSBezierPath(); path.move(to:NSPoint(x:gutter+CGFloat(column)*columnWidth,y:y)); path.line(to:NSPoint(x:gutter+CGFloat(column+1)*columnWidth,y:y)); path.lineWidth=1; path.stroke()
      }
    }
  }
  override func mouseDown(with event:NSEvent){
    let point=convert(event.locationInWindow,from:nil)
    guard point.y>=header,point.x>=gutter,point.x<bounds.width else{return}
    let column=min(days-1,Int((point.x-gutter)/((bounds.width-gutter)/CGFloat(days))))
    let minute=max(0,min(1425,Int(((point.y-header)/hourHeight*60+CGFloat(firstHour*60))/15)*15))
    if let base=calendar.date(byAdding:.day,value:column,to:start),let date=calendar.date(byAdding:.minute,value:minute,to:base){onCreate?(date)}
  }
  private func line(_ a: NSPoint,_ b: NSPoint) {
    EmailTheme.line.withAlphaComponent(0.45).setStroke()
    let p=NSBezierPath();p.move(to:a);p.line(to:b);p.lineWidth=0.5;p.stroke()
  }
  private func drawText(_ text:String,rect:NSRect,size:CGFloat,color:NSColor) {
    (text as NSString).draw(in:rect,withAttributes:[.font:NSFont.systemFont(ofSize:size,weight:.medium),.foregroundColor:color])
  }
}
@MainActor private final class CalendarEventBlock: NSButton {
  var onOpen: (() -> Void)?
  var selected = false
  private let caption: String
  private let stamp: String
  private let tint: NSColor
  init(title:String,stamp:String,tint:NSColor) {
    self.caption=title;self.stamp=stamp;self.tint=tint
    super.init(frame:.zero)
    self.title=title
    isBordered=false
    target=self;action=#selector(open)
    setAccessibilityRole(.button)
  }
  required init?(coder:NSCoder){fatalError()}
  @objc private func open(){onOpen?()}
  override var isFlipped: Bool {true}
  override func draw(_ dirtyRect:NSRect) {
    tint.withAlphaComponent(selected || isHighlighted ? 0.34 : 0.22).setFill()
    NSBezierPath(roundedRect:bounds.insetBy(dx:0.5,dy:0.5),xRadius:4,yRadius:4).fill()
    if selected {
      tint.withAlphaComponent(0.9).setStroke()
      let border = NSBezierPath(roundedRect:bounds.insetBy(dx:1,dy:1),xRadius:4,yRadius:4)
      border.lineWidth = 1.5; border.stroke()
    }
    tint.withAlphaComponent(0.85).setFill()
    NSRect(x:0,y:3,width:2,height:max(0,bounds.height-6)).fill()
    let p=NSMutableParagraphStyle();p.lineBreakMode = .byTruncatingTail
    let text=caption + (bounds.height >= 46 ? "\n"+stamp : "")
    (text as NSString).draw(in:bounds.insetBy(dx:6,dy:4),withAttributes:[.font:NSFont.systemFont(ofSize:11,weight:.medium),.foregroundColor:EmailTheme.ink,.paragraphStyle:p])
  }
}

@MainActor private final class CalendarDraftBlock:NSView {
  var title="New event"
  override var isFlipped:Bool{true}
  override func hitTest(_ point:NSPoint)->NSView?{nil}
  override func draw(_ dirtyRect:NSRect){
    let box=NSBezierPath(roundedRect:bounds.insetBy(dx:1,dy:1),xRadius:5,yRadius:5)
    EmailTheme.ink.withAlphaComponent(0.13).setFill();box.fill();EmailTheme.ink.withAlphaComponent(0.65).setStroke();box.lineWidth=1;box.setLineDash([4,3],count:2,phase:0);box.stroke()
    let p=NSMutableParagraphStyle();p.lineBreakMode = .byTruncatingTail
    ((title+" · Draft") as NSString).draw(in:bounds.insetBy(dx:7,dy:5),withAttributes:[.font:NSFont.systemFont(ofSize:11,weight:.medium),.foregroundColor:EmailTheme.ink,.paragraphStyle:p])
  }
}
