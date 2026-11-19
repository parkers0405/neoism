//! UTC-only schedule picker. `schedule_at` remains the single source of truth;
//! browsing months selects the same day (clamped for shorter months).
use super::{App, Page, edit, format_utc, month_days, parse_schedule_time};
use rune::{Color, El, TextStyle, col, el, row, text};

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June",
    "July", "August", "September", "October", "November", "December",
];
const FUTURE: &str = "Choose a future time in UTC";

#[derive(Clone, Copy, Debug, PartialEq)]
struct Choice { year: u64, month: u64, day: u64, hour: u64, minute: u64 }

impl Choice {
    fn from_seconds(seconds: u64) -> Option<Self> {
        let value = format_utc(seconds);
        let (date, time) = value.split_once(' ')?;
        let mut date = date.split('-');
        let (hour, minute) = time.split_once(':')?;
        Some(Self {
            year: date.next()?.parse().ok()?, month: date.next()?.parse().ok()?,
            day: date.next()?.parse().ok()?, hour: hour.parse().ok()?, minute: minute.parse().ok()?,
        })
    }
    fn value(self) -> String {
        format!("{:04}-{:02}-{:02} {:02}:{:02}", self.year, self.month, self.day, self.hour, self.minute)
    }
    fn seconds(self) -> u64 { parse_schedule_time(&self.value()).expect("valid calendar choice") }
    fn shift_month(self, delta: i64) -> Self {
        let index = (self.year as i64 * 12 + self.month as i64 - 1 + delta)
            .clamp(1970 * 12, 9999 * 12 + 11);
        let (year, month) = (index as u64 / 12, index as u64 % 12 + 1);
        Self { year, month, day: self.day.min(month_days(year)[month as usize - 1]), ..self }
    }
    // Time controls wrap within the chosen date, without surprising date changes.
    fn shift_time(self, hours: bool, delta: i64) -> Self {
        if hours { Self { hour: (self.hour as i64 + delta).rem_euclid(24) as u64, ..self } }
        else { Self { minute: (self.minute as i64 + delta).rem_euclid(60) as u64, ..self } }
    }
    fn grid(self) -> [Option<u64>; 42] {
        let first = Self { day: 1, hour: 0, minute: 0, ..self }.seconds();
        // 1970-01-01 was Thursday. The grid starts on Monday.
        let offset = ((first / 86400 + 3) % 7) as usize;
        let days = month_days(self.year)[self.month as usize - 1];
        std::array::from_fn(|i| {
            if i >= offset && i < offset + days as usize { Some((i - offset + 1) as u64) }
            else { None }
        })
    }
}

fn choice(app: &App) -> Choice {
    parse_schedule_time(&app.schedule_at.text).ok().and_then(Choice::from_seconds)
        .or_else(|| Choice::from_seconds(app.now.saturating_add(3600)))
        .expect("current clock is within supported calendar range")
}
fn store(app: &mut App, choice: Choice) {
    app.schedule_at = edit(&choice.value());
    if app.error.as_deref() == Some(FUTURE) { app.error = None; }
}
fn confirm(app: &mut App) {
    let selected = choice(app);
    if selected.seconds() <= app.now { app.error = Some(FUTURE.into()); return; }
    store(app, selected);
    app.show_schedule = true;
    app.dialog = None;
}
fn pending(app: &mut App) {
    app.dialog = None;
    app.navigate(Page::Scheduled);
}

fn gray(v: u8) -> Color {
    let s = v as f32 / 255.;
    let c = if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) };
    Color { r: c, g: c, b: c, a: 1. }
}
fn label(s: impl Into<String>) -> El {
    text(s.into()).font(TextStyle::new(12., 400.)).color(gray(220))
}
fn button(key: impl Into<String>, s: impl Into<String>, action: impl Fn(&mut App) + 'static) -> El {
    el().key(key.into()).h(28.).px(8.).radius(7.).hover(gray(57)).center()
        .child(label(s)).on_press(action)
}
fn time_control(selected: Choice, hours: bool) -> El {
    let (name, value) = if hours { ("Hour", selected.hour) } else { ("Minute", selected.minute) };
    col().gap(3.).child(label(name).color(gray(144)))
        .child(row().gap(3.).items_center().fill(gray(0)).radius(8.)
            .child(button(format!("calendar-{name}-minus"), "−", move |a| store(a, choice(a).shift_time(hours, -1))))
            .child(label(format!("{value:02}")).w(26.))
            .child(button(format!("calendar-{name}-plus"), "+", move |a| store(a, choice(a).shift_time(hours, 1)))))
}

