import AppKit

/// Stable calendar colors, independent of event order and process hash seeds.
@MainActor enum CalendarEventTint {
  static let palette: [NSColor] = [
    NSColor(srgbRed:125/255,green:168/255,blue:221/255,alpha:1),
    NSColor(srgbRed:169/255,green:138/255,blue:213/255,alpha:1),
    NSColor(srgbRed:208/255,green:165/255,blue:101/255,alpha:1),
    NSColor(srgbRed:117/255,green:181/255,blue:161/255,alpha:1),
    NSColor(srgbRed:207/255,green:142/255,blue:159/255,alpha:1),
    NSColor(srgbRed:146/255,green:158/255,blue:198/255,alpha:1)
  ]
  static func color(_ event: [String:Any]) -> NSColor {
    color(id: event["calendar_id"] as? String ?? event["account_id"] as? String ?? "primary",
          raw: event["calendar_color"] as? String ?? event["color"] as? String)
  }
  static func color(id: String, raw: String? = nil) -> NSColor {
    if let raw {
      let hex = raw.trimmingCharacters(in: CharacterSet(charactersIn:"#"))
      if hex.count == 6, let value = UInt32(hex,radix:16) {
        return NSColor(srgbRed:CGFloat((value >> 16)&255)/255,green:CGFloat((value >> 8)&255)/255,blue:CGFloat(value&255)/255,alpha:1)
      }
    }
    let hash = id.utf8.reduce(UInt32(2166136261)) { ($0 ^ UInt32($1)) &* 16777619 }
    return palette[Int(hash % UInt32(palette.count))]
  }
}

/// Normalize all-day calendar dates once for the entire overview, not once
/// per date cell. Each month's canvas then considers only its visible range.
private struct CalendarOverviewEvent {
  let data: [String:Any]
  let start: Date
  let end: Date
  let allDay: Bool
  init(_ data: [String:Any]) {
    self.data=data
    let allDay=data["all_day"] as? Bool == true
    self.allDay=allDay
    func normalized(_ raw:Any?)->Date {
      let date=Date(timeIntervalSince1970:((raw as? NSNumber)?.doubleValue ?? 0)/1000)
      guard allDay else{return date}
      var utc=Calendar(identifier:.gregorian);utc.timeZone=TimeZone(secondsFromGMT:0)!
      return Calendar.current.date(from:utc.dateComponents([.year,.month,.day],from:date)) ?? date
    }
    self.start=normalized(data["start_ms"])
    self.end=normalized(data["end_ms"])
  }
}

/// Month and year share the same date geometry and direct drill-down behavior.
/// All controls are inert until clicked; opening a date or event never writes.
@MainActor final class CalendarOverviewGrid: NSView {
  private let start: Date
  private let year: Bool
  private let events: [[String:Any]]
  private let onEvent: ([String:Any])->Void
  private let onDate: (Date)->Void
  private var monthViews: [CalendarMonthCanvas] = []
  private var sizeConstraint: NSLayoutConstraint!
  nonisolated(unsafe) private var viewportObserver: NSObjectProtocol?
  override var isFlipped: Bool { true }
  init(start: Date, year: Bool, events: [[String:Any]],
       onEvent: @escaping ([String:Any])->Void, onDate: @escaping (Date)->Void,
       onMonth: ((Date)->Void)? = nil) {
    self.start=start;self.year=year;self.events=events;self.onEvent=onEvent;self.onDate=onDate
    super.init(frame:.zero)
    translatesAutoresizingMaskIntoConstraints=false
    let calendar=Calendar.current
    let base=calendar.dateInterval(of:year ? .year : .month,for:start)!.start
    let normalized=events.map(CalendarOverviewEvent.init).sorted { $0.start < $1.start }
    for index in 0..<(year ? 12 : 1) {
      let date=calendar.date(byAdding:.month,value:index,to:base)!
      let canvas=CalendarMonthCanvas(month:date,compact:year,events:normalized,onEvent:onEvent,onDate:onDate,onMonth:onMonth ?? onDate)
      addSubview(canvas);monthViews.append(canvas)
    }
    sizeConstraint=heightAnchor.constraint(equalToConstant:year ? 1100 : 760)
    sizeConstraint.isActive=true
    widthAnchor.constraint(greaterThanOrEqualToConstant:year ? 330 : 640).isActive=true
    setAccessibilityElement(false)
  }
  required init?(coder:NSCoder){fatalError()}
  deinit { if let viewportObserver { NotificationCenter.default.removeObserver(viewportObserver) } }
  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    if let viewportObserver { NotificationCenter.default.removeObserver(viewportObserver); self.viewportObserver=nil }
    guard !year, let clip=enclosingScrollView?.contentView else { return }
    clip.postsBoundsChangedNotifications=true
    viewportObserver=NotificationCenter.default.addObserver(forName:NSView.boundsDidChangeNotification,object:clip,queue:.main) { [weak self] _ in
      Task { @MainActor [weak self] in self?.fitMonthToViewport() }
    }
    fitMonthToViewport()
  }
  private func fitMonthToViewport() {
    guard !year else { return }
    let height=max(420,enclosingScrollView?.contentView.bounds.height ?? 600)
    if abs(sizeConstraint.constant-height)>0.5 { sizeConstraint.constant=height; needsLayout=true }
  }
  override func layout() {
    super.layout()
    if year {
      let columns=max(1,min(4,Int((bounds.width+24)/280)))
      let gap:CGFloat=24, width=(bounds.width-CGFloat(columns-1)*gap)/CGFloat(columns)
      let height:CGFloat=272
      for (index,view) in monthViews.enumerated() {
        view.frame=NSRect(x:CGFloat(index%columns)*(width+gap),y:CGFloat(index/columns)*(height+gap),width:width,height:height)
      }
      let count=(12+columns-1)/columns
      sizeConstraint.constant=CGFloat(count)*(height+gap)-gap
    } else {
      fitMonthToViewport()
      monthViews.first?.frame=NSRect(x:0,y:0,width:bounds.width,height:sizeConstraint.constant)
    }
  }
  func selectEvent(_ id:String?,scroll:Bool) {
    for month in monthViews { month.selectEvent(id,scroll:scroll) }
  }
}

