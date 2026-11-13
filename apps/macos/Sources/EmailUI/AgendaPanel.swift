import AppKit

/// Connected calendar requests use a separate service lane from cached mail.
@MainActor final class AgendaPanel: NSWindowController, NSSearchFieldDelegate, NSPopoverDelegate {
  private static var active: AgendaPanel?
  static func present(core: CoreBridge, parent: NSWindow?) {
    if let mail = parent as? MailWindow { mail.showCalendarWorkspace(); return }
    active?.close()
    let panel = AgendaPanel(core: core, parent: parent)
    active = panel
    panel.showWindow(nil)
  }
  static func embedded(core: CoreBridge, host: NSWindow, onClose: @escaping () -> Void) -> AgendaPanel {
    let panel = AgendaPanel(core: core, parent: nil, embeddedHost: host, onClose: onClose)
    panel.workspaceView = panel.window!.contentView!
    panel.window?.contentView = nil
    return panel
  }
  private(set) var workspaceView: NSView?
  private weak var embeddedHost: NSWindow?
  private var onClose: (() -> Void)?
  private var interactionWindow: NSWindow? { embeddedHost ?? window }
  private let search = NSSearchField()
  private var selectedEvents: [[String:Any]] = []
  private var selectedEventIndex = 0
  private weak var currentGrid: CalendarWeekGrid?
  private var currentEventDraft:(String,Date,Date)?
  private weak var currentOverview: CalendarOverviewGrid?
  private let legend = NSStackView()
  private var legendHeight:NSLayoutConstraint?
  private var hiddenCalendars = Set<String>()
  private var datePopover: NSPopover?
  private var dateMonitor:Any?
  private var editorPopover:NSPopover?
  private var editorMonitor:Any?
  private var detailMonitor:Any?
  private var dateSubmit:(()->Void)?
  private var dateKeyHandler:((NSEvent)->Bool)?
  private var dateBrowsing=false
  private weak var dateQueryField:NSTextField?
  private var updateDatePreview:(()->Void)?
  private var editorSubmit:(()->Void)?
  private weak var eventTitleField:NSTextField?
  private var updateEventSuggestion:(()->Void)?
  private var applyEventSuggestion:(()->Bool)?
  private weak var eventCreateAnchor:NSButton?
  private var detailEdit:(()->Void)?
  var hasActiveOverlay:Bool{[datePopover,editorPopover,detailPopover,searchPopover,navigationPopover].contains{$0?.isShown == true}}
  private weak var previousButton: NSButton?
  private weak var nextButton: NSButton?
  private var modeMenu: NSPopUpButton?
  private var compactHeader = true
  private let miniMonth = AgendaMiniMonth()
  private var hourDensity = 1
  private var pendingScrollScale:CGFloat=1
  private var detailPopover: NSPopover?
  private var navigationPopover:NSPopover?
  private var navigationRail:NSStackView?
  private weak var navigationHost:NSStackView?
  private var railHeight:NSLayoutConstraint?
  private var railCompact=false
  private weak var navigationButton:NSButton?
  private var compactSearch: NSSearchField?
  private var searchPopover: NSPopover?
  private var searchMonitor: Any?
  private weak var searchShortcut: NSButton?
  private var readingEvent = false
  private let shortcutStrip = ShortcutStrip()
  private let core: CoreBridge
  private let accounts = NSPopUpButton()
  private let dateLabel = NSTextField(labelWithString: "Today")
  private let status = NSTextField(wrappingLabelWithString: "Loading saved calendars…")
  private let rows = NSStackView()
  private let connect = NSButton(title: "Connect Google Calendar…", target: nil, action: nil)
  private let refresh = NSButton(title: "Refresh", target: nil, action: nil)
  private var records: [[String: Any]] = [], collections: [[String: Any]] = []
  private var day = Calendar.current.startOfDay(for: Date())
  private var generation = 0, busy = false
  private var connectedAccountIDs:[String]=[]
  private var refreshAttempts: [String:Date] = [:]
  private var freshRanges: [(String, Int64, Int64, Date)] = []
  private var monitor: Any?
  private enum CalendarMode: Int { case day, week, month, year
    var component: Calendar.Component { switch self { case .day,.week: return .day; case .month: return .month; case .year: return .year } }
    var title: String { ["day","week","month","year"][rawValue] }
  }
  private var mode: CalendarMode = .week
  private var inSubpage = false
  private var timelineScroll: NSScrollView?
  private var modeControl: NSSegmentedControl?
  private var formKeyViews: [NSView] = []
  private var accountID: String? {
    records.indices.contains(accounts.indexOfSelectedItem-1)
      ? records[accounts.indexOfSelectedItem-1]["id"] as? String : nil
  }
  private var rangeStart: Date {
    let calendar = Calendar.current
    switch mode {
    case .day: return calendar.startOfDay(for: day)
    case .week:
      let weekday = calendar.component(.weekday, from: day)
      return calendar.date(byAdding: .day, value: -((weekday+5)%7), to: calendar.startOfDay(for: day))!
    case .month: return calendar.dateInterval(of: .month, for: day)!.start
    case .year: return calendar.dateInterval(of: .year, for: day)!.start
    }
  }
  private var rangeEnd: Date {
    Calendar.current.date(byAdding: mode.component, value: mode == .week ? 7 : 1, to: rangeStart)!
  }
  private var range: (Int64, Int64) {
    (Int64(rangeStart.timeIntervalSince1970 * 1000), Int64(rangeEnd.timeIntervalSince1970 * 1000))
  }
  init(core: CoreBridge, parent: NSWindow?, embeddedHost: NSWindow? = nil, onClose: (() -> Void)? = nil) {
    self.embeddedHost = embeddedHost
    self.onClose = onClose
    self.core = core
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 1000, height: 700),
      styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
    window.title = "Calendar"
    window.titlebarAppearsTransparent = true
    window.titleVisibility = .hidden
    window.backgroundColor = EmailTheme.reader
    window.minSize = NSSize(width: 480, height: 420)
    window.isReleasedWhenClosed = false
    super.init(window: window)
    build()
    if embeddedHost == nil {
      if let parent { parent.addChildWindow(window, ordered: .above) }
      window.center()
      window.makeKeyAndOrderFront(nil)
      monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
        guard let self, event.window === self.interactionWindow else { return event }
        return self.handleKeyboard(event) ? nil : event
      }
    }
    Task { await loadAccounts() }
  }
  required init?(coder: NSCoder) { fatalError() }
  override func close() {
    searchPopover?.close()
    detailPopover?.close()
    navigationPopover?.close()
    datePopover?.close();editorPopover?.close()
    if let dateMonitor {NSEvent.removeMonitor(dateMonitor);self.dateMonitor=nil}
    if let editorMonitor {NSEvent.removeMonitor(editorMonitor);self.editorMonitor=nil}
    if let detailMonitor {NSEvent.removeMonitor(detailMonitor);self.detailMonitor=nil}
    if let searchMonitor {NSEvent.removeMonitor(searchMonitor);self.searchMonitor=nil}
    if let onClose { onClose(); return }
    if let monitor {
      NSEvent.removeMonitor(monitor)
      self.monitor = nil
    }
    super.close()
    if Self.active === self { Self.active = nil }
  }
  private func label(_ text: String, strong: Bool = false) -> NSTextField {
    let value = NSTextField(wrappingLabelWithString: text)
    value.font = .systemFont(ofSize: 12, weight: strong ? .semibold : .regular)
    value.textColor = strong ? EmailTheme.ink : EmailTheme.muted
    return value
  }
  private func action(_ title: String, _ run: @escaping () -> Void) -> NSButton {
    let button = AgendaButton(title: title, target: nil, action: nil)
    button.run = run
    button.bezelStyle = .inline
    button.isBordered = false
    button.heightAnchor.constraint(equalToConstant:30).isActive=true
    button.controlSize = .regular
    button.font = .systemFont(ofSize: 12, weight: .medium)
    button.contentTintColor = EmailTheme.ink
    button.target = button
    button.action = #selector(AgendaButton.invoke)
    return button
  }
  private func build() {
    guard let window else { return }
    let root = AgendaSurface()
    let outer = NSStackView(); outer.orientation = .horizontal; outer.alignment = .top; outer.spacing = 0
    outer.translatesAutoresizingMaskIntoConstraints = false; root.addSubview(outer)
    NSLayoutConstraint.activate([outer.topAnchor.constraint(equalTo:root.topAnchor),outer.leadingAnchor.constraint(equalTo:root.leadingAnchor),outer.trailingAnchor.constraint(equalTo:root.trailingAnchor),outer.bottomAnchor.constraint(equalTo:root.bottomAnchor)])
    let rail = NSStackView(); rail.orientation = .vertical; rail.alignment = .leading; rail.spacing = 14
    rail.edgeInsets = NSEdgeInsets(top:22,left:16,bottom:16,right:16)
    rail.widthAnchor.constraint(equalToConstant:184).isActive = true
    outer.addArrangedSubview(rail);railHeight=rail.heightAnchor.constraint(equalTo:outer.heightAnchor);railHeight?.isActive=true
    navigationRail=rail;navigationHost=outer
    let railTitle = label("Calendar",strong:true);railTitle.font = .systemFont(ofSize:15,weight:.semibold);rail.addArrangedSubview(railTitle)
    miniMonth.onDate = { [weak self] date in guard let self else{return};self.day=date;self.reloadTask() }
    rail.addArrangedSubview(miniMonth)
    miniMonth.widthAnchor.constraint(equalToConstant:152).isActive=true;miniMonth.heightAnchor.constraint(equalToConstant:172).isActive=true
    let sourceTitle=label("MY CALENDARS");sourceTitle.font = .systemFont(ofSize:9,weight:.semibold);rail.addArrangedSubview(sourceTitle)
    accounts.target=self;accounts.action=#selector(accountChanged);accounts.controlSize = .small;accounts.font = .systemFont(ofSize:10)
    accounts.setAccessibilityLabel("Calendar account");rail.addArrangedSubview(accounts);accounts.widthAnchor.constraint(equalToConstant:152).isActive=true
    legend.orientation = .vertical;legend.alignment = .leading;legend.spacing=3
    let legendDocument=AgendaDocument();legendDocument.translatesAutoresizingMaskIntoConstraints=false
    legend.translatesAutoresizingMaskIntoConstraints=false;legendDocument.addSubview(legend)
    legendHeight=legend.heightAnchor.constraint(equalToConstant:0);legendHeight?.isActive=true
    NSLayoutConstraint.activate([legend.leadingAnchor.constraint(equalTo:legendDocument.leadingAnchor),legend.trailingAnchor.constraint(equalTo:legendDocument.trailingAnchor),legend.topAnchor.constraint(equalTo:legendDocument.topAnchor),legend.bottomAnchor.constraint(equalTo:legendDocument.bottomAnchor),legendDocument.widthAnchor.constraint(equalToConstant:152)])
    let legendScroll=NSScrollView();legendScroll.drawsBackground=false;legendScroll.hasVerticalScroller=true;legendScroll.autohidesScrollers=false;legendScroll.scrollerStyle = .overlay;legendScroll.documentView=legendDocument
    rail.addArrangedSubview(legendScroll);legendScroll.widthAnchor.constraint(equalToConstant:152).isActive=true
    legendScroll.heightAnchor.constraint(greaterThanOrEqualToConstant:70).isActive=true
    let railSpacer=NSView();railSpacer.setContentHuggingPriority(.defaultLow,for:.vertical);rail.addArrangedSubview(railSpacer);railSpacer.heightAnchor.constraint(equalToConstant:0).isActive=true
    let zone=label(TimeZone.current.identifier.replacingOccurrences(of:"_",with:" "));zone.font = .systemFont(ofSize:10);rail.addArrangedSubview(zone)
    if DistributionProfile.current?.hasScheduling == true { rail.addArrangedSubview(action("Bookings and links", { [weak self] in guard let self else { return }; if let mail = self.interactionWindow as? MailWindow { mail.showBookingPanel() } else { BookingPanel.present(core: self.core) } })) }
    let free=action("Find free time",{[weak self] in self?.availability()});free.toolTip="Find available 30-minute slots";rail.addArrangedSubview(free)
    let divider=NSBox();divider.boxType = .separator;outer.addArrangedSubview(divider);divider.widthAnchor.constraint(equalToConstant:1).isActive=true;divider.heightAnchor.constraint(equalTo:outer.heightAnchor).isActive=true
    let stack=NSStackView();stack.orientation = .vertical;stack.alignment = .leading;stack.spacing=0
    stack.edgeInsets = NSEdgeInsets(top:18,left:18,bottom:8,right:18)
    outer.addArrangedSubview(stack);stack.heightAnchor.constraint(equalTo:outer.heightAnchor).isActive=true
    stack.setContentHuggingPriority(.defaultLow,for:.horizontal)
    let mainWidth=stack.widthAnchor.constraint(equalTo:outer.widthAnchor,constant:-185);mainWidth.isActive=true
    for item in rail.arrangedSubviews where item !== railSpacer {item.setContentHuggingPriority(.required,for:.vertical)}
    legendScroll.setContentHuggingPriority(.defaultLow,for:.vertical)
    legendScroll.setContentCompressionResistancePriority(.defaultLow,for:.vertical)
    let header=NSStackView();header.orientation = .horizontal;header.alignment = .centerY;header.spacing=6
    let back=action("",{[weak self] in self?.close()});back.image=NSImage(systemSymbolName:"arrow.left",accessibilityDescription:"Back to mail");back.toolTip="Back to mail (G then I)";back.widthAnchor.constraint(equalToConstant:28).isActive=true;header.addArrangedSubview(back)
    dateLabel.font = .systemFont(ofSize:20,weight:.semibold);dateLabel.textColor=EmailTheme.ink;dateLabel.lineBreakMode = .byTruncatingTail
    dateLabel.setContentCompressionResistancePriority(.defaultLow,for:.horizontal);header.addArrangedSubview(dateLabel)
    let jump=action("⌄",{[weak self] in self?.jumpToDate()});jump.toolTip="Go to a date (B)";header.addArrangedSubview(jump)
    let spacer=NSView();spacer.setContentHuggingPriority(.defaultLow,for:.horizontal);header.addArrangedSubview(spacer)
    let controls=NSStackView();controls.orientation = .horizontal;controls.alignment = .centerY;controls.spacing=6;header.addArrangedSubview(controls)
    let previous=action("‹",{[weak self] in self?.move(-1)});previousButton=previous;previous.widthAnchor.constraint(equalToConstant:28).isActive=true;controls.addArrangedSubview(previous)
    let today=action("Today",{[weak self] in self?.goToday()});today.toolTip="Today (T)";controls.addArrangedSubview(today)
    let next=action("›",{[weak self] in self?.move(1)});nextButton=next;next.widthAnchor.constraint(equalToConstant:28).isActive=true;controls.addArrangedSubview(next)
    let menu=NSPopUpButton();menu.addItems(withTitles:["Day","Week","Month","Year"]);menu.selectItem(at:1);menu.target=self;menu.action=#selector(agendaModeMenuChanged);menu.font = .systemFont(ofSize:12,weight:.medium);menu.bezelStyle = .texturedRounded;menu.toolTip="View: D Day · W Week · M Month · Y Year";modeMenu=menu;controls.addArrangedSubview(menu)
    search.delegate=self;search.placeholderString="Search calendar";search.isHidden=true
    let sourceButton=action("",{[weak self] in self?.showNavigator()});sourceButton.image=NSImage(systemSymbolName:"sidebar.left",accessibilityDescription:"Calendars");sourceButton.toolTip="Calendars, account and date navigation";let sourcesWidth=sourceButton.widthAnchor.constraint(equalToConstant:30);sourcesWidth.isActive=true;sourceButton.isHidden=true;navigationButton=sourceButton;controls.addArrangedSubview(sourceButton)
    let searchButton=action("",{[weak self] in self?.focusSearch()});searchButton.image=NSImage(systemSymbolName:"magnifyingglass",accessibilityDescription:"Search calendar");searchButton.toolTip="Search calendar (/)";searchButton.widthAnchor.constraint(equalToConstant:30).isActive=true;searchShortcut=searchButton;controls.addArrangedSubview(searchButton)
    let create=action("New event",{[weak self] in self?.newEvent()});eventCreateAnchor=create;(create as? AgendaButton)?.prominent=true;create.image=NSImage(systemSymbolName:"plus",accessibilityDescription:nil);create.imagePosition = .imageLeft;create.toolTip="New event (C)";controls.addArrangedSubview(create)
    refresh.target=self;refresh.action=#selector(refreshCalendar);refresh.title="";refresh.image=NSImage(systemSymbolName:"arrow.clockwise",accessibilityDescription:"Refresh calendar");refresh.bezelStyle = .inline;refresh.isBordered=false;refresh.toolTip="Refresh calendar (⌘R)";controls.addArrangedSubview(refresh)
    let help=action("?",{[weak self] in self?.showNavigationHelp()});help.toolTip="All calendar shortcuts (?)";controls.addArrangedSubview(help)
    stack.addArrangedSubview(header);header.widthAnchor.constraint(equalTo:stack.widthAnchor,constant:-36).isActive=true;header.heightAnchor.constraint(equalToConstant:38).isActive=true
    let compactRow=NSStackView();compactRow.orientation = .horizontal;compactRow.alignment = .centerY;compactRow.isHidden=true
    stack.addArrangedSubview(compactRow);compactRow.widthAnchor.constraint(equalTo:header.widthAnchor).isActive=true;compactRow.heightAnchor.constraint(equalToConstant:36).isActive=true
    let compactControlsWidth=controls.widthAnchor.constraint(equalTo:compactRow.widthAnchor)
    connect.target=self;connect.action=#selector(connectCalendar);connect.controlSize = .small;stack.addArrangedSubview(connect)
    let separator=NSBox();separator.boxType = .separator;stack.addArrangedSubview(separator);separator.widthAnchor.constraint(equalTo:header.widthAnchor).isActive=true
    rows.orientation = .vertical;rows.alignment = .leading;rows.spacing=0;rows.translatesAutoresizingMaskIntoConstraints=false
    let document=AgendaDocument();document.translatesAutoresizingMaskIntoConstraints=false;document.addSubview(rows)
    let scroll=NSScrollView();scroll.drawsBackground=false;scroll.hasVerticalScroller=true;scroll.hasHorizontalScroller=true;scroll.autohidesScrollers=true;scroll.documentView=document;timelineScroll=scroll;scroll.translatesAutoresizingMaskIntoConstraints=false
    stack.addArrangedSubview(scroll)
    NSLayoutConstraint.activate([scroll.widthAnchor.constraint(equalTo:header.widthAnchor),scroll.heightAnchor.constraint(greaterThanOrEqualToConstant:140),document.widthAnchor.constraint(greaterThanOrEqualTo:scroll.contentView.widthAnchor),rows.leadingAnchor.constraint(equalTo:document.leadingAnchor),rows.trailingAnchor.constraint(equalTo:document.trailingAnchor,constant:-2),rows.topAnchor.constraint(equalTo:document.topAnchor),rows.bottomAnchor.constraint(equalTo:document.bottomAnchor)])
    let preferred=document.widthAnchor.constraint(equalTo:scroll.contentView.widthAnchor);preferred.priority = .defaultHigh;preferred.isActive=true
    status.font = .systemFont(ofSize:10);status.textColor=EmailTheme.muted;stack.addArrangedSubview(status);status.widthAnchor.constraint(equalTo:header.widthAnchor).isActive=true
    updateShortcuts();stack.addArrangedSubview(shortcutStrip);shortcutStrip.widthAnchor.constraint(equalTo:header.widthAnchor).isActive=true;shortcutStrip.heightAnchor.constraint(equalToConstant:28).isActive=true
    root.onWidthChanged = { [weak self,weak rail,weak divider,weak create,weak today,weak refresh,weak sourceButton,weak help,weak back,weak jump,weak controls,weak header,weak compactRow] width in
      guard let self else{return};self.railCompact=width<1100;mainWidth.constant=self.railCompact ? 0 : -185
      if self.navigationPopover?.isShown != true {rail?.isHidden=self.railCompact}
      divider?.isHidden=self.railCompact;sourceButton?.isHidden = !self.railCompact
      help?.isHidden=width<800;back?.isHidden=width<700;jump?.isHidden=width<700
      self.dateLabel.font = .systemFont(ofSize:width<700 ? 16 : 20,weight:.semibold)
      if let controls,let header,let compactRow {
        let split=width<800
        if split && controls.superview !== compactRow {header.removeArrangedSubview(controls);controls.removeFromSuperview();compactRow.addArrangedSubview(controls);compactControlsWidth.isActive=true}
        else if !split && controls.superview !== header {compactControlsWidth.isActive=false;compactRow.removeArrangedSubview(controls);controls.removeFromSuperview();header.addArrangedSubview(controls)}
        compactRow.isHidden = !split
        sourceButton?.title=split ? "Calendars" : "";sourcesWidth.constant=split ? 76 : 30
      }
      create?.title="New event";today?.isHidden=width<600;refresh?.isHidden=width<800;self.compactHeader=true
    }
    window.contentView=root
  }
  func handleKeyboard(_ event: NSEvent) -> Bool {
    // The containing mail window owns the first local keyboard monitor.
    // Route active calendar overlays before any inbox/calendar global key.
    if hasActiveOverlay {
      if event.keyCode==53 {
        if editorPopover?.isShown == true{editorPopover?.close()}
        else if datePopover?.isShown == true{datePopover?.close()}
        else if detailPopover?.isShown == true{detailPopover?.close()}
        else if searchPopover?.isShown == true{searchPopover?.close()}
        else{navigationPopover?.close()}
        interactionWindow?.makeFirstResponder(nil);return true
      }
      if datePopover?.isShown == true{return dateKeyHandler?(event) ?? false}
      if editorPopover?.isShown == true,event.keyCode==36,event.modifierFlags.contains(.command){editorSubmit?();return true}
      if detailPopover?.isShown == true,event.charactersIgnoringModifiers?.lowercased()=="e",event.modifierFlags.intersection([.command,.control,.option]).isEmpty{detailEdit?();return true}
      return false
    }
    guard event.window === interactionWindow, NSApp.modalWindow == nil else { return false }
    let editor = interactionWindow?.firstResponder as? NSTextView
    let editing = editor?.isEditable == true
    let searchEditing = editor != nil && (search.currentEditor() === editor || compactSearch?.currentEditor() === editor)
    if event.keyCode == 53 {
      if detailPopover?.isShown == true {detailPopover?.close();return true}
      if searchEditing {
        search.stringValue = ""; compactSearch?.stringValue=""; searchPopover?.close()
        interactionWindow?.makeFirstResponder(nil); reloadTask()
      } else if inSubpage { reloadTask() } else { close() }
      return true
    }
    if event.keyCode == 36, searchEditing {
      searchPopover?.close();interactionWindow?.makeFirstResponder(nil);return true
    }
    guard !editing else { return false }
    let modifiers = event.modifierFlags.intersection([.command,.control,.option,.shift])
    let key = event.charactersIgnoringModifiers?.lowercased()
    if modifiers == .command && key == "r" { refreshCalendar(); return true }
    if modifiers == .control && [2,32].contains(event.keyCode) {
      scrollCalendar((event.keyCode == 2 ? 1 : -1) * (timelineScroll?.contentView.bounds.height ?? 400)/2)
      return true
    }
    // Slash requires Shift on layouts such as Norwegian. Treat it as a
    // character shortcut, without requiring an unshifted physical key.
    if event.characters == "/", modifiers.subtracting(.shift).isEmpty {
      focusSearch(); return true
    }
    if event.characters == "?", modifiers.subtracting(.shift).isEmpty { showNavigationHelp(); return true }
    if modifiers.subtracting(.shift).isEmpty, ["+","=","-"].contains(event.characters ?? "") {
      let old=hourDensity;hourDensity=max(0,min(2,hourDensity + (event.characters == "-" ? -1 : 1)))
      pendingScrollScale=CGFloat([44,58,76][hourDensity])/CGFloat([44,58,76][old]);reloadTask();return true
    }
    guard modifiers.isEmpty else { return false }
    if let index = ["d":0,"w":1,"m":2,"y":3,"1":0,"2":1,"3":2,"4":3][key ?? ""] {
      setMode(index); return true
    }
    if key == "t" { goToday(); return true }
    if key == "b" { jumpToDate(); return true }
    if key == "h" { move(-1); return true }
    if key == "l" { move(1); return true }
    if key == "c" { newEvent(); return true }
    if event.keyCode == 123 { move(-1); return true }
    if event.keyCode == 124 { move(1); return true }
    if key == "j" || event.keyCode == 125 {
      if inSubpage { scrollCalendar(80) } else { selectEvent(selectedEventIndex+1) }
      return true
    }
    if key == "k" || event.keyCode == 126 {
      if inSubpage { scrollCalendar(-80) } else { selectEvent(selectedEventIndex-1) }
      return true
    }
    if event.keyCode == 36, !inSubpage, selectedEvents.indices.contains(selectedEventIndex) {
      showEvent(selectedEvents[selectedEventIndex]); return true
    }
    return false
  }
  private func selectEvent(_ index: Int) {
    guard !selectedEvents.isEmpty else { return }
    selectedEventIndex = min(max(0,index),selectedEvents.count-1)
    currentGrid?.selectEvent(selectedEvents[selectedEventIndex]["id"] as? String, scroll: true)
    currentOverview?.selectEvent(selectedEvents[selectedEventIndex]["id"] as? String, scroll: true)
  }
  func focusSearch() {
    guard compactHeader,let anchor=searchShortcut else {interactionWindow?.makeFirstResponder(search);return}
    if let searchMonitor {NSEvent.removeMonitor(searchMonitor);self.searchMonitor=nil}
    searchPopover?.close()
    let popover=NSPopover();popover.behavior = .transient;popover.delegate=self
    let controller=NSViewController();let view=NSView(frame:NSRect(x:0,y:0,width:320,height:56))
    let field=NSSearchField(frame:NSRect(x:14,y:14,width:292,height:28))
    field.placeholderString="Search calendar";field.stringValue=search.stringValue;field.delegate=self
    field.setAccessibilityLabel("Search calendar events in this date range")
    view.addSubview(field);controller.view=view;popover.contentViewController=controller
    compactSearch=field;searchPopover=popover
    popover.show(relativeTo:anchor.bounds,of:anchor,preferredEdge:.maxY)
    field.window?.makeFirstResponder(field)
    searchMonitor=NSEvent.addLocalMonitorForEvents(matching:.keyDown) { [weak self,weak field] event in
      guard let self,let field,event.window === field.window else{return event}
      if event.keyCode==53 {
        self.search.stringValue="";field.stringValue="";self.reloadTask();self.searchPopover?.close();self.interactionWindow?.makeFirstResponder(nil);return nil
      }
      if event.keyCode==36 {self.searchPopover?.close();self.interactionWindow?.makeFirstResponder(nil);return nil}
      return event
    }
  }
  private func showNavigator(){
    guard let rail=navigationRail,let host=navigationHost,let anchor=navigationButton else{return}
    if navigationPopover?.isShown == true{navigationPopover?.close();return}
    railHeight?.isActive=false;host.removeArrangedSubview(rail);rail.removeFromSuperview();rail.isHidden=false
    let popover=NSPopover();popover.behavior = .transient;popover.delegate=self
    let controller=NSViewController();controller.view=rail;popover.contentViewController=controller
    popover.contentSize=NSSize(width:184,height:560);navigationPopover=popover
    popover.show(relativeTo:anchor.bounds,of:anchor,preferredEdge:.maxY)
  }
  func popoverDidClose(_ notification:Notification) {
    if notification.object as? NSPopover === detailPopover {if let detailMonitor {NSEvent.removeMonitor(detailMonitor);self.detailMonitor=nil};detailPopover=nil;detailEdit=nil;return}
    if notification.object as? NSPopover === datePopover {if let dateMonitor {NSEvent.removeMonitor(dateMonitor);self.dateMonitor=nil};datePopover=nil;dateSubmit=nil;dateKeyHandler=nil;dateBrowsing=false;dateQueryField=nil;updateDatePreview=nil;return}
    if notification.object as? NSPopover === editorPopover {if let editorMonitor {NSEvent.removeMonitor(editorMonitor);self.editorMonitor=nil};currentEventDraft=nil;currentGrid?.setDraft(nil);editorPopover=nil;editorSubmit=nil;eventTitleField=nil;updateEventSuggestion=nil;applyEventSuggestion=nil;formKeyViews=[];return}
    if notification.object as? NSPopover === navigationPopover,let rail=navigationRail,let host=navigationHost{
      navigationPopover?.contentViewController?.view=NSView();rail.removeFromSuperview();host.insertArrangedSubview(rail,at:0);railHeight?.isActive=true;rail.isHidden=railCompact;navigationPopover=nil;return
    }
    guard notification.object as? NSPopover === searchPopover else{return}
    if let searchMonitor {NSEvent.removeMonitor(searchMonitor);self.searchMonitor=nil}
    compactSearch=nil;searchPopover=nil
  }
  func createEvent() { newEvent() }
  func refreshEvents() { refreshCalendar() }
  private func updateShortcuts() {
    shortcutStrip.items = inSubpage
      ? [("J K", "Scroll"), ("⌃D ⌃U", "Half page"), ("Esc", "Calendar")]
      : [("H L", "Previous / next \(mode.title)"), ("J K", "Select event"), ("↵", "Read"), ("T", "Today"), ("?", "All shortcuts")]
  }
  func controlTextDidChange(_ notification: Notification) {
    if notification.object as? NSTextField === eventTitleField {updateEventSuggestion?();return}
    if notification.object as? NSTextField === dateQueryField {updateDatePreview?();return}
    if notification.object as? NSSearchField === compactSearch {
      search.stringValue=compactSearch?.stringValue ?? "";reloadTask()
    } else if notification.object as? NSSearchField === search { reloadTask() }
  }
  private func scrollCalendar(_ amount: CGFloat) {
    guard let scroll = timelineScroll, let document = scroll.documentView else { return }
    let clip = scroll.contentView
    let y = max(0,min(clip.bounds.minY+amount,document.bounds.height-clip.bounds.height))
    clip.scroll(to:NSPoint(x:clip.bounds.minX,y:y))
    scroll.reflectScrolledClipView(clip)
  }
  private func setBusy(_ value: Bool) {
    busy = value
    accounts.isEnabled = true
  }

  func resume(){Task{await loadAccounts()}}
  private func loadAccounts() async {
    do {
      let previous=accountID
      records = try await core.callAsync(["op": "list_accounts"]) as? [[String: Any]] ?? []
      accounts.removeAllItems()
      accounts.addItem(withTitle:"All accounts")
      accounts.addItems(withTitles: records.map { $0["email"] as? String ?? "Account" })
      if let previous,let index=records.firstIndex(where:{$0["id"] as? String == previous}){accounts.selectItem(at:index+1)}
      if records.isEmpty {
        status.stringValue = "Add a Gmail account in Email to connect your calendar."
        connect.isEnabled = false
        refresh.isEnabled = false
      } else {
        await reload()
      }
    } catch { status.stringValue = error.localizedDescription }
  }
  @objc private func agendaModeChanged(_ sender: NSSegmentedControl) {
    setMode(sender.selectedSegment)
  }
  @objc private func agendaModeMenuChanged(_ sender:NSPopUpButton) { setMode(sender.indexOfSelectedItem) }
  @objc private func accountChanged() { reloadTask() }
  private func reloadTask() { Task { await reload() } }
  func move(_ offset: Int) {
    day = Calendar.current.date(byAdding: mode.component, value: offset * (mode == .week ? 7 : 1), to: day)!
    reloadTask()
  }
  func setMode(_ index: Int) {
    guard let value = CalendarMode(rawValue: index) else { return }
    mode=value;modeControl?.selectedSegment=index;modeMenu?.selectItem(at:index);reloadTask()
  }
  func goToday() { day = Calendar.current.startOfDay(for: Date()); reloadTask() }
  private func calendarKey(_ calendar:[String:Any])->String{(calendar["account_id"] as? String ?? "")+"|"+(calendar["calendar_id"] as? String ?? calendar["id"] as? String ?? "")}
  private func accountName(_ id:String)->String{records.first(where:{$0["id"] as? String == id})?["email"] as? String ?? "Account"}
  private func updateLegend() {
    legend.arrangedSubviews.forEach {legend.removeArrangedSubview($0);$0.removeFromSuperview()}
    var height:CGFloat=0
    for record in records {
      guard let aid=record["id"] as? String else{continue}
      let calendars=collections.filter{$0["account_id"] as? String == aid}
      if let scope=accountID,scope != aid{continue}
      let heading=action(accountName(aid),{[weak self] in guard let self,let index=self.records.firstIndex(where:{$0["id"] as? String == aid}) else{return};self.accounts.selectItem(at:index+1);self.reloadTask()})
      heading.font = .systemFont(ofSize:9,weight:.semibold);heading.contentTintColor=EmailTheme.muted;heading.alignment = .left;heading.lineBreakMode = .byTruncatingTail;heading.widthAnchor.constraint(equalToConstant:152).isActive=true;heading.toolTip="Show only "+accountName(aid);legend.addArrangedSubview(heading);height+=33
      if calendars.isEmpty {
        let connectButton=action("Connect calendar…",{[weak self] in guard let self,let index=self.records.firstIndex(where:{$0["id"] as? String == aid}) else{return};self.accounts.selectItem(at:index+1);self.connectCalendar()})
        connectButton.alignment = .left;connectButton.font = .systemFont(ofSize:11);connectButton.widthAnchor.constraint(equalToConstant:152).isActive=true;connectButton.toolTip="Connect Google Calendar for "+accountName(aid);legend.addArrangedSubview(connectButton);height+=33
      }
      for calendar in calendars {
        guard let cid=calendar["id"] as? String else{continue};let key=calendarKey(calendar);let name=calendar["name"] as? String ?? "Calendar"
        let button=action((hiddenCalendars.contains(key) ? "○  " : "●  ")+name,{[weak self] in guard let self else{return};if !self.hiddenCalendars.insert(key).inserted{self.hiddenCalendars.remove(key)};self.reloadTask()})
        button.contentTintColor=hiddenCalendars.contains(key) ? EmailTheme.muted : CalendarEventTint.color(["calendar_id":cid]);button.alignment = .left;button.font = .systemFont(ofSize:11,weight:.medium);button.widthAnchor.constraint(equalToConstant:152).isActive=true;button.lineBreakMode = .byTruncatingTail;button.toolTip=name+" · "+accountName(aid);legend.addArrangedSubview(button);height+=33
      }
    }
    legendHeight?.constant=max(0,height-3)
  }
  func goToDate(_ date:Date) { day=Calendar.current.startOfDay(for:date);reloadTask() }
  func jumpToDate() {
    datePopover?.close();if let dateMonitor {NSEvent.removeMonitor(dateMonitor);self.dateMonitor=nil}
    let popover=NSPopover();popover.behavior = .transient;popover.delegate=self
    let controller=NSViewController();let view=AgendaSurface(frame:NSRect(x:0,y:0,width:400,height:230))
    let heading=label("Go to date",strong:true);heading.font = .systemFont(ofSize:15,weight:.semibold);view.addSubview(heading)
    let field=NSTextField();field.placeholderString="20 October, next Friday, in 2 weeks…";field.font = .systemFont(ofSize:15)
    field.isBordered=false;field.drawsBackground=false;field.focusRingType = .none;field.setAccessibilityLabel("Date command");field.delegate=self;view.addSubview(field);dateQueryField=field
    let inputSurface=NSView();inputSurface.wantsLayer=true;inputSurface.layer?.backgroundColor=EmailTheme.ink.withAlphaComponent(0.06).cgColor;inputSurface.layer?.cornerRadius=8
    view.addSubview(inputSurface,positioned:.below,relativeTo:field)
    let result=action("",{});(result as? AgendaButton)?.prominent=true;result.alignment = .left;view.addSubview(result)
    let resolved=label("",strong:true);resolved.font = .systemFont(ofSize:13,weight:.semibold);view.addSubview(resolved)
    let detail=label("");detail.font = .systemFont(ofSize:11);view.addSubview(detail)
    let examples=label("Try 20 October · +2w · next Friday");examples.font = .systemFont(ofSize:10);view.addSubview(examples)
    let browse=action("Browse calendar  ↓",{});browse.font = .systemFont(ofSize:11);browse.alignment = .left;view.addSubview(browse)
    let escape=label("esc close");escape.font = .monospacedSystemFont(ofSize:10,weight:.regular);escape.alignment = .right;view.addSubview(escape)
    let picker=AgendaMiniMonth(frame:NSRect(x:48,y:45,width:304,height:248));picker.selectedDate=day;picker.isHidden=true;view.addSubview(picker)
    let previous=action("‹",{}),next=action("›",{});view.addSubview(previous);view.addSubview(next);previous.isHidden=true;next.isHidden=true
    let choose:(Date)->Void={ [weak self,weak popover] date in guard let self else{return};popover?.close();self.interactionWindow?.makeFirstResponder(nil);self.goToDate(date) }
    let format=DateFormatter();format.dateFormat="EEEE, d MMMM yyyy"
    var expanded=false
    func layout(){
      let h:CGFloat=expanded ? 510 : 230
      view.setFrameSize(NSSize(width:400,height:h));popover.contentSize=view.frame.size
      heading.frame=NSRect(x:20,y:h-42,width:340,height:23)
      inputSurface.frame=NSRect(x:16,y:h-94,width:368,height:40);field.frame=NSRect(x:28,y:h-84,width:344,height:22)
      result.frame=NSRect(x:16,y:h-164,width:368,height:60);resolved.frame=NSRect(x:28,y:h-132,width:344,height:20);detail.frame=NSRect(x:28,y:h-151,width:344,height:17)
      examples.frame=NSRect(x:24,y:h-186,width:352,height:15)
      browse.frame=NSRect(x:16,y:12,width:210,height:27);escape.frame=NSRect(x:270,y:18,width:106,height:17)
      picker.isHidden = !expanded;previous.isHidden = !expanded;next.isHidden = !expanded
      previous.frame=NSRect(x:27,y:267,width:30,height:28);next.frame=NSRect(x:343,y:267,width:30,height:28)
      browse.title=expanded ? "Hide calendar  ↑" : "Browse calendar  ↓"
    }
    func preview(){
      let raw=(field.currentEditor()?.string ?? field.stringValue).trimmingCharacters(in:.whitespacesAndNewlines)
      let date=raw.isEmpty ? Calendar.current.startOfDay(for:Date()) : AgendaDateQuery.parse(raw)
      result.isEnabled=date != nil
      result.setAccessibilityLabel(date.map{"Go to "+format.string(from:$0)} ?? "Enter a valid date")
      resolved.stringValue=date.map{format.string(from:$0)} ?? "Keep typing a date…"
      detail.stringValue=date == nil ? "Try a date or a relative phrase below" : "Return to jump · Tab to browse days"
      if let date {picker.selectedDate=date}
    }
    updateDatePreview=preview
    (result as? AgendaButton)?.run={if let date=AgendaDateQuery.parse(field.currentEditor()?.string ?? field.stringValue){choose(date)}else if field.stringValue.isEmpty{choose(Date())}}
    (browse as? AgendaButton)?.run={expanded.toggle();layout()}
    (previous as? AgendaButton)?.run={picker.selectedDate=Calendar.current.date(byAdding:.month,value:-1,to:picker.selectedDate)!}
    (next as? AgendaButton)?.run={picker.selectedDate=Calendar.current.date(byAdding:.month,value:1,to:picker.selectedDate)!}
    picker.onDate=choose;picker.onRebuild={ [weak field,weak picker] in field?.nextKeyView=picker?.selectedDateTarget }
    dateBrowsing=false
    dateSubmit={ [weak self,weak picker,weak result] in if self?.dateBrowsing == true,let picker{choose(picker.selectedDate)}else{result?.performClick(nil)} }
    field.target=result;field.action=#selector(NSButton.performClick(_:));field.cell?.sendsActionOnEndEditing=false
    controller.view=view;popover.contentViewController=controller;datePopover=popover;layout();preview()
    popover.show(relativeTo:dateLabel.bounds,of:dateLabel,preferredEdge:.maxY);field.window?.makeFirstResponder(field)
    dateKeyHandler={ [weak self,weak field,weak popover,weak picker,weak detail] event in
      guard let self,let field,let popover,let picker else{return false}
      if event.keyCode==53{popover.close();self.interactionWindow?.makeFirstResponder(nil);return true}
      if event.keyCode==48 {
        self.dateBrowsing = !event.modifierFlags.contains(.shift)
        if self.dateBrowsing {expanded=true;layout()}
        field.window?.makeFirstResponder(nil);field.window?.makeFirstResponder(self.dateBrowsing ? picker.selectedDateTarget : field)
        detail?.stringValue=self.dateBrowsing ? "Arrow keys choose a day · Return to jump" : "Return to jump · Tab to browse days"
        return true
      }
      if event.keyCode==36{self.dateSubmit?();return true}
      if self.dateBrowsing,let delta=[123:-1,124:1,125:7,126:-7][Int(event.keyCode)] {
        picker.selectedDate=Calendar.current.date(byAdding:.day,value:delta,to:picker.selectedDate)!
        resolved.stringValue=format.string(from:picker.selectedDate);field.window?.makeFirstResponder(picker.selectedDateTarget);return true
      }
      return false
    }
    dateMonitor=NSEvent.addLocalMonitorForEvents(matching:.keyDown){[weak self,weak field] event in
      guard let self,let field,event.window === field.window else{return event};return self.dateKeyHandler?(event) == true ? nil : event
    }
  }

  func showNavigationHelp() {
    datePopover?.close()
    let popover=NSPopover();popover.behavior = .transient;popover.delegate=self
    let controller=NSViewController();let view=AgendaSurface(frame:NSRect(x:0,y:0,width:420,height:460))
    let title=label("Calendar shortcuts",strong:true);title.font = .systemFont(ofSize:17,weight:.semibold);title.frame=NSRect(x:22,y:416,width:340,height:25);view.addSubview(title)
    let close=action("×",{[weak popover] in popover?.close()});close.frame=NSRect(x:366,y:414,width:32,height:28);close.setAccessibilityLabel("Close calendar shortcuts");view.addSubview(close)
    var y:CGFloat=380
    func section(_ name:String,_ items:[(String,String)]) {
      let heading=label(name);heading.font = .systemFont(ofSize:10,weight:.semibold);heading.frame=NSRect(x:24,y:y,width:350,height:17);view.addSubview(heading);y-=29
      for (keys,name) in items {
        let text=label(name);text.font = .systemFont(ofSize:12);text.textColor=EmailTheme.ink;text.frame=NSRect(x:24,y:y,width:240,height:21);view.addSubview(text)
        let key=NSTextField(labelWithString:keys);key.font = .monospacedSystemFont(ofSize:11,weight:.medium);key.textColor=EmailTheme.ink;key.alignment = .center;key.wantsLayer=true;key.layer?.cornerRadius=5;key.layer?.backgroundColor=EmailTheme.ink.withAlphaComponent(0.08).cgColor;key.frame=NSRect(x:282,y:y,width:112,height:22);view.addSubview(key);y-=29
      };y-=12
    }
    section("MOVE THROUGH TIME",[("H / L  ← / →","Previous / next period"),("D W M Y","Day, week, month, year"),("T","Today"),("B","Go to date…")])
    section("WORK WITH EVENTS",[("J / K  ↓ / ↑","Select event"),("Return","Open selected event"),("E","Edit opened event"),("C","Create event"),("/","Search this date range")])
    let footer=label("Esc to close · G then I returns to mail");footer.font = .systemFont(ofSize:10);footer.frame=NSRect(x:24,y:16,width:370,height:18);view.addSubview(footer)
    controller.view=view;popover.contentViewController=controller;datePopover=popover
    dateKeyHandler={ [weak popover] event in if event.keyCode==53{popover?.close();return true};return false }
    popover.show(relativeTo:dateLabel.bounds,of:dateLabel,preferredEdge:.maxY)
    dateMonitor=NSEvent.addLocalMonitorForEvents(matching:.keyDown){[weak self,weak popover] event in guard popover?.isShown == true else{return event};return self?.dateKeyHandler?(event) == true ? nil : event}
  }
  private func clear() {
    formKeyViews = []
    interactionWindow?.autorecalculatesKeyViewLoop = true
    interactionWindow?.initialFirstResponder = accounts
    rows.arrangedSubviews.forEach {
      rows.removeArrangedSubview($0)
      $0.removeFromSuperview()
    }
    interactionWindow?.recalculateKeyViewLoop()
  }
  private func reload() async {
    guard !records.isEmpty else{return}
    let id=accountID
    generation += 1
    let serial = generation
    inSubpage = false
    readingEvent = false
    currentGrid = nil
    currentOverview = nil
    updateShortcuts()
    rows.spacing = 0
    let (start, end) = range
    let dateFormat = DateFormatter()
    dateFormat.dateFormat=mode == .year ? "yyyy" : mode == .month ? "MMMM yyyy" : "EEE, d MMM yyyy"
    if mode == .week {
      let last=Calendar.current.date(byAdding:.day,value:-1,to:rangeEnd)!
      dateFormat.dateFormat=Calendar.current.isDate(rangeStart,equalTo:last,toGranularity:.month) ? "d" : "d MMM"
      let first=dateFormat.string(from:rangeStart)
      dateFormat.dateFormat="d MMM yyyy"
      dateLabel.stringValue=first+" – "+dateFormat.string(from:last)
    } else {dateLabel.stringValue=dateFormat.string(from:rangeStart)}
    dateLabel.toolTip="\(dateLabel.stringValue) · Go to date (B)"
    previousButton?.toolTip="Previous \(mode.title) (H or ←)";previousButton?.setAccessibilityLabel("Previous \(mode.title)")
    nextButton?.toolTip="Next \(mode.title) (L or →)";nextButton?.setAccessibilityLabel("Next \(mode.title)")
    do {
      var calendarsRequest:[String:Any]=["op":"list_calendars"];if let id{calendarsRequest["account_id"]=id}
      collections=try await core.callAsync(calendarsRequest) as? [[String:Any]] ?? []
      guard serial==generation else{return}
      var agendaRequest:[String:Any]=["op":"list_agenda","start_ms":start,"end_ms":end,"limit":10000,"deduplicate_shared":id == nil]
      if let id{agendaRequest["account_id"]=id}
      agendaRequest["hidden_sources"]=collections.filter{hiddenCalendars.contains(calendarKey($0))}.map{["account_id":$0["account_id"] as? String ?? "","calendar_id":$0["id"] as? String ?? ""]}
      let events=try await core.callAsync(agendaRequest) as? [[String:Any]] ?? []
      guard serial == generation else { return }
      var savedScroll = timelineScroll?.contentView.bounds.origin ?? .zero
      savedScroll.y = max(0,(savedScroll.y-82)*pendingScrollScale+82);pendingScrollScale=1
      clear()
      miniMonth.selectedDate = day
      let oldID = selectedEvents.indices.contains(selectedEventIndex) ? selectedEvents[selectedEventIndex]["id"] as? String : nil
      let query = search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
      updateLegend()
      selectedEvents = events.filter {
        !hiddenCalendars.contains(calendarKey($0)) &&
        (query.isEmpty || (($0["title"] as? String ?? "") + " " + ($0["location"] as? String ?? "")).localizedCaseInsensitiveContains(query))
      }
      selectedEventIndex = selectedEvents.firstIndex(where: { $0["id"] as? String == oldID }) ?? 0
      let grid: NSView
      if mode == .month || mode == .year {
        let overview = CalendarOverviewGrid(start: rangeStart, year: mode == .year, events: selectedEvents,
          onEvent: { [weak self] in self?.showEvent($0) }, onDate: { [weak self] date in
            guard let self else { return }; self.day = date; self.setMode(0)
          }, onMonth: { [weak self] date in
            guard let self else { return }; self.day = date; self.setMode(2)
          })
        currentOverview = overview
        grid = overview
      } else {
        let timeline = CalendarWeekGrid(start: rangeStart, days: mode == .week ? 7 : 1, events: selectedEvents, hourHeight:[44,58,76][hourDensity]) { [weak self] event in self?.showEvent(event) }
        timeline.onCreate = { [weak self] date in self?.newEvent(at:date) }
        timeline.toolTip="Click an empty time to create an event · + / − adjusts hour density"
        currentGrid = timeline
        timeline.setDraft(currentEventDraft)
        grid = timeline
      }
      rows.addArrangedSubview(grid)
      if selectedEvents.indices.contains(selectedEventIndex) {
        let id = selectedEvents[selectedEventIndex]["id"] as? String
        currentGrid?.selectEvent(id, scroll: false)
        currentOverview?.selectEvent(id, scroll: false)
      }
      grid.widthAnchor.constraint(equalTo: rows.widthAnchor).isActive = true
      rows.layoutSubtreeIfNeeded()
      timelineScroll?.contentView.scroll(to:savedScroll)
      timelineScroll?.reflectScrolledClipView(timelineScroll!.contentView)
      let relevant=records.filter{id == nil || $0["id"] as? String == id}
      var connected:[String]=[];var supported=false
      for record in relevant {
        guard let aid=record["id"] as? String else{continue}
        let state=try await core.callAsync(["op":"calendar_status","account_id":aid]) as? [String:Any] ?? [:]
        guard serial==generation else{return}
        supported = supported || (state["supported"] as? Bool ?? false)
        if state["connected"] as? Bool == true{connected.append(aid)}
      }
      connectedAccountIDs=connected
      connect.isHidden = id == nil ? !connected.isEmpty : !connected.isEmpty
      connect.title=id == nil ? "Connect a calendar…" : "Connect Google Calendar…"
      connect.isEnabled=supported && !busy;refresh.isEnabled = !connected.isEmpty && !busy
      status.stringValue = !connected.isEmpty ? (busy ? "Refreshing… Cached events remain available." : id == nil ? "All connected accounts · Saved for offline access" : "Saved for offline access") : "Choose an account in Calendars, then connect Google Calendar."
      if !connected.isEmpty && !busy {
        let key="\(id ?? "*"):\(start):\(end)"
        let attempted=refreshAttempts[key].map{Date().timeIntervalSince($0)<60} ?? false
        let fresh=connected.allSatisfy{aid in freshRanges.contains{$0.0==aid && $0.1<=start && $0.2>=end && Date().timeIntervalSince($0.3)<300}}
        if !attempted && !fresh{refreshCalendar()}
      }
    } catch { if serial == generation { status.stringValue = error.localizedDescription } }
  }
  private func time(_ ms: Any?) -> String {
    DateFormatter.localizedString(
      from: Date(timeIntervalSince1970: ((ms as? NSNumber)?.doubleValue ?? 0) / 1000),
      dateStyle: .none, timeStyle: .short)
  }
  private func allDay(_ ms: Any?) -> String {
    let formatter = DateFormatter()
    formatter.dateFormat = "yyyy-MM-dd"
    formatter.timeZone = TimeZone(secondsFromGMT: 0)
    return "All day · "
      + formatter.string(
        from: Date(timeIntervalSince1970: ((ms as? NSNumber)?.doubleValue ?? 0) / 1000))
      + " (calendar date)"
  }

  private func addEvent(_ event: [String: Any]) {
    let row = NSStackView()
    row.orientation = .horizontal
    row.alignment = .top
    row.spacing = 18
    row.edgeInsets = NSEdgeInsets(top: 12, left: 0, bottom: 12, right: 0)
    let isAllDay = event["all_day"] as? Bool == true
    var stamp = isAllDay ? "All day" : "\(time(event["start_ms"])) – \(time(event["end_ms"]))"
    if !isAllDay, let begin = event["start_ms"] as? NSNumber, let finish = event["end_ms"] as? NSNumber {
      let endDate = Date(timeIntervalSince1970: finish.doubleValue / 1000)
      if !Calendar.current.isDate(Date(timeIntervalSince1970: begin.doubleValue / 1000), inSameDayAs: endDate) {
        let format = DateFormatter(); format.dateFormat = "d MMM, HH:mm"
        stamp = "\(time(begin)) – \(format.string(from: endDate))"
      }
    }
    let timing = label(stamp)
    timing.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
    timing.widthAnchor.constraint(equalToConstant: 118).isActive = true
    row.addArrangedSubview(timing)
    let card = NSStackView()
    card.orientation = .vertical
    card.alignment = .leading
    card.spacing = 5
    let title = label(event["title"] as? String ?? "Untitled event", strong: true)
    title.font = .systemFont(ofSize: 13, weight: .medium)
    card.addArrangedSubview(title)
    if let location = event["location"] as? String, !location.isEmpty {
      let place = label(location)
      place.font = .systemFont(ofSize: 11)
      card.addArrangedSubview(place)
      place.widthAnchor.constraint(equalTo: card.widthAnchor).isActive = true
    }
    let writable = collections.contains {
      $0["account_id"] as? String == event["account_id"] as? String
        && $0["id"] as? String == event["calendar_id"] as? String
        && $0["can_write"] as? Bool == true
    }
    if event["can_respond"] as? Bool == true && writable {
      let actions = NSStackView()
      actions.spacing = 8
      for (caption, response) in [("Accept", "accepted"), ("Maybe", "tentative"), ("Decline", "declined")] {
        actions.addArrangedSubview(action(caption, { [weak self] in self?.respond(event, response) }))
      }
      card.addArrangedSubview(actions)
    }
    row.addArrangedSubview(card)
    rows.addArrangedSubview(row)
    row.widthAnchor.constraint(equalTo: rows.widthAnchor).isActive = true
    title.widthAnchor.constraint(equalTo: card.widthAnchor).isActive = true
    let line = NSBox()
    line.boxType = .separator
    rows.addArrangedSubview(line)
    line.widthAnchor.constraint(equalTo: rows.widthAnchor).isActive = true
  }
  private func backToCalendar() {
    rows.addArrangedSubview(action("← Back to calendar", { [weak self] in self?.reloadTask() }))
  }
  private func showEvent(_ event: [String: Any]) {
    selectedEventIndex=selectedEvents.firstIndex(where:{$0["id"] as? String == event["id"] as? String}) ?? selectedEventIndex
    currentGrid?.selectEvent(event["id"] as? String,scroll:false)
    currentOverview?.selectEvent(event["id"] as? String,scroll:false)
    detailPopover?.close()
    let popover=NSPopover();popover.behavior = .transient;popover.delegate=self
    let controller=NSViewController();let view=NSView(frame:NSRect(x:0,y:0,width:356,height:280))
    let stack=NSStackView();stack.orientation = .vertical;stack.alignment = .leading;stack.spacing=13
    stack.translatesAutoresizingMaskIntoConstraints=false;view.addSubview(stack)
    NSLayoutConstraint.activate([stack.topAnchor.constraint(equalTo:view.topAnchor,constant:20),stack.leadingAnchor.constraint(equalTo:view.leadingAnchor,constant:20),stack.trailingAnchor.constraint(equalTo:view.trailingAnchor,constant:-20),stack.bottomAnchor.constraint(lessThanOrEqualTo:view.bottomAnchor,constant:-18)])
    let name=collections.first(where:{$0["id"] as? String == event["calendar_id"] as? String})?["name"] as? String ?? "Calendar"
    let source=label("●  "+name);source.textColor=CalendarEventTint.color(event);stack.addArrangedSubview(source)
    let title=label(event["title"] as? String ?? "Untitled event",strong:true);title.font = .systemFont(ofSize:19,weight:.semibold);stack.addArrangedSubview(title)
    let format=DateFormatter();format.dateStyle = .full;format.timeStyle = .short
    let begin=Date(timeIntervalSince1970:((event["start_ms"] as? NSNumber)?.doubleValue ?? 0)/1000)
    let finish=Date(timeIntervalSince1970:((event["end_ms"] as? NSNumber)?.doubleValue ?? 0)/1000)
    stack.addArrangedSubview(label(event["all_day"] as? Bool == true ? allDay(event["start_ms"]) : format.string(from:begin)+"\n"+format.string(from:finish)))
    if let place=event["location"] as? String,!place.isEmpty{stack.addArrangedSubview(label(place,strong:true))}
    if let description=event["description"] as? String,!description.isEmpty{
      let body=label(String(description.prefix(300))+(description.count>300 ? "…" : ""));body.maximumNumberOfLines=5;body.lineBreakMode = .byTruncatingTail;body.toolTip=description;stack.addArrangedSubview(body);body.heightAnchor.constraint(lessThanOrEqualToConstant:85).isActive=true
    }
    let writable=collections.contains{$0["id"] as? String == event["calendar_id"] as? String && $0["account_id"] as? String == event["account_id"] as? String && $0["can_write"] as? Bool == true}
    if writable && event["can_respond"] as? Bool == true{
      let response=NSStackView();response.spacing=8
      for (name,value) in [("Accept","accepted"),("Maybe","tentative"),("Decline","declined")]{response.addArrangedSubview(action(name,{[weak self,weak popover] in popover?.close();self?.respond(event,value)}))}
      stack.addArrangedSubview(response)
    }
    let canEdit=writable && event["can_edit"] as? Bool == true && !(event["etag"] as? String ?? "").isEmpty
    var editButton:NSButton?
    if canEdit {
      let edit=action("Edit event",{[weak self,weak popover] in popover?.close();self?.newEvent(editing:event)})
      edit.toolTip="Edit event (E)";(edit as? AgendaButton)?.prominent=true;stack.addArrangedSubview(edit);editButton=edit
    } else {
      let reason=label(event["all_day"] as? Bool == true ? "All-day events can be edited in Google Calendar." : writable ? "Editing is not available for this event. Use Google Calendar." : "This calendar is read-only.")
      reason.font = .systemFont(ofSize:10);stack.addArrangedSubview(reason)
      stack.addArrangedSubview(action("Open in Google Calendar",{
        let value=event["html_url"] as? String ?? "https://calendar.google.com/calendar/u/0/r"
        if let url=URL(string:value),url.scheme=="https",["calendar.google.com","www.google.com"].contains(url.host ?? ""){NSWorkspace.shared.open(url)}
      }))
    }
    stack.addArrangedSubview(action("Close",{[weak self,weak popover] in popover?.close();self?.interactionWindow?.makeFirstResponder(self?.currentGrid)}))
    for item in stack.arrangedSubviews{item.widthAnchor.constraint(lessThanOrEqualToConstant:316).isActive=true}
    controller.view=view;view.layoutSubtreeIfNeeded();popover.contentViewController=controller
    popover.contentSize=NSSize(width:356,height:min(580,max(230,stack.fittingSize.height+40)))
    detailPopover=popover
    detailEdit={ [weak editButton] in editButton?.performClick(nil) }
    let anchor=currentGrid?.anchor(for:event["id"] as? String ?? "") ?? dateLabel
    popover.show(relativeTo:anchor.bounds,of:anchor,preferredEdge:.maxX)
    detailMonitor=NSEvent.addLocalMonitorForEvents(matching:.keyDown){[weak self,weak popover,weak editButton] key in
      guard let self,let popover,key.window === popover.contentViewController?.view.window else{return key}
      if key.keyCode==53{popover.close();self.interactionWindow?.makeFirstResponder(nil);return nil}
      if key.charactersIgnoringModifiers?.lowercased()=="e",key.modifierFlags.intersection([.command,.control,.option]).isEmpty,let editButton{editButton.performClick(nil);return nil}
      return key
    }
  }
  @objc private func connectCalendar() {
    guard !busy else{return}
    guard let id=accountID else{if railCompact{showNavigator()}else{accounts.performClick(nil)};status.stringValue="Choose an account in Calendars to connect it.";return}
    setBusy(true)
    connect.isEnabled = false
    status.stringValue = "Finish calendar permission approval in your browser…"
    Task {
      do {
        _ = try await core.callAsync(["op": "connect_calendar", "account_id": id])
        setBusy(false)
        await reload()
        refreshCalendar()
      } catch {
        setBusy(false)
        connect.isEnabled = true
        status.stringValue = error.localizedDescription
      }
    }
  }
  @objc private func refreshCalendar() {
    guard !busy else{return}
    let ids=accountID.map{[$0]} ?? connectedAccountIDs
    guard !ids.isEmpty else{return}
    setBusy(true);refresh.isEnabled=false;status.stringValue="Refreshing connected calendars… Cached events remain available."
    let (start,end)=range;refreshAttempts["\(accountID ?? "*"):\(start):\(end)"]=Date()
    Task {
      var failures:[String]=[]
      for id in ids {
        do{_ = try await core.callAsync(["op":"sync_calendar","account_id":id,"start_ms":start,"end_ms":end]);freshRanges.append((id,start,end,Date()))}
        catch{failures.append(accountName(id)+": "+error.localizedDescription)}
      }
      freshRanges=Array(freshRanges.suffix(32));setBusy(false);await reload()
      if !failures.isEmpty{status.stringValue="Some calendars could not refresh; saved events remain available.";status.toolTip=failures.joined(separator:"\n")}
    }
  }
  private func respond(_ event: [String: Any], _ response: String) {
    guard let id = event["account_id"] as? String, (accountID == nil || id == accountID),
      let cid = event["calendar_id"] as? String, let eid = event["id"] as? String, !busy
    else { return }
    setBusy(true)
    status.stringValue = "Sending your response…"
    Task {
      do {
        _ = try await core.callAsync([
          "op": "respond_calendar_invitation", "account_id": id, "calendar_id": cid,
          "event_id": eid, "response": response,
        ])
        setBusy(false)
        await reload()
        status.stringValue = "Response sent to Google Calendar."
      } catch {
        setBusy(false)
        status.stringValue = error.localizedDescription
      }
    }
  }
  private func newEvent(at selectedDate:Date? = nil, editing event:[String:Any]? = nil) {
    let scope=accountID
    let writable=collections.filter{$0["can_write"] as? Bool == true && (scope == nil || $0["account_id"] as? String == scope) && (event == nil || $0["account_id"] as? String == event?["account_id"] as? String)}
    guard !writable.isEmpty else{status.stringValue="Connect and refresh a writable calendar first.";return}
    if let event,(!(event["can_edit"] as? Bool ?? false) || (event["etag"] as? String ?? "").isEmpty){status.stringValue="Refresh Calendar to check whether this event is editable.";return}
    detailPopover?.close();editorPopover?.close()
    let popover=NSPopover();popover.behavior = .semitransient;popover.delegate=self
    let popWidth:CGFloat=400, compactHeight:CGFloat=344
    let controller=NSViewController();let view=AgendaSurface(frame:NSRect(x:0,y:0,width:popWidth,height:compactHeight));view.wantsLayer=true;view.layer?.cornerRadius=12;view.layer?.borderWidth=1;view.layer?.borderColor=EmailTheme.line.cgColor
    let heading=label(event == nil ? "NEW EVENT" : "EDIT EVENT");heading.font = .systemFont(ofSize:10,weight:.semibold);heading.textColor=EmailTheme.muted;heading.frame=NSRect(x:24,y:compactHeight-40,width:352,height:18);heading.autoresizingMask=[.minYMargin];view.addSubview(heading)
    let scroll=NSScrollView(frame:NSRect(x:0,y:76,width:popWidth,height:compactHeight-126));scroll.drawsBackground=false;scroll.hasVerticalScroller=true;scroll.autohidesScrollers=true;scroll.autoresizingMask=[.width,.height]
    let document=AgendaDocument();document.translatesAutoresizingMaskIntoConstraints=false;scroll.documentView=document;view.addSubview(scroll)
    let fields=NSStackView();fields.orientation = .vertical;fields.alignment = .leading;fields.spacing=10;fields.translatesAutoresizingMaskIntoConstraints=false;document.addSubview(fields)
    NSLayoutConstraint.activate([document.widthAnchor.constraint(equalTo:scroll.contentView.widthAnchor),fields.leadingAnchor.constraint(equalTo:document.leadingAnchor,constant:24),fields.trailingAnchor.constraint(equalTo:document.trailingAnchor,constant:-24),fields.topAnchor.constraint(equalTo:document.topAnchor,constant:0),fields.bottomAnchor.constraint(equalTo:document.bottomAnchor,constant:-8)])
    func row(_ caption:String,_ field:NSView,in stack:NSStackView){
      let line=NSStackView();line.orientation = .horizontal;line.alignment = .centerY;line.spacing=12
      let name=label(caption);name.font = .systemFont(ofSize:12);name.textColor=EmailTheme.muted;name.widthAnchor.constraint(equalToConstant:56).isActive=true
      line.addArrangedSubview(name);line.addArrangedSubview(field);stack.addArrangedSubview(line);line.widthAnchor.constraint(equalTo:stack.widthAnchor).isActive=true;field.heightAnchor.constraint(greaterThanOrEqualToConstant:28).isActive=true
    }
    let title=NSTextField(string:event?["title"] as? String ?? "");title.placeholderString="Add a title";title.setAccessibilityLabel("Event title");title.font = .systemFont(ofSize:24,weight:.medium);title.isBordered=false;title.drawsBackground=false;title.focusRingType = .none;title.textColor=EmailTheme.ink;title.placeholderAttributedString=NSAttributedString(string:"Add a title",attributes:[.font:NSFont.systemFont(ofSize:24,weight:.medium),.foregroundColor:EmailTheme.muted])
    fields.addArrangedSubview(title);title.widthAnchor.constraint(equalTo:fields.widthAnchor).isActive=true;title.heightAnchor.constraint(equalToConstant:42).isActive=true
    let examples=label("Try Lunch 12h · Lunch tomorrow 12:30 for 1h");examples.font = .systemFont(ofSize:11);examples.textColor=EmailTheme.muted;fields.addArrangedSubview(examples)
    let suggestion=action("",{});suggestion.font = .systemFont(ofSize:12,weight:.medium);suggestion.alignment = .left;suggestion.cell?.wraps=true;suggestion.cell?.lineBreakMode = .byWordWrapping;suggestion.setAccessibilityLabel("Event date and time. Expand to edit")
    fields.addArrangedSubview(suggestion);suggestion.widthAnchor.constraint(equalTo:fields.widthAnchor).isActive=true;suggestion.heightAnchor.constraint(equalToConstant:34).isActive=true
    let destination=AgendaDestination(frame:.zero,pullsDown:false);destination.addItems(withTitles:writable.map{calendar in
      let name=calendar["name"] as? String ?? "Calendar",owner=accountName(calendar["account_id"] as? String ?? "")
      return name.caseInsensitiveCompare(owner) == .orderedSame ? name : name+" · "+owner
    });destination.setAccessibilityLabel("Event calendar");destination.font = .systemFont(ofSize:12)
    if let event,let index=writable.firstIndex(where:{$0["id"] as? String == event["calendar_id"] as? String && $0["account_id"] as? String == event["account_id"] as? String}){destination.selectItem(at:index);destination.isEnabled=false}
    row("Calendar",destination,in:fields)
    let begin=event.flatMap{($0["start_ms"] as? NSNumber).map{Date(timeIntervalSince1970:$0.doubleValue/1000)}} ?? selectedDate ?? day.addingTimeInterval(9*3600)
    let finish=event.flatMap{($0["end_ms"] as? NSNumber).map{Date(timeIntervalSince1970:$0.doubleValue/1000)}} ?? begin.addingTimeInterval(1800)
    func dateInput(_ date:Date,_ name:String)->NSDatePicker{
      let picker=AgendaDateInput(frame:.zero);picker.datePickerStyle = .textFieldAndStepper;picker.datePickerElements=[.yearMonthDay,.hourMinute];picker.dateValue=date;picker.locale=Locale(identifier:"en_GB");picker.timeZone = .current;picker.font = .systemFont(ofSize:13);picker.isBordered=false;picker.drawsBackground=false;picker.setAccessibilityLabel(name);return picker
    }
    let start=dateInput(begin,"Event starts"),end=dateInput(finish,"Event ends")
    let timeFields=NSStackView();timeFields.orientation = .vertical;timeFields.alignment = .leading;timeFields.spacing=10;timeFields.isHidden=true;var editingTime=false
    row("Starts",start,in:timeFields);row("Ends",end,in:timeFields)
    fields.addArrangedSubview(timeFields);timeFields.widthAnchor.constraint(equalTo:fields.widthAnchor).isActive=true
    let zone=label(TimeZone.current.identifier.replacingOccurrences(of:"_",with:" "));zone.font = .systemFont(ofSize:10);zone.textColor=EmailTheme.muted;timeFields.addArrangedSubview(zone)
    let extra=NSStackView();extra.orientation = .vertical;extra.alignment = .leading;extra.spacing=10
    let location=NSTextField(string:event?["location"] as? String ?? "");location.placeholderString="Location or meeting link";location.setAccessibilityLabel("Event location");row("Where",location,in:extra)
    let notes=NSTextField(string:event?["description"] as? String ?? "");notes.placeholderString="Notes";notes.setAccessibilityLabel("Event notes");notes.maximumNumberOfLines=3;notes.cell?.wraps=true;notes.cell?.isScrollable=false;row("Notes",notes,in:extra);notes.heightAnchor.constraint(equalToConstant:56).isActive=true
    let attendees=NSTextField();attendees.placeholderString="Emails, separated by commas";attendees.setAccessibilityLabel("Event guests")
    if event == nil{row("Guests",attendees,in:extra)}
    let guestNote=label(event == nil ? "Guests receive an invitation when you create the event." : "Existing guests are kept and receive updates when saved.");guestNote.font = .systemFont(ofSize:10);guestNote.textColor=EmailTheme.muted;extra.addArrangedSubview(guestNote)
    let details=action("Add details",{});details.toolTip="Show location, notes and guests";fields.addArrangedSubview(details)
    fields.addArrangedSubview(extra);extra.widthAnchor.constraint(equalTo:fields.widthAnchor).isActive=true
    var expanded = !location.stringValue.isEmpty || !notes.stringValue.isEmpty
    extra.isHidden = !expanded;details.title=expanded ? "Hide details" : "Add details"
    let error=label("");error.font = .systemFont(ofSize:10);error.frame=NSRect(x:24,y:42,width:352,height:28);view.addSubview(error)
    let cancel=action("Cancel",{[weak self,weak popover] in popover?.close();self?.interactionWindow?.makeFirstResponder(nil)});cancel.frame=NSRect(x:24,y:10,width:64,height:30);view.addSubview(cancel)
    let save=action(event == nil ? "Create event" : "Save changes",{})
    save.frame=NSRect(x:246,y:10,width:130,height:30);(save as? AgendaButton)?.prominent=true;save.toolTip="Save event (⌘Return)";view.addSubview(save)
    let hint=label("Tab next · ⌘↵ save");hint.font = .systemFont(ofSize:10);hint.textColor=EmailTheme.muted;hint.frame=NSRect(x:98,y:18,width:145,height:16);view.addSubview(hint)
    let updateKeyOrder:()->Void = { [weak self] in
      guard let self else{return}
      var keyViews: [NSView] = [title,suggestion]
      if event == nil { keyViews.append(destination) }
      if editingTime { keyViews.append(contentsOf: [start,end]) }
      keyViews.append(details)
      if expanded {
        keyViews.append(contentsOf: [location,notes])
        if event == nil { keyViews.append(attendees) }
      }
      keyViews.append(contentsOf: [save,cancel])
      self.formKeyViews=keyViews
      for (i,field) in self.formKeyViews.enumerated(){field.nextKeyView=self.formKeyViews[(i+1)%self.formKeyViews.count]}
    }
    (details as? AgendaButton)?.run={ [weak popover,weak details] in
      expanded.toggle();extra.isHidden = !expanded;details?.title=expanded ? "Hide details" : "Add details"
      popover?.contentSize=NSSize(width:popWidth,height:(expanded ? 540 : compactHeight)+(editingTime ? 96 : 0));updateKeyOrder()
      if expanded{location.window?.makeFirstResponder(location)}
    }
    var resolvedTitle:String? = nil
    var queryInvalid=false
    var previousStart=begin
    let refreshDraft:()->Void = { [weak self,weak start,weak end,weak title,weak suggestion] in
      guard let start,let end,let title,let suggestion else{return}
      let date=DateFormatter();date.dateFormat="EEE, d MMM";let time=DateFormatter();time.locale=Locale(identifier:"en_GB");time.dateFormat="HH:mm"
      let endText=Calendar.current.isDate(start.dateValue,inSameDayAs:end.dateValue) ? time.string(from:end.dateValue) : date.string(from:end.dateValue)+" "+time.string(from:end.dateValue)
      suggestion.title=date.string(from:start.dateValue)+" · "+time.string(from:start.dateValue)+"–"+endText+"  ⌄"
      previousStart=start.dateValue
      let raw=title.currentEditor()?.string ?? title.stringValue
      self?.currentEventDraft=(resolvedTitle ?? raw,start.dateValue,end.dateValue)
      self?.currentGrid?.setDraft(self?.currentEventDraft)
    }
    (start as? AgendaDateInput)?.onChange={ [weak start,weak end,weak title] in guard let start,let end else{return};end.dateValue=end.dateValue.addingTimeInterval(start.dateValue.timeIntervalSince(previousStart));if let resolvedTitle{title?.stringValue=resolvedTitle};refreshDraft()};(end as? AgendaDateInput)?.onChange={ [weak title] in if let resolvedTitle{title?.stringValue=resolvedTitle};refreshDraft()}
    destination.onChange=refreshDraft
    eventTitleField=title
    updateEventSuggestion={
      let raw=title.currentEditor()?.string ?? title.stringValue
      let parsed=AgendaEventQuery.parse(raw,selectedDay:begin)
      resolvedTitle=parsed?.title
      let queryHint=AgendaEventQuery.validationHint(raw)
      queryInvalid=queryHint != nil || (parsed == nil && raw.range(of:#"(?: at \d| \d{1,2}(?::\d+)?(?:h|am|pm)?(?: for .*)?$)"#,options:[.regularExpression,.caseInsensitive]) != nil)
      if let parsed {start.dateValue=parsed.start;end.dateValue=parsed.end}
      examples.stringValue=parsed == nil ? "Try Lunch tomorrow 12h for 1h" : "Title: "+parsed!.title
      error.stringValue=queryHint ?? (queryInvalid ? "Check the time — for example, Lunch 12:30 for 1h." : "")
      refreshDraft()
    }
    (suggestion as? AgendaButton)?.run={ [weak popover] in
      editingTime.toggle();timeFields.isHidden = !editingTime
      popover?.contentSize=NSSize(width:popWidth,height:(expanded ? 540 : compactHeight)+(editingTime ? 96 : 0));updateKeyOrder()
      if editingTime{start.window?.makeFirstResponder(start)}
    }
    applyEventSuggestion={suggestion.window?.makeFirstResponder(suggestion);return true}
    (save as? AgendaButton)?.run={ [weak self,weak popover,weak save] in
      if queryInvalid {error.stringValue="Check the time before creating the event.";return}
      // Commit the active field editor before reading form values on ⌘Return.
      save?.window?.makeFirstResponder(nil)
      guard let self,!self.busy,self.accountID==scope else{error.stringValue="Calendar is refreshing. Try saving in a moment.";return}
      guard !title.stringValue.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty else{error.stringValue="Give this event a title.";title.window?.makeFirstResponder(title);return}
      let begin=start.dateValue,finish=end.dateValue
      guard finish>begin else{error.stringValue="End time must be after start time.";end.window?.makeFirstResponder(end);return}
      let calendarID=writable[destination.indexOfSelectedItem]["id"] as? String ?? ""
      let id=writable[destination.indexOfSelectedItem]["account_id"] as? String ?? ""
      var request:[String:Any]=["op":event == nil ? "create_calendar_event" : "update_calendar_event","account_id":id,"calendar_id":calendarID,"title":resolvedTitle ?? title.stringValue,"start_ms":Int64(begin.timeIntervalSince1970*1000),"end_ms":Int64(finish.timeIntervalSince1970*1000),"location":location.stringValue,"description":notes.stringValue]
      if let event{request["event_id"]=event["id"] as? String ?? "";request["expected_etag"]=event["etag"] as? String ?? "";request["edit_scope"]=(event["recurring_event_id"] as? String ?? "").isEmpty ? "single" : "instance"}
      else{request["attendee_emails"]=attendees.stringValue.split(separator:",").map{$0.trimmingCharacters(in:.whitespacesAndNewlines)}.filter{!$0.isEmpty}}
      save?.isEnabled=false;self.setBusy(true);error.stringValue=event == nil ? "Creating…" : "Saving…"
      Task{do{_ = try await self.core.callAsync(request);self.setBusy(false);popover?.close();await self.reload();self.status.stringValue=event == nil ? "Event created." : "Event updated."}catch let failure{self.setBusy(false);save?.isEnabled=true;error.stringValue=failure.localizedDescription}}
    }
    for field in [title,location,notes,attendees]{field.delegate=self}
    updateKeyOrder()
    editorSubmit={ [weak save] in save?.performClick(nil) }
    controller.view=view;popover.contentViewController=controller;popover.contentSize=NSSize(width:popWidth,height:expanded ? 540 : compactHeight);editorPopover=popover
    let anchor=eventCreateAnchor ?? dateLabel
    popover.show(relativeTo:anchor.bounds,of:anchor,preferredEdge:.maxY);title.window?.makeFirstResponder(title);refreshDraft()
    editorMonitor=NSEvent.addLocalMonitorForEvents(matching:.keyDown){[weak self,weak popover,weak title,weak save] key in
      guard let self,let popover,let title,key.window === title.window else{return key}
      if key.keyCode==53{popover.close();self.interactionWindow?.makeFirstResponder(nil);return nil}
      if key.keyCode==36,key.modifierFlags.contains(.command){save?.performClick(nil);return nil}
      return key
    }
  }

  func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
    if control === eventTitleField,selector == #selector(NSResponder.insertNewline(_:)),applyEventSuggestion?() == true{return true}
    guard let index = formKeyViews.firstIndex(where: { $0 === control }), !formKeyViews.isEmpty
    else { return false }
    if selector == #selector(NSResponder.insertTab(_:))
      || selector == #selector(NSResponder.insertBacktab(_:))
    {
      let direction = selector == #selector(NSResponder.insertBacktab(_:)) ? -1 : 1
      let next = (index + direction + formKeyViews.count) % formKeyViews.count
      control.window?.makeFirstResponder(formKeyViews[next])
      return true
    }
    return false
  }

  private func availability() {
    guard !busy else{return}
    let id=accountID
    let start = Int64(
      Calendar.current.date(bySettingHour: 9, minute: 0, second: 0, of: day)!.timeIntervalSince1970
        * 1000)
    let end = Int64(
      Calendar.current.date(bySettingHour: 17, minute: 0, second: 0, of: day)!.timeIntervalSince1970
        * 1000)
    generation += 1
    let serial = generation
    Task {
      do {
        var request:[String:Any]=["op":"availability","start_ms":start,"end_ms":end,"duration_minutes":30]
        if let id{request["account_id"]=id}
        let result=try await core.callAsync(request) as? [String:Any] ?? [:]
        guard serial == generation else { return }
        clear()
        rows.spacing = 12
        inSubpage = true
        updateShortcuts()
        backToCalendar()
        rows.addArrangedSubview(label("Suggested free times · 30 minutes", strong: true))
        status.stringValue =
          result["coverage_complete"] as? Bool == true
          ? "30-minute suggestions, 09:00–17:00 on the selected day. Other calendars may conflict; confirm before sharing."
          : "09:00–17:00 on the selected day. Calendar coverage is incomplete; these are suggestions, not confirmed availability."
        let slots = result["slots"] as? [[String: Any]] ?? []
        var lines: [String] = []
        for slot in slots.prefix(5) {
          let line = "\(time(slot["start_ms"])) – \(time(slot["end_ms"]))"
          lines.append(line)
          rows.addArrangedSubview(label(line, strong: true))
        }
        if !lines.isEmpty {
          let formatter = DateFormatter()
          formatter.dateStyle = .medium
          formatter.timeStyle = .none
          let copied =
            "Suggested times — \(formatter.string(from:day)) · \(TimeZone.current.identifier)\n"
            + lines.joined(separator: "\n")
            + "\nPlease confirm availability; based on saved calendar data."
          rows.addArrangedSubview(
            action(
              "Copy suggested times",
              {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(copied, forType: .string)
              }))
        }
        if slots.isEmpty {
          rows.addArrangedSubview(label("No suggestions in the saved calendar window."))
        }
      } catch { if serial == generation { status.stringValue = error.localizedDescription } }
    }
  }
}
private final class AgendaSurface: NSView {
  var onWidthChanged: ((CGFloat)->Void)?
  private var lastWidth:CGFloat = -1
  override func layout(){
    super.layout()
    if abs(bounds.width-lastWidth)>1 {lastWidth=bounds.width;onWidthChanged?(bounds.width)}
  }
  override func draw(_ dirtyRect: NSRect) { EmailTheme.reader.setFill(); bounds.fill() }
}
private final class AgendaDocument: NSView { override var isFlipped: Bool { true } }
@MainActor private final class AgendaButton: NSButton {
  var run: (() -> Void)?
  var prominent = false
  private var hovering = false
  override func updateTrackingAreas() {
    super.updateTrackingAreas();trackingAreas.forEach(removeTrackingArea)
    addTrackingArea(NSTrackingArea(rect:bounds,options:[.activeInKeyWindow,.mouseEnteredAndExited,.inVisibleRect],owner:self,userInfo:nil))
  }
  override func mouseEntered(with event:NSEvent){hovering=true;needsDisplay=true}
  override func mouseExited(with event:NSEvent){hovering=false;needsDisplay=true}
  override func draw(_ dirtyRect:NSRect) {
    if prominent || hovering || isHighlighted {
      (prominent ? EmailTheme.ink.withAlphaComponent(isHighlighted ? 0.16 : 0.1) : EmailTheme.ink.withAlphaComponent(0.06)).setFill()
      NSBezierPath(roundedRect:bounds.insetBy(dx:1,dy:1),xRadius:6,yRadius:6).fill()
    }
    super.draw(dirtyRect)
  }
  @objc func invoke() { run?() }
}