pub(super) fn picker(app: &App, panel: El) -> El {
    let selected = choice(app);
    let today = Choice::from_seconds(app.now);
    let mut grid = col().key("calendar-grid").w_full().gap(2.);
    let mut weekdays = row().w_full().gap(2.);
    for day in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"] {
        weekdays = weekdays.child(el().grow(1.).w(0.).h(20.).center().child(label(day).color(gray(144))));
    }
    grid = grid.child(weekdays);
    for week in selected.grid().chunks(7) {
        let mut line = row().w_full().gap(2.);
        for day in week {
            let mut cell = el().grow(1.).w(0.).h(28.).radius(7.).center();
            if let Some(day) = *day {
                let date = Choice { day, ..selected };
                let past = date.seconds() / 86400 < app.now / 86400;
                let is_today = today.is_some_and(|t| (t.year, t.month, t.day) == (date.year, date.month, day));
                cell = cell.key(format!("calendar-day-{day}"))
                    .fill(gray(if day == selected.day { 72 } else { 0 })).hover(gray(58))
                    .border(1., gray(if is_today { 120 } else { 29 }))
                    .child(label(day.to_string()).color(gray(if past { 115 } else { 235 })))
                    .on_press(move |a: &mut App| store(a, date));
            }
            line = line.child(cell);
        }
        grid = grid.child(line);
    }
    panel.child(col().w_full().gap(8.)
        .child(label("Choose a date and time in UTC, then send from the composer.").color(gray(160)).wrap())
        .child(row().w_full().items_center()
            .child(button("calendar-prev", "‹", |a| store(a, choice(a).shift_month(-1))))
            .child(el().grow(1.).center().child(label(format!("{} {}", MONTHS[selected.month as usize - 1], selected.year)).font(TextStyle::new(14., 600.))))
            .child(button("calendar-next", "›", |a| store(a, choice(a).shift_month(1)))))
        .child(grid)
        .child(row().w_full().gap(12.).items_center()
            .child(time_control(selected, true)).child(time_control(selected, false))
            .child(label("UTC · 24-hour").color(gray(144))))
        .child(row().gap(4.)
            .child(button("calendar-ten-minutes", "In 10 min", |a| {
                if let Some(c) = Choice::from_seconds(a.now.saturating_add(600)) { store(a, c); }
            }).fill(gray(0)).border(1.,gray(55)))
            .child(button("calendar-one-hour", "In 1 hour", |a| {
                if let Some(c) = Choice::from_seconds(a.now.saturating_add(3600)) { store(a, c); }
            }).fill(gray(0)).border(1.,gray(55))))
        .child(label(format!("{} UTC", selected.value())))
        .child(label(if selected.seconds() <= app.now { FUTURE } else { "Time zone: UTC, not local time" }).color(gray(160)))
        .child(row().w_full().items_center()
            .child(button("calendar-immediate", "Send immediately", |a| { a.show_schedule = false; a.dialog = None; }))
            .child(el().grow(1.))
            .child(button("calendar-confirm", "Use time", confirm).fill(gray(0)).border(1.,gray(55))))
        .child(button("calendar-pending", format!("Pending scheduled posts ({})  ›", app.schedules.iter().filter(|s|crate::projects::contains(app.project,&s.channel)).count()), pending).w_full().fill(gray(0)).border(1.,gray(55))))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(value: &str) -> Choice { Choice::from_seconds(parse_schedule_time(value).unwrap()).unwrap() }

    #[test]
    fn real_grid_handles_weekdays_and_leap_centuries() {
        let leap = at("2024-02-01 00:00").grid();
        assert_eq!(leap[3], Some(1)); // Thursday
        assert_eq!(leap.iter().flatten().count(), 29);
        assert_eq!(leap[31], Some(29));
        assert_eq!(at("2100-02-01 00:00").grid().iter().flatten().count(), 28);
        assert_eq!(at("2000-02-01 00:00").grid().iter().flatten().count(), 29);
        let six_weeks = at("2026-03-01 00:00").grid();
        assert_eq!(six_weeks[6], Some(1));
        assert_eq!(six_weeks[36], Some(31));
        assert_eq!(at("1970-01-01 00:00").grid()[3], Some(1));
    }

    #[test]
    fn month_navigation_clamps_day_and_calendar_boundaries() {
        assert_eq!(at("2024-01-31 23:59").shift_month(1).value(), "2024-02-29 23:59");
        assert_eq!(at("2025-03-31 12:15").shift_month(-1).value(), "2025-02-28 12:15");
        assert_eq!(at("2025-12-31 12:15").shift_month(1).value(), "2026-01-31 12:15");
        assert_eq!(at("2025-01-31 12:15").shift_month(-1).value(), "2024-12-31 12:15");
        assert_eq!(at("1970-01-01 00:00").shift_month(-1).year, 1970);
        assert_eq!(at("9999-12-31 23:59").shift_month(1).value(), "9999-12-31 23:59");
    }

    #[test]
    fn time_controls_wrap_without_changing_date() {
        let c = at("2024-02-29 23:59");
        assert_eq!(c.shift_time(true, 1).value(), "2024-02-29 00:59");
        assert_eq!(c.shift_time(false, 1).value(), "2024-02-29 23:00");
        assert_eq!(at("2024-02-29 00:00").shift_time(false, -1).minute, 59);
        assert_eq!(at("2024-02-29 00:00").shift_time(true, -1).hour, 23);
    }

    #[test]
    fn confirmation_validates_future_and_normalizes_legacy_seconds() {
        let mut app = App::new();
        app.now = parse_schedule_time("2025-01-01 12:00").unwrap();
        app.dialog = Some(super::super::Dialog::Schedule);
        app.schedule_at = edit("2025-01-01 12:00");
        confirm(&mut app);
        assert!(!app.show_schedule);
        assert!(app.dialog.is_some());
        assert_eq!(app.error.as_deref(), Some(FUTURE));
        app.schedule_at = edit(&(app.now + 3600).to_string());
        confirm(&mut app);
        assert!(app.show_schedule);
        assert!(app.dialog.is_none());
        assert!(app.error.is_none());
        assert_eq!(app.schedule_at.text, "2025-01-01 13:00");
    }

    #[test]
    fn invalid_or_empty_input_gets_a_visible_future_default() {
        let mut app = App::new();
        app.now = parse_schedule_time("2025-12-31 23:30").unwrap();
        for value in ["", "bad", "18446744073709551615"] {
            app.schedule_at = edit(value);
            assert_eq!(choice(&app).value(), "2026-01-01 00:30");
        }
    }

    #[test]
    fn pending_link_keeps_draft_and_opens_existing_list() {
        let mut app = App::new();
        app.dialog = Some(super::super::Dialog::Schedule);
        app.draft = edit("Keep this draft");
        pending(&mut app);
        assert!(app.dialog.is_none());
        assert!(app.page == Page::Scheduled);
        assert_eq!(app.draft.text, "Keep this draft");
    }

    #[test]
    fn calendar_mounts_in_real_dialog() {
        let mut app = App::new();
        super::super::fixture::load(&mut app, true);
        app.dialog = Some(super::super::Dialog::Schedule);
        let mut rt = rune::Runtime::new(app, super::super::view::root, (700., 674.), rune::Env::new(true, 1.));
        for _ in 0..5 { rt.frame(1. / 60.); }
        for key in ["calendar-grid", "calendar-day-1", "calendar-prev", "calendar-next", "calendar-Hour-plus", "calendar-Minute-minus", "calendar-confirm", "calendar-pending"] {
            assert!(rt.find(key).is_some(), "missing {key}");
        }
        let click = |rt: &mut rune::Runtime<App>, key: &str| {
            let (x, y) = rt.find(key).unwrap();
            assert!((0. ..700.).contains(&x) && (0. ..674.).contains(&y), "offscreen {key}: {x}, {y}");
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            for _ in 0..5 { rt.frame(1. / 60.); }
        };
        let original = choice(&rt.state);
        click(&mut rt, "calendar-next");
        assert_eq!(choice(&rt.state), original.shift_month(1));
        click(&mut rt, "calendar-day-1");
        assert_eq!(choice(&rt.state).day, 1);
        let hour = choice(&rt.state).hour;
        click(&mut rt, "calendar-Hour-plus");
        assert_eq!(choice(&rt.state).hour, (hour + 1) % 24);
        click(&mut rt, "calendar-pending");
        assert!(rt.state.dialog.is_none());
        assert!(rt.state.page == Page::Scheduled);
    }
}