@MainActor private final class CalendarMonthCanvas:NSView {
  private let month:Date
  private let compact:Bool
  private let events:[CalendarOverviewEvent]
  private let onEvent:([String:Any])->Void
  private let onDate:(Date)->Void
  private let onMonth:(Date)->Void
  private var weekdays:[NSTextField]=[]
  private var dates:[CalendarDateCell]=[]
  private var eventButtons:[(OverviewEventButton,String)]=[]
  private var eventDates:[(CalendarDateCell,Set<String>)]=[]
  private var monthTitle:OverviewActionButton?
  private var rows=6
  override var isFlipped:Bool {true}
  init(month:Date,compact:Bool,events:[CalendarOverviewEvent],onEvent:@escaping ([String:Any])->Void,onDate:@escaping (Date)->Void,onMonth:@escaping (Date)->Void) {
    self.month=month;self.compact=compact;self.events=events;self.onEvent=onEvent;self.onDate=onDate;self.onMonth=onMonth
    super.init(frame:.zero)
    build()
  }
  required init?(coder:NSCoder){fatalError()}
  private func build() {
    let calendar=Calendar.current
    let formatter=DateFormatter();formatter.dateFormat="MMMM"
    if compact {
      let title=OverviewActionButton(title:formatter.string(from:month),run:{[weak self] in guard let self else{return};self.onMonth(self.month)})
      title.font = .systemFont(ofSize:14,weight:.semibold)
      title.contentTintColor=EmailTheme.ink
      title.setAccessibilityLabel("Open \(formatter.string(from:month))")
      addSubview(title);monthTitle=title
    }
    for label in (compact ? ["M","T","W","T","F","S","S"] : ["Mon","Tue","Wed","Thu","Fri","Sat","Sun"]) {
      let view=NSTextField(labelWithString:label);view.alignment = .center
      view.font = .systemFont(ofSize:compact ? 10 : 11,weight:.medium);view.textColor=EmailTheme.muted
      addSubview(view);weekdays.append(view)
    }
    let offset=(calendar.component(.weekday,from:month)+5)%7
    let begin=calendar.date(byAdding:.day,value:-offset,to:month)!
    let end=calendar.date(byAdding:.day,value:42,to:begin)!
    let visible=events.filter { $0.start < end && $0.end > begin }
    let fullDate=DateFormatter();fullDate.dateStyle = .full
    let time=DateFormatter();time.dateFormat="HH:mm"
    for index in 0..<42 {
      let date=calendar.date(byAdding:.day,value:index,to:begin)!,next=calendar.date(byAdding:.day,value:1,to:date)!
      let inMonth=calendar.isDate(date,equalTo:month,toGranularity:.month)
      let matches=visible.filter { $0.start < next && $0.end > date }
      let cell=CalendarDateCell(date:date,inMonth:inMonth,compact:compact,colors:matches.prefix(3).map{CalendarEventTint.color($0.data)},run:{[weak self] in self?.onDate(date)})
      cell.setAccessibilityLabel("\(fullDate.string(from:date)), \(matches.count) events. Open day")
      cell.totalEvents=matches.count
      addSubview(cell);dates.append(cell)
      eventDates.append((cell,Set(matches.compactMap{$0.data["id"] as? String})))
      if !compact {
        for (position,record) in matches.prefix(3).enumerated() {
          let event=record.data
          let title=(record.allDay ? "" : time.string(from:record.start)+" ")+(event["title"] as? String ?? "Untitled event")
          let button=OverviewEventButton(title:title,tint:CalendarEventTint.color(event),run:{[weak self] in self?.onEvent(event)})
          button.toolTip=(event["title"] as? String ?? "Untitled event")+"\n"+fullDate.string(from:date)
          button.setAccessibilityLabel(button.toolTip!+", open event details")
          button.position=position;cell.addSubview(button)
          eventButtons.append((button,event["id"] as? String ?? ""))
        }
        if !matches.isEmpty {
          let more=OverviewActionButton(title:"+\(matches.count) more",run:{[weak self] in self?.onDate(date)})
          more.font = .systemFont(ofSize:10,weight:.medium);more.contentTintColor=EmailTheme.muted
          cell.more=more;cell.addSubview(more)
        }
      }
    }
  }
  override func layout() {
    super.layout()
    let titleHeight:CGFloat=compact ? 30 : 0
    let weekdayHeight:CGFloat=24
    let width=bounds.width/7,height=(bounds.height-titleHeight-weekdayHeight)/CGFloat(rows)
    monthTitle?.frame=NSRect(x:0,y:0,width:bounds.width,height:24)
    for (index,label) in weekdays.enumerated(){label.frame=NSRect(x:CGFloat(index)*width,y:titleHeight,width:width,height:20)}
    for (index,cell) in dates.enumerated(){cell.frame=NSRect(x:CGFloat(index%7)*width,y:titleHeight+weekdayHeight+CGFloat(index/7)*height,width:width,height:height)}
  }
  func selectEvent(_ id:String?,scroll:Bool) {
    for (cell, ids) in eventDates {
      cell.eventSelected = id.map { ids.contains($0) } ?? false
      cell.needsDisplay = true
      if cell.eventSelected && scroll { cell.scrollToVisible(cell.bounds) }
    }
    for (button,eventID) in eventButtons {
      button.selected=eventID==id;button.needsDisplay=true
      if button.selected && scroll && !button.isHidden {button.scrollToVisible(button.bounds.insetBy(dx:-4,dy:-12))}
    }
  }
}