/// A tiny native month map. Date clicks change the visible period without a modal.
@MainActor private final class AgendaMiniMonth:NSView {
  var selectedDate=Date(){didSet{rebuild()}}
  var onDate:((Date)->Void)?
  private var buttons:[AgendaDateButton]=[]
  var onRebuild:(()->Void)?
  var selectedDateTarget:NSView?{buttons.first(where:{$0.chosen})}
  override var isFlipped:Bool{true}
  override var intrinsicContentSize:NSSize{NSSize(width:152,height:172)}
  override var isOpaque:Bool{true}
  override init(frame:NSRect){super.init(frame:frame);setAccessibilityElement(false);rebuild()}
  required init?(coder:NSCoder){fatalError()}
  private func rebuild(){
    buttons.forEach{$0.removeFromSuperview()};buttons=[]
    let c=Calendar.current;let first=c.dateInterval(of:.month,for:selectedDate)!.start;let offset=(c.component(.weekday,from:first)+5)%7
    let start=c.date(byAdding:.day,value:-offset,to:first)!;let full=DateFormatter();full.dateStyle = .full
    for i in 0..<42 {
      let date=c.date(byAdding:.day,value:i,to:start)!
      let button=AgendaDateButton();button.title=String(c.component(.day,from:date));button.isBordered=false;button.font = .systemFont(ofSize:10)
      button.inMonth=c.isDate(date,equalTo:first,toGranularity:.month);button.chosen=c.isDate(date,inSameDayAs:selectedDate);button.today=c.isDateInToday(date)
      button.setAccessibilityLabel(full.string(from:date));button.toolTip=full.string(from:date);button.setAccessibilityValue(button.chosen ? "Selected date" : "")
      button.run={ [weak self] in self?.onDate?(date) };button.target=button;button.action=#selector(AgendaDateButton.choose)
      buttons.append(button);addSubview(button)
    }
    for (i,button) in buttons.enumerated(){button.nextKeyView=buttons[(i+1)%buttons.count]}
    needsLayout=true;needsDisplay=true;onRebuild?()
  }
  override func layout(){super.layout();let w=bounds.width/7;let h=(bounds.height-46)/6;for (i,button) in buttons.enumerated(){button.frame=NSRect(x:CGFloat(i%7)*w,y:46+CGFloat(i/7)*h,width:w,height:h)}}
  override func draw(_ dirtyRect:NSRect){
    EmailTheme.reader.setFill();bounds.fill();let f=DateFormatter();f.dateFormat="MMMM yyyy"
    let p=NSMutableParagraphStyle();p.alignment = .center
    (f.string(from:selectedDate) as NSString).draw(in:NSRect(x:0,y:2,width:bounds.width,height:20),withAttributes:[.font:NSFont.systemFont(ofSize:11,weight:.semibold),.foregroundColor:EmailTheme.ink,.paragraphStyle:p])
    let w=bounds.width/7
    for (i,n) in ["M","T","W","T","F","S","S"].enumerated(){(n as NSString).draw(in:NSRect(x:CGFloat(i)*w,y:28,width:w,height:14),withAttributes:[.font:NSFont.systemFont(ofSize:9),.foregroundColor:EmailTheme.muted,.paragraphStyle:p])}
  }
}
@MainActor private final class AgendaDateButton:NSButton {
  var run:(()->Void)?;var inMonth=true;var chosen=false;var today=false
  override var acceptsFirstResponder:Bool{true}
  override func becomeFirstResponder()->Bool{needsDisplay=true;return true}
  override func resignFirstResponder()->Bool{needsDisplay=true;return true}
  @objc func choose(){run?()}
  override func draw(_ dirtyRect:NSRect){
    EmailTheme.reader.setFill();bounds.fill()
    if chosen || isHighlighted {EmailTheme.ink.withAlphaComponent(0.12).setFill();NSBezierPath(roundedRect:bounds.insetBy(dx:1,dy:1),xRadius:5,yRadius:5).fill()}
    let p=NSMutableParagraphStyle();p.alignment = .center
    (title as NSString).draw(in:NSRect(x:0,y:(bounds.height-14)/2,width:bounds.width,height:14),withAttributes:[.font:NSFont.systemFont(ofSize:10,weight:chosen ? .semibold : .regular),.foregroundColor:inMonth ? EmailTheme.ink : EmailTheme.muted.withAlphaComponent(0.4),.paragraphStyle:p])
    if today{EmailTheme.ink.setFill();NSBezierPath(ovalIn:NSRect(x:bounds.midX-1,y:bounds.maxY-3,width:2,height:2)).fill()}
    if window?.firstResponder === self {EmailTheme.ink.withAlphaComponent(0.7).setStroke();let border=NSBezierPath(roundedRect:bounds.insetBy(dx:1,dy:1),xRadius:5,yRadius:5);border.lineWidth=1;border.stroke()}
  }
}

@MainActor private final class AgendaDateInput:NSDatePicker {
  var onChange:(()->Void)?
  override init(frame:NSRect){super.init(frame:frame);target=self;action=#selector(changed)}
  required init?(coder:NSCoder){fatalError()}
  @objc private func changed(){onChange?()}
}
@MainActor private final class AgendaDestination:NSPopUpButton {
  var onChange:(()->Void)?
  override init(frame:NSRect,pullsDown:Bool){super.init(frame:frame,pullsDown:pullsDown);target=self;action=#selector(changed)}
  required init?(coder:NSCoder){fatalError()}
  @objc private func changed(){onChange?()}
}
