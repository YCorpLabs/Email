//! Local, account-scoped composer suggestions. The popover never takes keyboard
//! focus from its entry; accepting only replaces the segment at the caret.
use super::*;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq)]
struct Context {
    start: usize,
    end: usize,
    query: String,
}
fn segments(text: &str) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    let (mut start, mut quoted, mut escaped, mut angle) = (0, false, false, false);
    for (i, c) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && quoted {
            escaped = true;
            continue;
        }
        if c == '"' {
            quoted = !quoted;
            continue;
        }
        if !quoted {
            if c == '<' {
                angle = true;
            }
            if c == '>' {
                angle = false;
            }
            if !angle && matches!(c, ',' | ';') {
                result.push((start, i));
                start = i + c.len_utf8();
            }
        }
    }
    result.push((start, text.len()));
    result
}
fn context(text: &str, caret_chars: i32) -> Option<Context> {
    let caret = text
        .char_indices()
        .nth(caret_chars.max(0) as usize)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    let (start, end) = segments(text)
        .into_iter()
        .find(|(a, b)| caret >= *a && caret <= *b)?;
    let part = text[start..end].trim();
    let query = part
        .rsplit_once('<')
        .map(|(_, s)| s.trim_matches(|c| c == '>' || c == ' '))
        .unwrap_or_else(|| part.trim_matches(|c| c == '"' || c == ' '));
    if query.is_empty() || query.len() > 128 {
        return None;
    }
    Some(Context {
        start,
        end,
        query: query.to_string(),
    })
}
fn inserting(text: &str, context: &Context, formatted: &str) -> Option<(String, i32)> {
    if context.end > text.len()
        || !text.is_char_boundary(context.start)
        || !text.is_char_boundary(context.end)
        || formatted.contains(['\r', '\n'])
    {
        return None;
    }
    let replacement = format!("{}{formatted}", if context.start == 0 { "" } else { " " });
    let result = format!(
        "{}{}{}",
        &text[..context.start],
        replacement,
        &text[context.end..]
    );
    let caret = (text[..context.start].chars().count() + replacement.chars().count()) as i32;
    Some((result, caret))
}
#[derive(Clone)]
struct Suggestion {
    email: String,
    name: String,
    formatted: String,
}
struct EntryState {
    popover: gtk::Popover,
    list: gtk::Box,
    items: RefCell<Vec<Suggestion>>,
    context: RefCell<Option<Context>>,
    selected: Cell<usize>,
    buttons: RefCell<Vec<gtk::Button>>,
    generation: Cell<u64>,
}
pub(super) struct Group {
    entries: [gtk::Entry; 3],
    account: gtk::ComboBoxText,
    path: PathBuf,
    states: Vec<EntryState>,
}
impl Drop for Group {
    fn drop(&mut self) {
        for state in &self.states {
            if state.popover.parent().is_some() {
                state.popover.unparent();
            }
        }
    }
}
impl Group {
    pub(super) fn dismiss(&self) -> bool {
        let mut was_open = false;
        for state in &self.states {
            was_open |= state.popover.is_visible();
            state.generation.set(state.generation.get().wrapping_add(1));
            state.popover.popdown();
        }
        was_open
    }
    fn account_id(&self) -> Option<String> {
        self.account
            .active_id()
            .map(|id| id.split('\t').next().unwrap_or_default().to_string())
    }
    fn accept(&self, index: usize) {
        let entry = &self.entries[index];
        let state = &self.states[index];
        let selected = state.items.borrow().get(state.selected.get()).cloned();
        let context = state.context.borrow().clone();
        if let (Some(item), Some(context)) = (selected, context) {
            if self::context(entry.text().as_str(), entry.position()).as_ref() != Some(&context) {
                self.dismiss();
                return;
            }
            if let Some((_text, caret)) =
                inserting(entry.text().as_str(), &context, &item.formatted)
            {
                self.dismiss();
                let start = entry.text()[..context.start].chars().count() as i32;
                let end = entry.text()[..context.end].chars().count() as i32;
                let replacement = format!(
                    "{}{}",
                    if context.start == 0 { "" } else { " " },
                    item.formatted
                );
                entry.delete_text(start, end);
                let mut position = start;
                entry.insert_text(&replacement, &mut position);
                entry.set_position(caret);
                self.dismiss();
            }
        }
    }
    fn caret_changed(&self, index: usize) {
        let state = &self.states[index];
        let entry = &self.entries[index];
        if state.popover.is_visible()
            && (entry.selection_bounds().is_some()
                || context(entry.text().as_str(), entry.position()) != *state.context.borrow())
        {
            self.dismiss();
        }
    }
    fn highlight(&self, index: usize) {
        let state = &self.states[index];
        for (i, b) in state.buttons.borrow().iter().enumerate() {
            if i == state.selected.get() {
                b.add_css_class("recipient-selected");
            } else {
                b.remove_css_class("recipient-selected");
            }
        }
    }
    fn changed(self: &Rc<Self>, index: usize) {
        self.dismiss();
        let entry = &self.entries[index];
        if !focus_within(entry) || entry.selection_bounds().is_some() {
            return;
        }
        let Some(ctx) = context(entry.text().as_str(), entry.position()) else {
            return;
        };
        let Some(account) = self.account_id() else {
            return;
        };
        let text = entry.text().to_string();
        let version = self.states[index].generation.get();
        *self.states[index].context.borrow_mut() = Some(ctx.clone());
        let group = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
            let Some(group) = group.upgrade() else {
                return;
            };
            if group.states[index].generation.get() != version
                || !focus_within(&group.entries[index])
            {
                return;
            }
            let path = group.path.clone();
            let query = ctx.query.clone();
            let requested_account = account.clone();
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let result = MailService::open(&path).map_err(|e|e.to_string()).and_then(|s|s.request_json(&json!({"op":"suggest_recipients","account_id":requested_account,"query":query,"limit":6}).to_string()).map_err(|e|e.to_string()));
                let _ = tx.send(result);
            });
            let weak = Rc::downgrade(&group);
            glib::timeout_add_local(std::time::Duration::from_millis(20), move || {
                let Some(group) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let state = &group.states[index];
                let entry = &group.entries[index];
                if state.generation.get() != version
                    || entry.text().as_str() != text
                    || !focus_within(entry)
                    || context(entry.text().as_str(), entry.position()).as_ref() != Some(&ctx)
                    || group.account_id().as_deref() != Some(account.as_str())
                {
                    return glib::ControlFlow::Break;
                }
                match rx.try_recv() {
                    Ok(Ok(value)) => {
                        group.show(index, &ctx, value);
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    _ => glib::ControlFlow::Break,
                }
            });
        });
    }
    fn show(self: &Rc<Self>, index: usize, ctx: &Context, value: Value) {
        let state = &self.states[index];
        let mut excluded = HashSet::new();
        for (i, entry) in self.entries.iter().enumerate() {
            let text = entry.text();
            for (start, end) in segments(text.as_str()) {
                if i == index && start == ctx.start && end == ctx.end {
                    continue;
                }
                excluded.insert(parity::address(text[start..end].trim()).to_lowercase());
            }
        }
        let items: Vec<_> = value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let email = item["email"].as_str()?;
                let formatted = item["formatted"].as_str()?;
                if mail_core::parse_outgoing_recipients(formatted).ok()?.len() != 1 {
                    return None;
                }
                if excluded.contains(&email.to_lowercase()) || formatted.contains(['\r', '\n']) {
                    return None;
                }
                Some(Suggestion {
                    email: email.into(),
                    name: item["display_name"].as_str().unwrap_or_default().into(),
                    formatted: formatted.into(),
                })
            })
            .collect();
        while let Some(child) = state.list.first_child() {
            state.list.remove(&child);
        }
        state.buttons.borrow_mut().clear();
        *state.items.borrow_mut() = items;
        if state.items.borrow().is_empty() {
            state.popover.popdown();
            return;
        }
        state.selected.set(0);
        for (row, item) in state.items.borrow().iter().enumerate() {
            let button = gtk::Button::new();
            button.set_focusable(false);
            button.add_css_class("recipient-suggestion");
            let labels = gtk::Box::new(Orientation::Vertical, 2);
            let name = gtk::Label::new(Some(if item.name.is_empty() {
                &item.email
            } else {
                &item.name
            }));
            name.set_xalign(0.0);
            name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            labels.append(&name);
            if !item.name.is_empty() {
                let address = gtk::Label::new(Some(&item.email));
                address.set_xalign(0.0);
                address.add_css_class("dim-label");
                address.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                labels.append(&address);
            }
            button.set_child(Some(&labels));
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(group) = weak.upgrade() {
                    group.states[index].selected.set(row);
                    group.accept(index);
                }
            });
            state.list.append(&button);
            state.buttons.borrow_mut().push(button);
        }
        let hint = gtk::Label::new(Some(
            "Recent recipients · ↑↓ choose · ↵ add · Tab add & next · Esc dismiss",
        ));
        hint.add_css_class("recipient-hint");
        hint.set_wrap(true);
        state.list.append(&hint);
        self.highlight(index);
        state.popover.popup();
    }
}
pub(super) fn install(
    ui: &Rc<Ui>,
    account: &gtk::ComboBoxText,
    entries: [gtk::Entry; 3],
) -> Rc<Group> {
    let states = entries
        .iter()
        .map(|entry| {
            let popover = gtk::Popover::new();
            popover.set_autohide(false);
            popover.set_has_arrow(false);
            popover.set_position(gtk::PositionType::Bottom);
            popover.set_focusable(false);
            popover.set_parent(entry);
            popover.add_css_class("recipient-popover");
            let list = gtk::Box::new(Orientation::Vertical, 0);
            list.set_size_request(300, -1);
            popover.set_child(Some(&list));
            EntryState {
                popover,
                list,
                items: RefCell::new(Vec::new()),
                context: RefCell::new(None),
                selected: Cell::new(0),
                buttons: RefCell::new(Vec::new()),
                generation: Cell::new(0),
            }
        })
        .collect();
    let group = Rc::new(Group {
        entries,
        account: account.clone(),
        path: ui.database_path.clone(),
        states,
    });
    for (index, entry) in group.entries.iter().enumerate() {
        entry.set_enable_undo(true);
        let weak = Rc::downgrade(&group);
        entry.connect_cursor_position_notify(move |_| {
            if let Some(group) = weak.upgrade() {
                group.caret_changed(index);
            }
        });
        let weak = Rc::downgrade(&group);
        entry.connect_selection_bound_notify(move |_| {
            if let Some(group) = weak.upgrade() {
                group.caret_changed(index);
            }
        });
        let weak = Rc::downgrade(&group);
        entry.connect_changed(move |_| {
            if let Some(group) = weak.upgrade() {
                group.changed(index);
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(&group);
        focus.connect_leave(move |_| {
            if let Some(group) = weak.upgrade() {
                group.dismiss();
            }
        });
        entry.add_controller(focus);
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&group);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(group) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let state = &group.states[index];
            let count = state.items.borrow().len();
            if !state.popover.is_visible()
                || count == 0
                || mods.intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::SUPER_MASK,
                )
            {
                return glib::Propagation::Proceed;
            }
            match key {
                gdk::Key::Down => {
                    state.selected.set((state.selected.get() + 1) % count);
                    group.highlight(index);
                    glib::Propagation::Stop
                }
                gdk::Key::Up => {
                    state
                        .selected
                        .set((state.selected.get() + count - 1) % count);
                    group.highlight(index);
                    glib::Propagation::Stop
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    group.accept(index);
                    glib::Propagation::Stop
                }
                gdk::Key::Tab if !mods.contains(gdk::ModifierType::SHIFT_MASK) => {
                    group.accept(index);
                    glib::Propagation::Proceed
                }
                gdk::Key::Escape => {
                    group.dismiss();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        entry.add_controller(keys);
    }
    let weak = Rc::downgrade(&group);
    account.connect_changed(move |_| {
        if let Some(group) = weak.upgrade() {
            group.dismiss();
        }
    });
    group
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_commas_and_unicode_midlist_replacement() {
        let text = "\"Ødegård, Torjus\" <torjus@example.com>, ma; Last <last@example.com>";
        let caret = text[..text.find("ma;").unwrap() + 2].chars().count() as i32;
        let ctx = context(text, caret).unwrap();
        assert_eq!(ctx.query, "ma");
        let (result, _) = inserting(text, &ctx, "Maya <maya@example.com>").unwrap();
        assert_eq!(
            result,
            "\"Ødegård, Torjus\" <torjus@example.com>, Maya <maya@example.com>; Last <last@example.com>"
        );
    }
    #[test]
    fn incomplete_address_and_empty_segment() {
        assert_eq!(context("Alice <al", 9).unwrap().query, "al");
        assert!(context("a@example.com, ", 15).is_none());
        assert_eq!(context("\"Smith, Al", 10).unwrap().query, "Smith, Al");
    }
}