@MainActor private class OverviewActionButton:NSButton {
  private var run:()->Void
  init(title:String,run:@escaping ()->Void){self.run=run;super.init(frame:.zero);self.title=title;isBordered=false;target=self;action=#selector(activate)}
  required init?(coder:NSCoder){fatalError()}
  @objc private func activate(){run()}
}

@MainActor private final class CalendarDateLabel:NSTextField {
  // Date text is visual content of the parent date button, not a separate target.
  override func hitTest(_ point:NSPoint)->NSView? { nil }
}

@MainActor private final class CalendarDateCell:OverviewActionButton {
  private let numberLabel=CalendarDateLabel(labelWithString:"")
  let date:Date
  let inMonth:Bool
  let compact:Bool
  let colors:[NSColor]
  var more:NSButton?
  var eventSelected=false
  var totalEvents=0
  init(date:Date,inMonth:Bool,compact:Bool,colors:[NSColor],run:@escaping ()->Void){
    self.date=date;self.inMonth=inMonth;self.compact=compact;self.colors=colors
    super.init(title:String(Calendar.current.component(.day,from:date)),run:run)
    numberLabel.stringValue=title
    numberLabel.font = .systemFont(ofSize:compact ? 11 : 12,weight:Calendar.current.isDateInToday(date) ? .bold : .medium)
    numberLabel.textColor=inMonth ? EmailTheme.ink : EmailTheme.muted.withAlphaComponent(0.45)
    numberLabel.alignment = .center
    numberLabel.isSelectable=false
    numberLabel.setAccessibilityElement(false)
    addSubview(numberLabel)
  }
  required init?(coder:NSCoder){fatalError()}
  override var isFlipped:Bool {true}
  override func layout(){
    super.layout()
    numberLabel.frame=compact ? NSRect(x:(bounds.width-24)/2,y:3,width:24,height:22) : NSRect(x:7,y:5,width:25,height:22)
    guard !compact else { return }
    let dense=bounds.height<100
    let top:CGFloat=dense ? 25 : 30
    let chipHeight:CGFloat=dense ? 18 : 21
    let step:CGFloat=dense ? 20 : 23
    let moreHeight:CGFloat=dense ? 14 : 18
    let available=max(0,bounds.height-top-3)
    let capacity=max(0,min(3,Int((available+step-chipHeight)/step)))
    let shown=totalEvents>capacity
      ? max(0,min(3,Int((max(0,available-moreHeight-2)+step-chipHeight)/step)))
      : capacity
    for case let button as OverviewEventButton in subviews {
      button.isHidden=button.position>=shown
      button.frame=NSRect(x:5,y:top+CGFloat(button.position)*step,width:bounds.width-10,height:chipHeight)
    }
    let remaining=max(0,totalEvents-shown)
    more?.isHidden=remaining==0
    more?.title="+\(remaining) more"
    more?.frame=NSRect(x:5,y:top+CGFloat(shown)*step,width:bounds.width-10,height:moreHeight)
  }
  override func draw(_ dirtyRect:NSRect){
    if !compact {
      if !inMonth {EmailTheme.ink.withAlphaComponent(0.025).setFill();bounds.fill()}
      EmailTheme.line.withAlphaComponent(0.7).setStroke()
      let line=NSBezierPath();line.move(to:NSPoint(x:0,y:0));line.line(to:NSPoint(x:bounds.width,y:0));line.move(to:NSPoint(x:0,y:0));line.line(to:NSPoint(x:0,y:bounds.height));line.lineWidth=0.5;line.stroke()
    }
    if eventSelected {
      EmailTheme.ink.withAlphaComponent(0.45).setStroke()
      let path=NSBezierPath(roundedRect:bounds.insetBy(dx:1,dy:1),xRadius:compact ? 4 : 0,yRadius:compact ? 4 : 0)
      path.lineWidth=1;path.stroke()
    }
    let rect=compact ? NSRect(x:(bounds.width-24)/2,y:3,width:24,height:22) : NSRect(x:7,y:5,width:25,height:22)
    let today=Calendar.current.isDateInToday(date)
    if today {EmailTheme.ink.withAlphaComponent(0.12).setFill();NSBezierPath(ovalIn:rect.insetBy(dx:1,dy:0)).fill()}
    if compact && inMonth {
      for (index,color) in colors.enumerated(){color.setFill();NSBezierPath(ovalIn:NSRect(x:bounds.midX+CGFloat(index)*4-CGFloat(colors.count)*2,y:27,width:3,height:3)).fill()}
    }
  }
}

@MainActor private final class OverviewEventButton:OverviewActionButton {
  let tint:NSColor
  var position=0
  var selected=false
  init(title:String,tint:NSColor,run:@escaping ()->Void){self.tint=tint;super.init(title:title,run:run)}
  required init?(coder:NSCoder){fatalError()}
  override var isFlipped:Bool {true}
  override func draw(_ dirtyRect:NSRect){
    tint.withAlphaComponent(selected ? 0.34 : 0.14).setFill();NSBezierPath(roundedRect:bounds.insetBy(dx:0.5,dy:0.5),xRadius:3,yRadius:3).fill()
    tint.withAlphaComponent(0.8).setFill();NSRect(x:1,y:3,width:2,height:bounds.height-6).fill()
    if selected {tint.setStroke();let path=NSBezierPath(roundedRect:bounds.insetBy(dx:1,dy:1),xRadius:3,yRadius:3);path.lineWidth=1.5;path.stroke()}
    let style=NSMutableParagraphStyle();style.lineBreakMode = .byTruncatingTail
    (title as NSString).draw(in:bounds.insetBy(dx:6,dy:3),withAttributes:[.font:NSFont.systemFont(ofSize:10,weight:.medium),.foregroundColor:EmailTheme.ink,.paragraphStyle:style])
  }
}
