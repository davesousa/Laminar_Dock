//! How each widget face draws, at every size. Coordinates are logical pixels inside the
//! widget. Five shapes:
//!   Mini   — the dock at one icon wide: one visual and a short value
//!   Dock   — the dock, wider: visual, value and caption side by side
//!   Small  — half a Launchpad column
//!   Medium — a full column, one row tall (the reference layout)
//!   Large  — a full column, two rows tall: the medium layout plus details
//! Most faces describe themselves once as a `Sum` (visual, value, caption, chart, meter)
//! and share the Mini/Dock/Small layouts; faces with their own look draw every shape.

use std::borrow::Cow;

use laminar_sys::feed::{Feed, FeedError};
use windows::Win32::Graphics::Imaging::{IWICBitmapSource, IWICImagingFactory};

use super::canvas::{alpha, hex, Align, Badge, Canvas, ChartOpts, Col, Font};
use super::style::{self, Type};
use super::catalog::*;
use super::{compact, dur, headline, hms, money, Ctx, Painted, WAct, DAYS, MONTHS};

const BLUE: Col = hex(0x0A84FF);
const GREEN: Col = hex(0x30D158);
const RED: Col = hex(0xFF453A);
const ORANGE: Col = hex(0xFF9F0A);
const PURPLE: Col = hex(0xBF5AF2);
const YELLOW: Col = hex(0xFFD60A);
const CLAUDE: Col = hex(0xD97757);
const WHITE: Col = (1.0, 1.0, 1.0, 1.0);

// Segoe Fluent Icons
const G_PLAY: &str = "\u{E768}";
const G_PAUSE: &str = "\u{E769}";
const G_PREV: &str = "\u{E892}";
const G_NEXT: &str = "\u{E893}";
const G_RESET: &str = "\u{E72C}";
const G_WIFI: &str = "\u{E701}";
const G_BT: &str = "\u{E702}";
const G_MOON: &str = "\u{E708}";
const G_SUN: &str = "\u{E706}";
const G_VOL: &str = "\u{E767}";
const G_MUTE: &str = "\u{E74F}";
const G_MIC: &str = "\u{E720}";
const G_UP: &str = "\u{E70E}";
const G_BELL: &str = "\u{EA8F}";
const G_DESKTOP: &str = "\u{E7F4}";
const G_SEARCH: &str = "\u{E721}";
const G_RUN: &str = "\u{E756}";
const G_TRASH: &str = "\u{E74D}";
const G_NOTE: &str = "\u{E70B}";
const G_CHAT: &str = "\u{E8BD}";
const G_KEYBOARD: &str = "\u{E765}";
const G_MUSIC: &str = "\u{E8D6}";
const G_CLOCK: &str = "\u{E823}";
const G_PLUG: &str = "\u{E83E}";
const G_COFFEE: &str = "\u{EC32}";
const G_DND: &str = "\u{E7ED}";
const G_SETTINGS: &str = "\u{E713}";
const G_UPDATE: &str = "\u{E895}";
const G_SNIP: &str = "\u{E7C5}";
const G_LOCK: &str = "\u{E72E}";
const G_CHEV: &str = "\u{E76C}";
const G_DRIVE: &str = "\u{EDA2}";
const G_CAL: &str = "\u{E787}";
const G_SCREEN: &str = "\u{E7F4}";
const G_IMAGE: &str = "\u{E91B}";
const G_AI: &str = "\u{E99A}";
const G_WARN: &str = "\u{E7BA}";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Shape {
    Mini,
    Dock,
    Small,
    Medium,
    Large,
}

fn shape_of(c: &Canvas, dense: bool) -> Shape {
    if dense {
        if c.w < c.h * 1.5 {
            Shape::Mini
        } else {
            Shape::Dock
        }
    } else if c.w < 180.0 {
        Shape::Small
    } else if c.h > 150.0 {
        Shape::Large
    } else {
        Shape::Medium
    }
}

// ------------------------------------------------------------------ shared pieces

/// Data for a connector face: its feed, or why there isn't one.
enum Got<'a> {
    Feed(Cow<'a, Feed>),
    Setup(String),
    Loading,
    Error(String),
}

/// Widget sheet only (`state=setup|error`): force every connector face into its §7 setup
/// (1) or error (2) state so those layouts can be reviewed. 0 = off (always in the app).
pub(super) static SHEET_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn got<'a>(x: &'a Ctx, conn: &str) -> Got<'a> {
    match SHEET_STATE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => return Got::Setup("Add your API key in Settings.".into()),
        2 => return Got::Error("Couldn't reach the service.".into()),
        _ => {}
    }
    match (x.hub.feed(conn), x.hub.err(conn)) {
        (Some(f), None) => Got::Feed(Cow::Borrowed(f)),
        _ if x.sample => Got::Feed(Cow::Owned(super::hub::sample(conn))),
        (_, Some(FeedError::NeedsSetup(m))) | (_, Some(FeedError::Unauthorized(m))) => Got::Setup(m.clone()),
        (Some(f), Some(_)) => Got::Feed(Cow::Borrowed(f)),
        (None, Some(FeedError::Failed(m))) => Got::Error(m.clone()),
        (None, None) => Got::Loading,
    }
}

/// The card: fill and a 1 px inner edge, with the dock or Launchpad corner radius.
fn bg(c: &Canvas, dense: bool) {
    let r = if dense { style::DOCK_R } else { style::CARD_R };
    c.rr(0.0, 0.0, c.w, c.h, r, c.card());
    c.rr_stroke(0.0, 0.0, c.w, c.h, r, c.card_edge(), 1.0);
}

fn up_down(d: f64) -> Col {
    if d >= 0.0 {
        GREEN
    } else {
        RED
    }
}

// ------------------------------------------------------------------ formats (§2.2)

/// A change figure: "+12.4%" / "−3.1%" (U+2212 minus), no triangles.
fn fmt_delta_text(d: f64) -> String {
    if d.abs() < 0.05 {
        return "0.0%".into();
    }
    format!("{}{:.1}%", if d > 0.0 { "+" } else { "\u{2212}" }, d.abs())
}

/// A change figure and its colour: green up, red down, text2 for no change. Only use the
/// colour where "up" is a judgment (money, prices, followers).
fn fmt_delta(c: &Canvas, d: f64) -> (String, Col) {
    (fmt_delta_text(d), if d.abs() < 0.05 { c.text2() } else { up_down(d) })
}

/// "1.9M", "412K", "12.4K", "1,234".
#[allow(dead_code)]
fn fmt_compact(v: f64) -> String {
    compact(v)
}

/// The tightest form of a number: "5.3K", "17.5K", "412K", "1.9M", "83".
fn compact_tight(v: f64) -> String {
    let a = v.abs();
    let (n, unit) = if a >= 1e9 {
        (v / 1e9, "B")
    } else if a >= 1e6 {
        (v / 1e6, "M")
    } else if a >= 1e3 {
        (v / 1e3, "K")
    } else {
        return format!("{}", v.round() as i64);
    };
    let s = if n.abs() >= 100.0 { format!("{n:.0}") } else { format!("{n:.1}") };
    format!("{}{unit}", s.strip_suffix(".0").unwrap_or(&s))
}

/// The compact form of a formatted value ("$5,336" → "$5.3K", "$83.46" → "$83"), keeping
/// its prefix and suffix. None when there is no shorter form. Only money may lose its
/// decimals under 1,000: "+0.6%" or "8.4" has no compact form (rounding would change it).
fn compact_str(s: &str) -> Option<String> {
    let start = s.find(|ch: char| ch.is_ascii_digit())?;
    let end = s[start..].find(|ch: char| !(ch.is_ascii_digit() || ch == ',' || ch == '.')).map(|e| start + e).unwrap_or(s.len());
    let num = &s[start..end];
    let v: f64 = num.replace(',', "").parse().ok()?;
    let is_money = s[..start].contains(['$', '\u{20AC}', '\u{00A3}', '\u{00A5}']);
    if v.abs() < 1000.0 && num.contains('.') && !is_money {
        return None;
    }
    let short = compact_tight(v);
    (short.len() < num.len()).then(|| format!("{}{short}{}", &s[..start], &s[end..]))
}

/// Money (§2.2): cents under $100, "$5,336" to $9,999, "$12.4K" from $10,000; shortened
/// further ("$5.3K") when it has more than `budget` characters.
#[allow(dead_code)]
fn fmt_money(v: f64, currency: Option<&str>, budget: usize) -> String {
    let full = money(v, currency);
    if full.chars().count() <= budget {
        return full;
    }
    compact_str(&full).unwrap_or(full)
}

/// A key value that must never be cut: the full string if it fits `max_w` at the role's
/// size, else its compact form, else that shrunk (down to the role's minimum).
/// Draw the result with a generous `max_w` so it is never ellipsized.
fn fit_value(c: &Canvas, s: &str, role: Type, max_w: f32) -> (String, f32) {
    let w = c.measure(s, role.size, role.weight, Font::Text).0;
    if w <= max_w || w <= 0.0 {
        return (s.to_string(), role.size);
    }
    let s2 = compact_str(s).unwrap_or_else(|| s.to_string());
    let w2 = c.measure(&s2, role.size, role.weight, Font::Text).0;
    if w2 <= max_w {
        return (s2, role.size);
    }
    let mut size = (role.size * max_w / w2).max(role.min);
    // Width isn't quite linear in size (hinting, optical sizes): step down until it fits.
    while size > role.min && c.measure(&s2, size, role.weight, Font::Text).0 > max_w {
        size = (size - 0.5).max(role.min);
    }
    (s2, size)
}

/// The largest size (from `start` down to `min`) at which `s` fits in `max_w`.
fn fit(c: &Canvas, s: &str, start: f32, min: f32, weight: u32, font: Font, max_w: f32) -> f32 {
    let w = c.measure(s, start, weight, font).0;
    if w <= max_w || w <= 0.0 {
        return start;
    }
    (start * max_w / w).max(min)
}

fn open_hit(out: &mut Painted, c: &Canvas, open: &str) {
    if !open.is_empty() {
        out.hits.insert(0, ((0.0, 0.0, c.w, c.h), WAct::Open(open.to_string())));
    }
}

// ------------------------------------------------------------------ template helpers (§3)
// Family layouts build on these so margins, type and spacing stay the same everywhere.

/// The third line of a Glance block: a footnote, or a meter.
#[allow(dead_code)]
enum Line {
    Text(String),
    Meter(f32, Col),
}

/// Header template (Medium/Large): badge 20 and title on one row, an optional trailing
/// value at the right. Returns where content starts (y 44).
#[allow(dead_code)]
fn header(c: &Canvas, badge: &Badge, title: &str, trailing: Option<(&str, Col)>) -> f32 {
    let m = style::M_CARD;
    let bs = style::BADGE_HEADER;
    c.badge(badge, m, m, bs);
    let mut right = c.w - m;
    if let Some((t, col)) = trailing {
        let (v, fs) = fit_value(c, t, style::VALUE, (c.w - 2.0 * m) * 0.5);
        let w = c.measure(&v, fs, style::VALUE.weight, Font::Text).0;
        c.text_base(&v, c.w - m, style::HEADER_BASE, fs, style::VALUE.weight, col, Align::Right, c.w);
        right -= w + style::SP[3];
    }
    let tx = m + bs + style::SP[3];
    c.text_base(title, tx, style::HEADER_BASE, style::TITLE.size, style::TITLE.weight, c.text1(), Align::Left, right - tx);
    style::CONTENT_TOP
}

/// Glance template (Medium, and the top of Large): hero value, caption, optional third
/// line or meter, badge 24 top-right.
#[allow(dead_code)]
fn glance(c: &Canvas, badge: &Badge, value: &str, caption: &str, third: Option<Line>) {
    let m = style::M_CARD;
    let bs = style::BADGE_GLANCE;
    c.badge(badge, c.w - m - bs, m, bs);
    let (v, fs) = fit_value(c, value, style::HERO, c.w - 2.0 * m - bs - style::SP[3]);
    c.text_cap(&v, m, m, fs, style::HERO.weight, c.text1(), Align::Left, c.w * 2.0);
    c.text_base(caption, m, 58.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, c.w - 2.0 * m);
    match third {
        Some(Line::Text(t)) => {
            c.text_base(&t, m, 76.0, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, c.w - 2.0 * m);
        }
        Some(Line::Meter(f, col)) => c.bar(m, 70.0, c.w - 2.0 * m, style::METER_H, f, col),
        None => {}
    }
}

/// A hairline across the card at `y`, inside the margins.
fn separator(c: &Canvas, y: f32) {
    c.line(style::M_CARD, y + 0.5, c.w - style::M_CARD, y + 0.5, 1.0, c.sep());
}

/// The Large detail region under a Glance block: (x, y, w, h), inside the margins.
fn detail_region(c: &Canvas) -> (f32, f32, f32, f32) {
    let m = style::M_CARD;
    (m, style::DETAIL_TOP, c.w - 2.0 * m, c.h - m - style::DETAIL_TOP)
}

/// List rows from `top`, one per `pitch`, at most `max`: label at the left, trailing
/// value right-aligned. Returns the bottom of the last row.
fn rows(c: &Canvas, items: &[(&str, &str)], top: f32, pitch: f32, max: usize) -> f32 {
    let small = c.w < 180.0;
    let m = if small { style::M_SMALL } else { style::M_CARD };
    let ls = if small { style::FOOT.size } else { style::BODY.size };
    let inner = c.w - 2.0 * m;
    let n = items.len().min(max);
    for (k, (label, value)) in items.iter().take(n).enumerate() {
        let base = top + k as f32 * pitch + (pitch + c.cap_h(ls)) / 2.0;
        let mut vw = 0.0;
        if !value.is_empty() {
            // The value gets what the label leaves, but never squeezes the label under 45%.
            let lw = c.measure(label, ls, style::BODY.weight, Font::Text).0;
            let vmax = (inner - lw - style::SP[3]).max(inner * 0.45);
            vw = c.measure(value, style::FOOT.size, 500, Font::Text).0.min(vmax) + style::SP[3];
            c.text_base(value, c.w - m, base, style::FOOT.size, 500, c.text2(), Align::Right, vmax);
        }
        c.text_base(label, m, base, ls, style::BODY.weight, c.text1(), Align::Left, inner - vw);
    }
    top + n as f32 * pitch
}

/// Three equal columns of value (17/600) over a footnote label, from `top` (cap top).
#[allow(dead_code)]
fn stat_footer(c: &Canvas, items: [(&str, &str); 3], top: f32) {
    let m = style::M_CARD;
    let cw = (c.w - 2.0 * m) / 3.0;
    let base = top + c.cap_h(style::VALUE.size);
    for (i, (v, label)) in items.iter().enumerate() {
        let x = m + i as f32 * cw;
        let (v, fs) = fit_value(c, v, style::VALUE, cw - style::SP[3]);
        c.text_base(&v, x, base, fs, style::VALUE.weight, c.text1(), Align::Left, c.w);
        c.text_base(label, x, base + style::SP[6], style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, cw - style::SP[3]);
    }
}

/// A loading placeholder bar.
fn skeleton(c: &Canvas, x: f32, y: f32, w: f32, h: f32) {
    c.rr(x, y, w, h, (h / 2.0).min(6.0), c.track());
}

/// A fully round blue button with a white label; adds its hit area.
#[allow(clippy::too_many_arguments)]
fn pill_button(c: &Canvas, out: &mut Painted, x: f32, y: f32, w: f32, h: f32, label: &str, act: WAct) {
    c.rr(x, y, w, h, h / 2.0, c.blue());
    let fs = style::CAPTION.size;
    c.text_cap(label, x + w / 2.0, y + (h - c.cap_h(fs)) / 2.0, fs, 600, WHITE, Align::Center, w);
    out.hits.push(((x, y, w, h), act));
}

/// What a face shows at a glance.
#[derive(Clone)]
enum Vis {
    Badge(Badge),
    /// Progress ring: fraction, colour, text inside (may be empty).
    Ring(f32, Col, String),
    /// A colour emoji filling the box (the clock-and-weather face's icon).
    Emoji(&'static str),
}

/// What the accessory slot (Dock L, and Small's top-right) holds when a face chooses it.
/// `Auto` picks from the summary: a trend (with the change over it), a meter or the change.
#[derive(Clone, Default)]
enum Acc {
    #[default]
    Auto,
    /// A second value over its label ("812" over "subscribers").
    Second(String, String),
    /// Two lines of text (the newest sender over the subject).
    Note(String, String),
    /// The last seven days, oldest first: active or not.
    Week(Vec<bool>),
}

struct Sum {
    vis: Vis,
    value: String,
    caption: String,
    delta: Option<f64>,
    series: Vec<f64>,
    meter: Option<(f32, Col)>,
    /// Mini's value when it isn't just the compact form of `value` ("$3.0K").
    mini: Option<String>,
    /// A unit after the value, in text2 ("7" + "days").
    unit: String,
    /// The trend's identity colour; None = green/red by the change (blue with none).
    tint: Option<Col>,
    /// The change figure's colour where "up" is not a judgment (text2).
    delta_col: Option<Col>,
    /// The series is daily quantities: draw the trend as bars.
    bars: bool,
    /// Dock M shows the change after the caption. Off: the change waits for Dock L's slot.
    inline_delta: bool,
    acc: Acc,
}

impl Sum {
    fn new(vis: Vis, value: impl Into<String>, caption: impl Into<String>) -> Self {
        Sum {
            vis,
            value: value.into(),
            caption: caption.into(),
            delta: None,
            series: Vec::new(),
            meter: None,
            mini: None,
            unit: String::new(),
            tint: None,
            delta_col: None,
            bars: false,
            inline_delta: true,
            acc: Acc::Auto,
        }
    }
    /// The trend's colour: the identity tint, else up/down for a change (blue for none),
    /// else the meter's colour, else blue.
    fn spark_col(&self) -> Col {
        match (self.tint, self.delta, self.meter) {
            (Some(t), _, _) => t,
            (None, Some(_), _) => trend_col(self.delta),
            (None, None, Some((_, col))) => col,
            _ => BLUE,
        }
    }
    /// The change figure and its colour.
    fn delta_text(&self, c: &Canvas) -> Option<(String, Col)> {
        self.delta.map(|d| {
            let (t, col) = pct_delta(c, d);
            (t, self.delta_col.unwrap_or(col))
        })
    }
    /// The trend, drawn the way the face reads it.
    fn trend(&self, c: &Canvas, x: f32, y: f32, w: f32, h: f32) {
        if self.bars {
            day_bars(c, x, y, w, h, &self.series, self.spark_col(), false);
        } else {
            spark(c, x, y, w, h, &self.series, self.spark_col(), false);
        }
    }
}

fn draw_vis(c: &Canvas, v: &Vis, x: f32, y: f32, size: f32) {
    match v {
        Vis::Badge(b) => c.badge(b, x, y, size),
        Vis::Ring(f, col, t) => {
            let r = size / 2.0;
            let th = (size * style::RING_T).max(3.0);
            c.ring(x + r, y + r, r - th / 2.0, th, *f, *col);
            // Under 30 px the number inside would be unreadable (the value is shown beside or below).
            if !t.is_empty() && size >= 30.0 {
                let fs = (size * 0.26).max(style::MINI_VALUE.min);
                if c.measure(t, fs, 600, Font::Text).0 <= size - 2.0 * th - 4.0 {
                    c.text_cap(t, x + r, y + r - c.cap_h(fs) / 2.0, fs, 600, c.text1(), Align::Center, size);
                }
            }
        }
        Vis::Emoji(e) => {
            c.text(e, x + size / 2.0, y + size * 0.02, size * 0.8, 400, c.text1(), Align::Center, size * 1.4, Font::Text);
        }
    }
}

/// What a Dock L pill shows in its accessory slot.
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    None,
    Spark,
    Meter,
    Delta,
    Own,
}

/// A value with an optional unit after it on the same baseline (text2): "7 days".
#[allow(clippy::too_many_arguments)]
fn value_unit(c: &Canvas, v: &str, unit: &str, x: f32, base: f32, fs: f32, weight: u32, unit_fs: f32) -> f32 {
    let w = c.text_base(v, x, base, fs, weight, c.text1(), Align::Left, c.w * 2.0);
    if unit.is_empty() {
        return w;
    }
    let gap = style::SP[1];
    w + gap + c.text_base(unit, x + w + gap, base, unit_fs, 600, c.text2(), Align::Left, c.w)
}

/// Seven day cells (oldest first) right-aligned at `right`: filled in `col` where active,
/// track where not.
fn week_cells(c: &Canvas, days: &[bool], right: f32, y: f32, cell: f32, gap: f32, r: f32, col: Col) -> f32 {
    let n = days.len().min(7);
    let x0 = right - n as f32 * cell - (n.saturating_sub(1)) as f32 * gap;
    for (i, on) in days.iter().rev().take(n).rev().enumerate() {
        let x = x0 + i as f32 * (cell + gap);
        c.rr(x, y, cell, cell, r, if *on { col } else { c.track() });
    }
    x0
}

/// Mini, Dock M/L and Small layouts shared by most faces (§3).
fn compact_sum(c: &Canvas, s: &Sum, sh: Shape) {
    let vcol = c.text1();
    match sh {
        Shape::Mini => {
            // Dock sizes follow the icon size; the spec is drawn at 54.
            let k = c.h / 54.0;
            if let Vis::Ring(f, col, t) = &s.vis {
                // Visual-led: the ring is the icon, its number inside.
                let v = style::BADGE_MINI_VIS * k;
                let t = if t.is_empty() { s.value.clone() } else { t.clone() };
                draw_vis(c, &Vis::Ring(*f, *col, t), (c.w - v) / 2.0, (c.h - v) / 2.0, v);
                return;
            }
            let value = s.mini.as_deref().unwrap_or(&s.value);
            if value.is_empty() {
                let v = style::BADGE_MINI_VIS * k;
                draw_vis(c, &s.vis, (c.w - v) / 2.0, (c.h - v) / 2.0, v);
                return;
            }
            // Value-led: a small mark over one compact value.
            let v = style::BADGE_MINI * k;
            draw_vis(c, &s.vis, (c.w - v) / 2.0, 8.0 * k, v);
            let role = Type { size: style::MINI_VALUE.size * k, ..style::MINI_VALUE };
            let (val, fs) = fit_value(c, value, role, c.w - 2.0 * style::M_MINI * k);
            c.text_base(&val, c.w / 2.0, 45.0 * k, fs, role.weight, vcol, Align::Center, c.w * 2.0);
        }
        Shape::Dock => {
            let k = c.h / 54.0;
            let m = style::M_DOCK * k;
            let bs = style::BADGE_DOCK * k;
            draw_vis(c, &s.vis, m, (c.h - bs) / 2.0, bs);
            let x0 = m + bs + style::SP[4] * k;
            // Dock L (the wide pill) adds one accessory at the right; Dock M never does.
            let slot = if c.w < c.h * 3.4 {
                Slot::None
            } else if !matches!(s.acc, Acc::Auto) {
                Slot::Own
            } else if s.series.len() > 2 {
                Slot::Spark
            } else if s.meter.is_some() {
                Slot::Meter
            } else if s.delta.is_some() {
                Slot::Delta
            } else {
                Slot::None
            };
            let slot_w = (c.w * 0.34).round();
            let slot_x = c.w - m - slot_w;
            let right = if slot == Slot::None { c.w - m } else { slot_x - style::SP[4] * k };
            let col_meter = s.meter.filter(|_| slot != Slot::Meter);
            let role = Type { size: style::VALUE.size * k, ..style::VALUE };
            let ufs = style::TITLE.size * k;
            let uw = if s.unit.is_empty() { 0.0 } else { c.measure(&s.unit, ufs, 600, Font::Text).0 + style::SP[1] * k };
            let (val, fs) = fit_value(c, &s.value, role, right - x0 - uw);
            // With a meter under the caption the three lines move up as one block, so the
            // caption keeps clear of the value (baseline 40 under a value at 30 touched it).
            let (val_base, cap_base, bar_y) = if col_meter.is_some() { (23.0, 36.0, 41.0) } else { (30.0, 44.0, 45.0) };
            let (val_base, cap_base, bar_y) = (val_base * k, cap_base * k, bar_y * k);
            value_unit(c, &val, &s.unit, x0, val_base, fs, role.weight, ufs);
            let cs = style::FOOT.size * k;
            // The change shares the caption line only on faces that ask for it, and only
            // when both fit whole; a slot that shows the change keeps it to itself.
            let slot_has_delta = matches!(slot, Slot::Delta) || (slot == Slot::Spark && s.delta.is_some());
            let cap = short_caption(&s.caption);
            let both = s.delta_text(c).filter(|_| s.inline_delta && !slot_has_delta).filter(|(dt, _)| {
                let gap = if cap.is_empty() { 0.0 } else { style::SP[2] * k };
                c.measure(&cap, cs, 400, Font::Text).0 + gap + c.measure(dt, cs, 600, Font::Text).0 <= right - x0
            });
            match both {
                Some((dt, dcol)) => {
                    let cw = if cap.is_empty() { 0.0 } else { c.text_base(&cap, x0, cap_base, cs, 400, c.text2(), Align::Left, right - x0) + style::SP[2] * k };
                    c.text_base(&dt, x0 + cw, cap_base, cs, 600, dcol, Align::Left, right - x0 - cw);
                }
                None => {
                    c.text_base(&s.caption, x0, cap_base, cs, 400, c.text2(), Align::Left, right - x0);
                }
            }
            if let Some((f, col)) = col_meter {
                c.bar(x0, bar_y, right - x0, 3.0 * k, f, col);
            }
            let (sy0, sy1) = (14.0 * k, 40.0 * k);
            match slot {
                Slot::Spark => match s.delta_text(c) {
                    // The change over its trend, so the pill says which way it went.
                    Some((dt, dcol)) => {
                        c.text_base(&dt, c.w - m, 22.0 * k, cs, 600, dcol, Align::Right, slot_w);
                        s.trend(c, slot_x, 27.0 * k, slot_w, sy1 - 27.0 * k);
                    }
                    None => s.trend(c, slot_x, sy0, slot_w, sy1 - sy0),
                },
                Slot::Meter => {
                    if let Some((f, col)) = s.meter {
                        let pct = format!("{:.0}%", (f * 100.0).clamp(0.0, 999.0));
                        c.text_base(&pct, c.w - m, 23.0 * k, cs, 500, c.text2(), Align::Right, slot_w);
                        c.bar(slot_x, 29.0 * k, slot_w, style::METER_H_COMPACT * k, f, col);
                    }
                }
                Slot::Delta => {
                    if let Some((dt, dcol)) = s.delta_text(c) {
                        c.text_base(&dt, c.w - m, 32.0 * k, style::TITLE.size * k, 600, dcol, Align::Right, slot_w);
                    }
                }
                Slot::Own => match &s.acc {
                    Acc::Second(v, l) => {
                        c.text_base(v, c.w - m, val_base, style::TITLE.size * k, 600, vcol, Align::Right, slot_w);
                        c.text_base(l, c.w - m, cap_base, cs, 400, c.text2(), Align::Right, slot_w);
                    }
                    Acc::Note(a, b) => {
                        c.text_base(a, slot_x, 26.0 * k, cs, 600, vcol, Align::Left, slot_w);
                        c.text_base(b, slot_x, 40.0 * k, cs, 400, c.text2(), Align::Left, slot_w);
                    }
                    Acc::Week(days) => {
                        let d = 7.0 * k;
                        week_cells(c, days, c.w - m, 27.0 * k - d / 2.0, d, 3.0 * k, d / 2.0, s.spark_col());
                    }
                    Acc::Auto => {}
                },
                Slot::None => {}
            }
        }
        _ => {
            // Small card, bottom-anchored: mark and accessory on top, value and caption below.
            let m = style::M_SMALL;
            let bs = style::BADGE_SMALL;
            draw_vis(c, &s.vis, m, m, bs);
            // The top-right accessory slot: x 52 to 114, y 16 to 42.
            let (sx1, sw) = (c.w - m, 62.0);
            let sx0 = sx1 - sw;
            let fs = style::FOOT.size;
            match &s.acc {
                Acc::Second(v, l) => {
                    c.text_base(v, sx1, 26.0, fs, 600, vcol, Align::Right, sw);
                    c.text_base(l, sx1, 40.0, fs, 400, c.text2(), Align::Right, sw);
                }
                Acc::Note(a, _) => {
                    c.text_base(a, sx1, 26.0, fs, 600, c.text2(), Align::Right, sw);
                }
                Acc::Week(days) => {
                    week_cells(c, days, sx1, 23.0, 6.0, 3.0, 3.0, s.spark_col());
                }
                Acc::Auto => {
                    let mut sy0 = 16.0;
                    if let Some((t, col)) = s.delta_text(c) {
                        c.text_base(&t, sx1, 26.0, fs, 600, col, Align::Right, sw);
                        sy0 = 30.0;
                    }
                    if s.series.len() > 2 {
                        s.trend(c, sx0, sy0, sw, 42.0 - sy0);
                    } else if let Some((f, col)) = s.meter {
                        c.bar(sx0, if s.delta.is_some() { 36.0 } else { 27.0 }, sw, style::METER_H_COMPACT, f, col);
                    }
                }
            }
            let ufs = style::TITLE.size;
            let uw = if s.unit.is_empty() { 0.0 } else { c.measure(&s.unit, ufs, 600, Font::Text).0 + style::SP[1] };
            let (v, vfs) = fit_value(c, &s.value, style::HERO_S, c.w - 2.0 * m - uw);
            value_unit(c, &v, &s.unit, m, 86.0, vfs, style::HERO_S.weight, ufs);
            c.text_base(&s.caption, m, 106.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, c.w - 2.0 * m);
        }
    }
}

/// A caption short enough to share a dock pill with a change figure.
fn short_caption(cap: &str) -> String {
    let t = cap.trim();
    match t.strip_prefix("Last ") {
        Some(rest) if rest.ends_with(" days") => rest.to_string(),
        _ => t.to_string(),
    }
}

/// A reason line: text2, starting with an orange warning glyph for errors.
#[allow(clippy::too_many_arguments)]
fn reason_line(c: &Canvas, s: &str, x: f32, base: f32, size: f32, err: bool, align: Align, max_w: f32) {
    let g = if err { 12.0 + style::SP[1] } else { 0.0 };
    let tw = c.measure(s, size, 400, Font::Text).0.min(max_w - g);
    let left = match align {
        Align::Left => x,
        Align::Center => x - (g + tw) / 2.0,
        Align::Right => x - g - tw,
    };
    if err {
        c.glyph(G_WARN, left + 6.0, base - c.cap_h(size) / 2.0, 12.0, ORANGE);
    }
    c.text_base(s, left + g, base, size, 400, c.text2(), Align::Left, max_w - g);
}

/// What a Large Glance face shows under its separator, for the loading skeleton.
#[derive(Clone, Copy, PartialEq)]
enum Detail {
    /// Up to three rows and a closing footnote.
    Rows,
    /// A history chart filling the region.
    Chart,
    /// A 40 px chart over a three-stat footer (Resend, Search Console).
    ChartFooter,
    /// Followers: the milestone line and meter over a footer of this many columns.
    Footer(usize),
    /// Three stats, the paid / trial meter and its footnote (RevenueCat).
    Subs,
    /// A full-width meter on Medium; three stats and a pill on Large (OpenRouter).
    Credits,
    /// The contribution heat map (GitHub streak).
    Heat,
}

/// What an accessory holds (Dock L's slot and Small's top-right, or Medium's box), so the
/// skeleton draws the same shape the loaded card will.
#[derive(Clone, Copy, PartialEq)]
enum Sk {
    None,
    /// A sparkline or meter.
    Trend,
    /// A second value over its label, right-aligned.
    Stat,
    /// Two lines of text, left-aligned (the newest mail).
    Note,
    /// Seven day cells.
    Week,
}

/// The layout a connector face really draws, so its loading skeleton matches it (§7).
enum Tpl {
    /// Glance faces: Dock L / Small accessory, Medium box, Medium third line, Large detail.
    Glance { acc: Sk, boxed: Sk, third: bool, detail: Detail },
    /// Header faces (charts), with their real title.
    Header(String),
}

/// The whole-card fallback for connector faces that have no data (§7): a skeleton while
/// loading, the Connect layout when it needs setting up, the reason when it failed.
fn placeholder(c: &Canvas, out: &mut Painted, name: &str, badge: &Badge, conn: &'static str, g: &Got, sh: Shape) -> bool {
    placeholder_tpl(c, out, name, badge, conn, g, sh, &Tpl::Glance { acc: Sk::None, boxed: Sk::None, third: false, detail: Detail::Rows })
}

/// Seven track cells right-aligned at `right` (a week strip that hasn't loaded).
fn week_skeleton(c: &Canvas, right: f32, y: f32, cell: f32, gap: f32, r: f32) {
    week_cells(c, &[false; 7], right, y, cell, gap, r, c.track());
}

/// `placeholder` with the face's real template, so loading → loaded doesn't jump.
#[allow(clippy::too_many_arguments)]
fn placeholder_tpl(c: &Canvas, out: &mut Painted, name: &str, badge: &Badge, conn: &'static str, g: &Got, sh: Shape, tpl: &Tpl) -> bool {
    let (loading, connect, reason) = match g {
        Got::Feed(_) => return false,
        Got::Loading => (true, false, String::new()),
        Got::Setup(m) => (false, true, m.clone()),
        Got::Error(m) => (false, false, m.clone()),
    };
    let err = !loading && !connect;
    if connect {
        out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Connect(conn)));
    }
    let header = matches!(tpl, Tpl::Header(_));
    // What Dock L's slot and Small's top-right hold once loaded.
    let acc = match tpl {
        Tpl::Header(_) => Sk::Trend,
        Tpl::Glance { acc, .. } => *acc,
    };
    match sh {
        Shape::Mini if loading => {
            // The badge where the loaded value-led Mini puts it, over a value bar.
            let k = c.h / 54.0;
            let bs = style::BADGE_MINI * k;
            c.badge(badge, (c.w - bs) / 2.0, 8.0 * k, bs);
            skeleton(c, (c.w - 26.0 * k) / 2.0, 37.0 * k, 26.0 * k, 8.0 * k);
        }
        Shape::Mini => {
            let k = c.h / 54.0;
            let bs = 30.0 * k;
            let (bx, by) = ((c.w - bs) / 2.0, (c.h - bs) / 2.0);
            c.badge(badge, bx, by, bs);
            if connect || err {
                let (dx, dy, r) = (bx + bs - 1.0 * k, by + 1.0 * k, 3.5 * k);
                c.circle(dx, dy, r + 1.5 * k, c.halo());
                c.circle(dx, dy, r, if connect { BLUE } else { ORANGE });
            }
        }
        Shape::Dock => {
            let k = c.h / 54.0;
            let m = style::M_DOCK * k;
            let bs = style::BADGE_DOCK * k;
            c.badge(badge, m, (c.h - bs) / 2.0, bs);
            let x0 = m + bs + style::SP[4] * k;
            let tw = c.w - m - x0;
            if loading {
                // Value and caption bars where the loaded text sits; Dock L's slot too.
                let wide = c.w >= c.h * 3.4 && acc != Sk::None;
                let slot_w = (c.w * 0.34).round();
                let slot_x = c.w - m - slot_w;
                let tw = if wide { slot_x - style::SP[4] * k - x0 } else { tw };
                skeleton(c, x0, 20.0 * k, (60.0 * k).min(tw), 10.0 * k);
                skeleton(c, x0, 38.0 * k, (36.0 * k).min(tw), 6.0 * k);
                if wide {
                    let right = c.w - m;
                    match acc {
                        Sk::Trend => skeleton(c, slot_x, 25.0 * k, slot_w, style::METER_H_COMPACT * k),
                        Sk::Stat => {
                            skeleton(c, right - slot_w * 0.5, 20.0 * k, slot_w * 0.5, 10.0 * k);
                            skeleton(c, right - slot_w * 0.75, 38.0 * k, slot_w * 0.75, 6.0 * k);
                        }
                        Sk::Note => {
                            skeleton(c, slot_x, 18.0 * k, slot_w * 0.6, 8.0 * k);
                            skeleton(c, slot_x, 34.0 * k, slot_w, 6.0 * k);
                        }
                        Sk::Week => {
                            let d = 7.0 * k;
                            week_skeleton(c, right, 27.0 * k - d / 2.0, d, 3.0 * k, d / 2.0);
                        }
                        Sk::None => {}
                    }
                }
            } else if connect && c.w >= c.h * 3.4 {
                // Dock L: the reason under the name, and a Connect pill in the accessory slot.
                let slot_w = (c.w * 0.34).round();
                let ph = style::BTN_SECONDARY * k;
                let tw = c.w - m - slot_w - style::SP[4] * k - x0;
                let ns = fit(c, name, style::TITLE.size * k, style::TITLE.min * k, 600, Font::Text, tw);
                c.text_base(name, x0, 30.0 * k, ns, 600, c.text1(), Align::Left, tw);
                reason_line(c, &reason, x0, 44.0 * k, style::FOOT.size * k, false, Align::Left, tw);
                let (px, py) = (c.w - m - slot_w, (c.h - ph) / 2.0);
                c.rr(px, py, slot_w, ph, ph / 2.0, c.blue());
                let fs = style::FOOT.size * k;
                c.text_cap("Connect", px + slot_w / 2.0, py + (ph - c.cap_h(fs)) / 2.0, fs, 600, WHITE, Align::Center, slot_w);
            } else {
                // The name fits 13 → 11 before it ellipsizes ("Revenue Goal", not "Revenue G…").
                let ns = fit(c, name, style::TITLE.size * k, style::TITLE.min * k, 600, Font::Text, tw);
                c.text_base(name, x0, 30.0 * k, ns, 600, c.text1(), Align::Left, tw);
                if connect {
                    c.text_base("Connect", x0, 44.0 * k, style::FOOT.size * k, 600, BLUE, Align::Left, tw);
                } else {
                    reason_line(c, &reason, x0, 44.0 * k, style::FOOT.size * k, err, Align::Left, tw);
                }
            }
        }
        Shape::Small if loading && header => {
            // Chart-led Small: badge and period on top, the value, the chart's baseline.
            let m = style::M_SMALL;
            c.badge(badge, m, m, style::BADGE_HEADER);
            skeleton(c, m + style::BADGE_HEADER + style::SP[1], 24.0, 36.0, 6.0);
            let ch = c.cap_h(style::HERO_S.size);
            skeleton(c, m, 62.0 - ch, (c.w - 2.0 * m) * 0.6, ch);
            base_line(c, m, c.w - 2.0 * m, c.h - m);
        }
        Shape::Small => {
            let m = style::M_SMALL;
            c.badge(badge, m, m, style::BADGE_SMALL);
            let inner = c.w - 2.0 * m;
            if loading {
                let ch = c.cap_h(style::HERO_S.size);
                skeleton(c, m, 86.0 - ch, inner * 0.6, ch);
                skeleton(c, m, 106.0 - 8.0, 40.0, 8.0);
                // The top-right accessory slot (x 52 to 114, y 16 to 42).
                let right = c.w - m;
                match acc {
                    Sk::Trend => skeleton(c, right - 62.0, 36.0, 62.0, style::METER_H_COMPACT),
                    Sk::Stat => {
                        skeleton(c, right - 28.0, 18.0, 28.0, 8.0);
                        skeleton(c, right - 40.0, 34.0, 40.0, 6.0);
                    }
                    Sk::Note => skeleton(c, right - 40.0, 18.0, 40.0, 8.0),
                    Sk::Week => week_skeleton(c, right, 23.0, 6.0, 3.0, 3.0),
                    Sk::None => {}
                }
            } else {
                // Name and reason sit above the pill on the bottom margin.
                let pill_y = c.h - m - 26.0;
                let (nb, rb) = if connect { (pill_y - 22.0, pill_y - 7.0) } else { (86.0, 106.0) };
                c.text_base(name, m, nb, style::TITLE.size, 600, c.text1(), Align::Left, inner);
                reason_line(c, &reason, m, rb, style::FOOT.size, err, Align::Left, inner);
                if connect {
                    pill_button(c, out, m, pill_y, inner, 26.0, "Connect", WAct::Connect(conn));
                }
            }
        }
        Shape::Medium | Shape::Large if loading => {
            let m = style::M_CARD;
            let large = sh == Shape::Large;
            match tpl {
                Tpl::Header(title) => {
                    // The real badge and title; bars for the trailing value, subtitle and
                    // change; the chart's baseline; Large's three footer columns.
                    let bs = style::BADGE_HEADER;
                    c.badge(badge, m, m, bs);
                    let tx = m + bs + style::SP[3];
                    let (vw, cw) = (48.0, 32.0);
                    c.text_base(title, tx, style::HEADER_BASE, style::TITLE.size, style::TITLE.weight, c.text1(), Align::Left, c.w - m - vw - style::SP[5] - tx);
                    skeleton(c, c.w - m - vw, 19.0, vw, 12.0);
                    skeleton(c, tx, 38.0, 40.0, 8.0);
                    skeleton(c, c.w - m - cw, 38.0, cw, 8.0);
                    let plot_bottom = if large { HDR_PLOT_BOTTOM_L } else { c.h - m };
                    base_line(c, m, c.w - 2.0 * m, plot_bottom);
                    if large {
                        let colw = (c.w - 2.0 * m) / 3.0;
                        for i in 0..3 {
                            let x0 = m + i as f32 * colw;
                            skeleton(c, x0, 154.0, 40.0, 12.0);
                            skeleton(c, x0, 174.0, 32.0, 8.0);
                        }
                    }
                }
                Tpl::Glance { boxed, third, detail, .. } => {
                    let bs = style::BADGE_GLANCE;
                    let right = c.w - m;
                    c.badge(badge, right - bs, m, bs);
                    skeleton(c, m, m, (c.w - 2.0 * m - bs - style::SP[3]) * 0.6, c.cap_h(style::HERO.size));
                    skeleton(c, m, 58.0 - 8.0, 40.0, 8.0);
                    if *third {
                        skeleton(c, m, 76.0 - 6.0, 120.0, 6.0);
                    }
                    // Large keeps Medium's box unless its detail shows the same data.
                    let box_gone = large && matches!(detail, Detail::Chart | Detail::ChartFooter | Detail::Heat | Detail::Footer(_));
                    if *detail == Detail::Credits {
                        // The full-width meter under the caption.
                        skeleton(c, m, 70.0, c.w - 2.0 * m, style::METER_H);
                    } else if !box_gone {
                        match boxed {
                            Sk::Trend => skeleton(c, ACC_X, 76.0, right - ACC_X, style::METER_H_COMPACT),
                            Sk::Stat => {
                                skeleton(c, right - 36.0, 50.0, 36.0, 12.0);
                                skeleton(c, right - 28.0, 68.0, 28.0, 8.0);
                            }
                            Sk::Week => week_skeleton(c, right, 62.0, 12.0, 3.0, 3.0),
                            Sk::Note | Sk::None => {}
                        }
                    }
                    if large {
                        separator(c, style::SEP_Y_LARGE);
                        let (x, y, w, h) = detail_region(c);
                        let bottom = y + h;
                        // Three footer columns: value bar over label bar, from the value's baseline.
                        let footer = |n: usize, base: f32| {
                            let colw = w / 3.0;
                            for i in 0..n.min(3) {
                                skeleton(c, x + i as f32 * colw, base - 12.0, 40.0, 12.0);
                                skeleton(c, x + i as f32 * colw, base + style::SP[6] - 8.0, 28.0, 8.0);
                            }
                        };
                        match detail {
                            Detail::Rows => {
                                for (i, f) in [1.0, 0.75, 0.5].iter().enumerate() {
                                    skeleton(c, x, y + 4.0 + i as f32 * style::ROW_L, w * f, 8.0);
                                }
                                // The closing footnote.
                                skeleton(c, x, bottom - 7.0, 96.0, 6.0);
                            }
                            Detail::Chart => {
                                // A soft block where the chart will be, on its baseline.
                                c.rr(x, y + 16.0, w, h - 24.0, style::R_MIN, alpha(c.track(), 0.5));
                                base_line(c, x, w, y + h);
                            }
                            Detail::ChartFooter => {
                                c.rr(x, y, w, CHART_FOOT_H, style::R_MIN, alpha(c.track(), 0.5));
                                footer(3, CHART_FOOT_BASE);
                            }
                            Detail::Footer(n) => {
                                skeleton(c, x, SOCIAL_HEAD_BASE - 9.0, 96.0, 9.0);
                                c.rr(x, SOCIAL_METER_Y, w, style::METER_H, style::METER_H / 2.0, c.track());
                                footer(*n, HDR_FOOT_BASE);
                            }
                            Detail::Subs => {
                                footer(3, SUBS_FOOT_BASE);
                                c.rr(x, SUBS_METER_Y, w, style::METER_H, style::METER_H / 2.0, c.track());
                                skeleton(c, x, SUBS_NOTE_BASE - 7.0, 120.0, 6.0);
                            }
                            Detail::Credits => {
                                footer(3, SUBS_FOOT_BASE);
                                // The secondary pill on the bottom margin.
                                let ph = style::BTN_SECONDARY;
                                c.rr(x, y + h - ph, w, ph, ph / 2.0, alpha(c.track(), 0.5));
                            }
                            Detail::Heat => heat_weeks(c, &[], 0, 0, c.track()),
                        }
                    }
                }
            }
        }
        Shape::Medium => {
            let m = style::M_CARD;
            let bs = style::BADGE_SMALL;
            c.badge(badge, m, m, bs);
            let x0 = m + bs + style::SP[5];
            c.text_base(name, x0, 29.0, 15.0, 600, c.text1(), Align::Left, c.w - m - x0);
            reason_line(c, &reason, x0, 46.0, style::CAPTION.size, err, Align::Left, c.w - m - x0);
            if connect {
                let (bw, bh) = (88.0, 28.0);
                pill_button(c, out, c.w - m - bw, c.h - m - bh, bw, bh, "Connect", WAct::Connect(conn));
            }
        }
        Shape::Large => {
            // Without a pill under it (errors) the column moves down to stay centred.
            let dy = if connect { 0.0 } else { style::SP[7] };
            let bs = 48.0;
            c.badge(badge, (c.w - bs) / 2.0, 36.0 + dy, bs);
            c.text_base(name, c.w / 2.0, 108.0 + dy, 15.0, 600, c.text1(), Align::Center, c.w - 2.0 * style::M_CARD);
            reason_line(c, &reason, c.w / 2.0, 128.0 + dy, style::CAPTION.size, err, Align::Center, c.w - 2.0 * style::M_CARD);
            if connect {
                let (bw, bh) = (132.0, 30.0);
                pill_button(c, out, (c.w - bw) / 2.0, 150.0, bw, bh, "Connect", WAct::Connect(conn));
            }
        }
    }
    true
}

pub fn paint(c: &Canvas, def: &Def, x: &Ctx, dense: bool, wic: &IWICImagingFactory) -> Painted {
    let mut out = Painted::default();
    let sh = shape_of(c, dense);
    let custom_bg = matches!(def.face, Face::Weather(WeatherKind::Banner) | Face::Wallpaper | Face::Clock(ClockKind::DayPage | ClockKind::AnalogSquare) | Face::Media(MediaKind::Disc));
    if !custom_bg || dense {
        bg(c, dense);
    }
    match def.face {
        Face::Stat { conn, badge, open } => stat(c, &mut out, def, x, conn, &badge, open, sh),
        Face::Chart { conn, badge, open } => chart(c, &mut out, def, x, conn, &badge, open, sh),
        Face::Usage { conn, badge, open } => usage(c, &mut out, def, x, conn, &badge, open, sh),
        Face::Heat { conn, badge, color, open } => heat(c, &mut out, def, x, conn, &badge, hex(color), open, sh),
        Face::List { conn, badge, open } => list(c, &mut out, def, x, conn, &badge, open, sh),
        Face::Watch { conn } => watch(c, &mut out, def, x, conn, sh),
        Face::Next => next_event(c, &mut out, def, x, sh),
        Face::AiAll => ai_all(c, &mut out, x, sh),
        Face::Goal => goal(c, &mut out, x, sh),
        Face::Wallpaper => wallpaper(c, &mut out, def, x, wic, sh),
        Face::Clock(k) => clock(c, &mut out, k, x, sh),
        Face::Weather(k) => weather(c, &mut out, k, x, sh),
        Face::Timer(k) => timer(c, &mut out, k, x, sh),
        Face::Media(k) => media(c, &mut out, k, x, wic, sh),
        Face::Sys(k) => sys(c, &mut out, k, x, sh),
        Face::Tool(k) => tool(c, &mut out, k, x, sh),
        Face::Progress(s) => progress(c, s, x, sh),
    }
    out
}

fn compactish(sh: Shape) -> bool {
    matches!(sh, Shape::Mini | Shape::Dock | Shape::Small)
}

// ------------------------------------------------------------------ connector faces
// Stat faces use the Glance template, Chart faces the Header template, Usage faces the
// Glance template with a meter or a token trend. Mini and Dock are shared (compact_sum).

const TEAL: Col = hex(0x10A37F);
const INDIGO: Col = hex(0x6467F2);
/// The Medium Glance accessory box: x 148 to w − 16, y 48 to 80.
const ACC_X: f32 = 148.0;
const ACC_TOP: f32 = 48.0;
/// Header faces: the plot starts under the two-line header; Large's plot ends here.
const HDR_PLOT_TOP: f32 = 52.0;
const HDR_PLOT_BOTTOM_L: f32 = 132.0;
/// Large Header faces: the three-stat footer's value baseline (labels 16 below).
const HDR_FOOT_BASE: f32 = 166.0;
/// Large Glance details (y 108 to 188): a 38 px chart over the family's footer (values on
/// 166, labels on 182: Resend, Search Console).
const CHART_FOOT_H: f32 = 38.0;
const CHART_FOOT_BASE: f32 = HDR_FOOT_BASE;
/// Followers: the next-milestone line on 124 and its 6 px meter at 134, over the footer.
const SOCIAL_HEAD_BASE: f32 = 124.0;
const SOCIAL_METER_Y: f32 = 134.0;
/// RevenueCat and OpenRouter: stats on 126, then the meter at 158 and its footnote on 182.
const SUBS_FOOT_BASE: f32 = 126.0;
const SUBS_METER_Y: f32 = 158.0;
const SUBS_NOTE_BASE: f32 = 182.0;
/// Daily bars: earlier days are drawn at this opacity, the latest at full colour. Light
/// glass needs more ink for past bars to keep about 3:1 against the card.
const BAR_PAST: f32 = 0.55;
const BAR_PAST_LIGHT: f32 = 0.8;
/// The widest a daily bar gets (a week of bars stays fine, not slab-like).
const BAR_MAX_W: f32 = 20.0;

fn same_col(a: Col, b: Col) -> bool {
    (a.0 - b.0).abs() < 0.003 && (a.1 - b.1).abs() < 0.003 && (a.2 - b.2).abs() < 0.003
}

/// A trend's colour where up is a judgment (money, prices): green up, red down, and the
/// neutral blue when there is no change to back either.
fn trend_col(d: Option<f64>) -> Col {
    match d {
        Some(d) if d >= 0.05 => GREEN,
        Some(d) if d <= -0.05 => RED,
        _ => BLUE,
    }
}

/// A widget's identity colour for every chart, meter and sparkline it draws (§2.3), or
/// None where the trend's sign colours it (revenue and prices).
fn ident(c: &Canvas, conn: &str) -> Option<Col> {
    match conn {
        "claude_code" | "anthropic" => Some(CLAUDE),
        "openai" => Some(TEAL),
        "codex" => Some(if c.light { hex(0x6E7378) } else { hex(0x9AA0A6) }),
        "openrouter" => Some(INDIGO),
        "plausible" | "umami" | "supabase" | "search_console" | "resend" => Some(BLUE),
        "github" => Some(if c.light { hex(0x2DA44E) } else { hex(0x39D353) }),
        _ => None,
    }
}

/// A line's colour: in the light theme a deeper tone, so a 1.5 px stroke keeps 3:1 on pale
/// glass (system accents take their text variant; Claude orange and OpenAI teal deepen).
fn line_col(c: &Canvas, col: Col) -> Col {
    if !c.light {
        col
    } else if same_col(col, CLAUDE) {
        hex(0xC8643F)
    } else if same_col(col, TEAL) {
        hex(0x0E8A6B)
    } else {
        c.tone(col, true)
    }
}

/// A long series thinned to about `n` points (means of equal runs), so a narrow line
/// reads as a trend rather than day-to-day noise.
fn thin(vals: &[f64], n: usize) -> Vec<f64> {
    if vals.len() <= n + 2 {
        return vals.to_vec();
    }
    let k = vals.len().div_ceil(n);
    vals.chunks(k).map(|ch| ch.iter().sum::<f64>() / ch.len() as f64).collect()
}

/// A sparkline (§2.5): thinned when narrow, smoothed, stroke 1.5, gradient fill.
#[allow(clippy::too_many_arguments)]
fn spark(c: &Canvas, x: f32, y: f32, w: f32, h: f32, vals: &[f64], col: Col, dot: bool) {
    let v = if w < 120.0 { thin(vals, 10) } else { vals.to_vec() };
    c.chart(x, y, w, h, &v, line_col(c, col), ChartOpts { fill: true, dot, stroke: style::STROKE_SPARK, smooth: v.len() >= 5 });
}

/// A 1 px separator-colour hairline whose bottom edge is `bottom`.
fn base_line(c: &Canvas, x: f32, w: f32, bottom: f32) {
    c.line(x + 0.5, bottom - 0.5, x + w - 0.5, bottom - 0.5, 1.0, c.sep());
}

/// Daily quantities as bars from zero: fully rounded, earlier days at `BAR_PAST`, the
/// latest at full colour, 14% headroom over the tallest; optionally on a baseline.
#[allow(clippy::too_many_arguments)]
fn day_bars(c: &Canvas, x: f32, y: f32, w: f32, h: f32, vals: &[f64], col: Col, baseline: bool) {
    let n = vals.len();
    if n == 0 || w <= 0.0 || h <= 0.0 {
        return;
    }
    let col = line_col(c, col);
    let max = vals.iter().copied().filter(|v| v.is_finite()).fold(0.0, f64::max);
    let top = if max > 0.0 { max * (1.0 + style::CHART_PAD_HI) } else { 1.0 };
    let pitch = w / n as f32;
    let gap = (pitch * 0.36).clamp(1.33, 4.0);
    let bw = (pitch - gap).clamp(1.0, BAR_MAX_W);
    let floor = if baseline { y + h - 2.0 } else { y + h };
    if baseline {
        base_line(c, x, w, y + h);
    }
    // Fully round tops on a tall plot; short plots keep a tighter corner so low bars
    // don't turn into pills.
    let r = (bw / 2.0).min(if h >= 60.0 { 6.0 } else { 3.0 });
    let past = if c.light { BAR_PAST_LIGHT } else { BAR_PAST };
    for (i, v) in vals.iter().enumerate() {
        let v = if v.is_finite() { v.max(0.0) } else { 0.0 };
        if v <= 0.0 {
            continue;
        }
        let bh = ((v / top) as f32 * (floor - y)).max(bw.min(2.0));
        let a = if i + 1 == n { 1.0 } else { past };
        c.rr(x + i as f32 * pitch + (pitch - bw) / 2.0, floor - bh, bw, bh, r, alpha(col, a));
    }
}

/// Where `v` lands in `c.chart` for the same box and options, or None when it falls
/// outside the plotted range (used for a price chart's reference line).
fn chart_y(vals: &[f64], y: f32, h: f32, stroke: f32, dot: bool, v: f64) -> Option<f32> {
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for x in vals.iter().filter(|v| v.is_finite()) {
        lo = lo.min(*x);
        hi = hi.max(*x);
    }
    if lo > hi {
        return None;
    }
    if (hi - lo).abs() < 1e-9 {
        hi = lo + 1.0;
        lo -= 1.0;
    }
    let range = hi - lo;
    let (lo, hi) = (lo - range * style::CHART_PAD_LO, hi + range * style::CHART_PAD_HI);
    if v < lo || v > hi {
        return None;
    }
    let inset = stroke / 2.0;
    let top = y + inset.max(if dot { style::CHART_DOT + 1.5 } else { 0.0 });
    let bottom = y + h - inset;
    Some(bottom - ((v - lo) / (hi - lo)) as f32 * (bottom - top))
}

/// A dashed hairline (a price's reference level).
fn dashed(c: &Canvas, x0: f32, x1: f32, y: f32, col: Col) {
    let mut x = x0;
    while x < x1 {
        c.line(x + 0.5, y, (x + 2.5).min(x1), y, 1.0, col);
        x += 6.0;
    }
}

/// Mini's money (≤ 5 glyphs): "$126", "$5.3K", "$3.0K" (one decimal kept under $10K),
/// "$18K", "$1.2M".
fn mini_money(v: f64, cur: Option<&str>) -> String {
    let full = money(v, cur);
    let Some(start) = full.find(|ch: char| ch.is_ascii_digit()).filter(|s| *s > 0) else {
        return compact_str(&full).unwrap_or(full);
    };
    let a = v.abs();
    let body = if a >= 1e6 {
        format!("{:.1}M", a / 1e6)
    } else if a >= 1e4 {
        format!("{:.0}K", a / 1e3)
    } else if a >= 1e3 {
        format!("{:.1}K", a / 1e3)
    } else {
        format!("{a:.0}")
    };
    format!("{}{body}", &full[..start])
}

/// Mini's count (≤ 5 glyphs), the same rule as money: one decimal under 10K ("4.0K"),
/// whole thousands from 10K ("22K", "412K"), then "1.9M", "12M".
fn mini_count(v: f64) -> String {
    let a = v.abs();
    let sign = if v < 0.0 { "-" } else { "" };
    let body = if a < 1e3 {
        format!("{}", a.round() as i64)
    } else if a < 9.95e3 {
        format!("{:.1}K", a / 1e3)
    } else if a < 999.5e3 {
        format!("{:.0}K", a / 1e3)
    } else if a < 9.95e6 {
        format!("{:.1}M", a / 1e6)
    } else if a < 999.5e6 {
        format!("{:.0}M", a / 1e6)
    } else {
        format!("{:.1}B", a / 1e9)
    };
    format!("{sign}{body}")
}

/// A change figure per §2.2: one decimal under 10% ("+8.1%"), whole percent from 10%
/// ("+12%"), U+2212 minus; text2 when there is no change, else green / red.
fn pct_delta(c: &Canvas, d: f64) -> (String, Col) {
    let (t, col) = fmt_delta(c, d);
    if d.abs() < 9.95 {
        return (t, col);
    }
    (format!("{}{:.0}%", if d > 0.0 { "+" } else { "\u{2212}" }, d.abs()), col)
}

/// A live price: cents under 1,000 ("$248.31"), whole units with separators above
/// ("$96,412"), compact from a million.
fn quote(v: f64, cur: Option<&str>) -> String {
    let sym = match cur.unwrap_or("USD") {
        "USD" | "CAD" | "AUD" | "NZD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        "INR" => "₹",
        _ => return money(v, cur),
    };
    let a = v.abs();
    let sign = if v < 0.0 { "-" } else { "" };
    if a < 1000.0 {
        format!("{sign}{sym}{a:.2}")
    } else if a < 1e6 {
        format!("{sign}{sym}{}", super::grouped(a.round() as i64))
    } else {
        money(v, cur)
    }
}

/// A formatted amount back to a number ("$1,204.50" → 1204.5).
fn parse_amount(s: &str) -> Option<f64> {
    let t: String = s.chars().filter(|ch| ch.is_ascii_digit() || *ch == '.' || *ch == '-').collect();
    t.parse().ok()
}

/// "$50.00" → "$50".
fn trim_cents(s: &str) -> String {
    s.strip_suffix(".00").unwrap_or(s).to_string()
}

/// "2026-10-27" → "Oct 27".
fn short_date(ymd: &str) -> Option<String> {
    let mut it = ymd.split('-');
    let (_, m, d) = (it.next()?, it.next()?.parse::<usize>().ok()?, it.next()?.parse::<u32>().ok()?);
    Some(format!("{} {d}", MONTHS.get(m.checked_sub(1)?)?.get(..3)?))
}

/// The period in a caption: "Last 30 days" → "30 days", "Clicks, 28 days" → "28 days".
fn period_of(cap: &str) -> String {
    let t = cap.trim();
    let t = t.rsplit(", ").next().unwrap_or(t);
    t.strip_prefix("Last ").unwrap_or(t).to_string()
}

/// A period's shortest form: "30 days" → "30d", "24 hours" → "24h".
fn short_period(period: &str) -> String {
    let mut it = period.split_whitespace();
    match (it.next(), it.next(), it.next()) {
        (Some(n), Some(unit), None) if n.chars().all(|ch| ch.is_ascii_digit()) => format!("{n}{}", &unit[..1]),
        _ => period.to_string(),
    }
}

/// "30 days" → "30-day" (to lead a noun: "30-day visitors").
fn period_adj(period: &str) -> String {
    match period.strip_suffix(" days") {
        Some(n) => format!("{n}-day"),
        None => period.to_string(),
    }
}

/// A coin's letter tile: the first letter of its symbol on the badge's colours.
fn letter_badge(b: &Badge, symbol: &str) -> Badge {
    const LETTERS: [&str; 26] = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z"];
    match symbol.chars().next().map(|ch| ch.to_ascii_uppercase()) {
        Some(ch @ 'A'..='Z') => Badge { mark: LETTERS[(ch as u8 - b'A') as usize], glyph: false, ..*b },
        _ => *b,
    }
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

fn max_of(v: &[f64]) -> f64 {
    v.iter().copied().fold(f64::MIN, f64::max)
}

fn min_of(v: &[f64]) -> f64 {
    v.iter().copied().fold(f64::MAX, f64::min)
}

/// Up to three stats on the family's three-column grid (x 16 / 95 / 174): value 17/600
/// over a footnote label 16 below, by baseline. Fewer stats keep the same columns.
fn stats_row(c: &Canvas, items: &[(String, String)], value_base: f32) {
    let m = style::M_CARD;
    let cw = (c.w - 2.0 * m) / 3.0;
    for (i, (v, label)) in items.iter().take(3).enumerate() {
        let x = m + i as f32 * cw;
        let (v, fs) = fit_value(c, v, style::VALUE, cw - style::SP[3]);
        c.text_base(&v, x, value_base, fs, style::VALUE.weight, c.text1(), Align::Left, c.w);
        c.text_base(label, x, value_base + style::SP[6], style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, cw - style::SP[1]);
    }
}

// ------------------------------------------------------------------ Stat and Usage (Glance)

/// What a Glance face's Medium accessory box holds (x 148 to 252, y 48 to 80).
enum Boxed {
    None,
    /// The trend: smoothed, filled, with an end dot.
    Spark,
    /// A second value over its label, right-aligned.
    Stat(String, String),
    /// The last seven days as cells, weekday letters above.
    Week(Vec<bool>),
}

/// What a Glance face's Large adds under its separator.
enum Detail2 {
    None,
    /// The series as a history chart; `Some(label)` ends it at 172 with time labels
    /// ("13 days ago" … "Today").
    Chart(Option<String>),
    /// A shorter chart over a three-stat footer.
    ChartFooter(Vec<(String, String)>),
    /// Up to three rows (label, trailing value) and a footnote that closes the list.
    Rows(Vec<(String, String)>, String),
    /// The contribution heat map of the last 21 weeks (GitHub streak).
    Heat(Vec<f32>, usize),
    /// Followers: the next milestone and its meter, over the account's numbers in a footer
    /// (the box's number among them, so the box gives way).
    Social(Vec<(String, String)>, Option<(f32, String)>, Col),
    /// Three stats, then the paid / trial split, its footnote and (right) the stats' period.
    Subs(Vec<(String, String)>, f32, Col, String, String),
    /// Three stats, then a secondary pill that opens the credits page (OpenRouter).
    Credits(Vec<(String, String)>),
}

impl Detail2 {
    /// The Large detail shows the accessory's data again, so the box gives way to it.
    fn replaces_box(&self) -> bool {
        matches!(self, Detail2::Chart(_) | Detail2::ChartFooter(_) | Detail2::Heat(..) | Detail2::Social(..))
    }
}

/// How a Glance face reads at every size; the layouts below are shared.
struct Look {
    badge: Badge,
    /// The key value (Dock, Small and Medium), with an optional unit after it.
    value: String,
    unit: String,
    mini: Option<String>,
    /// The caption on dock sizes, and on Small and Medium.
    dock_cap: String,
    cap: String,
    delta: Option<f64>,
    delta_col: Option<Col>,
    series: Vec<f64>,
    /// Identity colour of the trend; None = by the change's sign.
    tint: Option<Col>,
    meter: Option<(f32, Col)>,
    /// Dock L's slot, and whether Small shows it too.
    acc: Acc,
    small_acc: bool,
    /// Medium's third line: an emphasised lead, then the rest (11, text2).
    third: Option<(String, String)>,
    boxed: Boxed,
    detail: Detail2,
}

impl Look {
    fn new(badge: Badge, value: String, cap: impl Into<String>) -> Self {
        let cap = cap.into();
        Look {
            badge,
            value,
            unit: String::new(),
            mini: None,
            dock_cap: cap.clone(),
            cap,
            delta: None,
            delta_col: None,
            series: Vec::new(),
            tint: None,
            meter: None,
            acc: Acc::Auto,
            small_acc: true,
            third: None,
            boxed: Boxed::None,
            detail: Detail2::None,
        }
    }

    fn trend(&self) -> Col {
        self.tint.unwrap_or_else(|| trend_col(self.delta))
    }

    /// Mini, Dock and Small, through the shared compact layouts.
    fn compact(&self, c: &Canvas, sh: Shape) {
        // Small takes the longer caption only when it fits whole; else the dock's.
        let small_fits = c.measure(&self.cap, style::CAPTION.size, style::CAPTION.weight, Font::Text).0 <= c.w - 2.0 * style::M_SMALL;
        let cap = if sh == Shape::Small && small_fits { &self.cap } else { &self.dock_cap };
        let mut s = Sum::new(Vis::Badge(self.badge), self.value.clone(), cap.clone());
        s.mini = self.mini.clone();
        s.unit = self.unit.clone();
        s.delta = self.delta;
        s.delta_col = self.delta_col;
        s.series = self.series.clone();
        s.tint = self.tint;
        s.meter = self.meter;
        s.inline_delta = false;
        if sh != Shape::Small || self.small_acc {
            s.acc = self.acc.clone();
        }
        compact_sum(c, &s, sh);
    }

    /// The Medium Glance block; Large draws it unchanged (bar a box its detail replaces).
    fn glance(&self, c: &Canvas, x: &Ctx, large: bool) {
        let m = style::M_CARD;
        let third = self.meter.map(|(f, col)| Line::Meter(f, col));
        glance(c, &self.badge, &self.value, &self.cap, third);
        if !self.unit.is_empty() {
            let bs = style::BADGE_GLANCE;
            let (v, fs) = fit_value(c, &self.value, style::HERO, c.w - 2.0 * m - bs - style::SP[3]);
            let w = c.measure(&v, fs, style::HERO.weight, Font::Text).0;
            c.text_base(&self.unit, m + w + style::SP[1], m + c.cap_h(fs), style::VALUE.size, 600, c.text2(), Align::Left, c.w);
        }
        // The change follows the caption on its baseline, the same size, 6 px after it.
        if let Some(d) = self.delta {
            let (t, col) = pct_delta(c, d);
            let cw = c.measure(&self.cap, style::CAPTION.size, style::CAPTION.weight, Font::Text).0;
            c.text_base(&t, m + cw + style::SP[2], 58.0, style::CAPTION.size, 600, self.delta_col.unwrap_or(col), Align::Left, c.w);
        }
        let show_box = !(large && self.detail.replaces_box());
        let boxed = show_box && !matches!(self.boxed, Boxed::None);
        if let Some((lead, rest)) = &self.third {
            let max = if boxed { ACC_X - m - style::SP[3] } else { c.w - 2.0 * m };
            let fs = style::FOOT.size;
            let lw = if lead.is_empty() { 0.0 } else { c.text_base(lead, m, 76.0, fs, 600, c.text2(), Align::Left, max) + style::SP[2] };
            c.text_base(rest, m + lw, 76.0, fs, 400, c.text2(), Align::Left, max - lw);
        }
        if !show_box {
            return;
        }
        let right = c.w - m;
        match &self.boxed {
            Boxed::None => {}
            Boxed::Spark => spark(c, ACC_X, ACC_TOP, right - ACC_X, 80.0 - ACC_TOP, &self.series, self.trend(), true),
            Boxed::Stat(v, l) => {
                c.text_base(v, right, 62.0, style::VALUE.size, style::VALUE.weight, c.text1(), Align::Right, right - ACC_X);
                c.text_base(l, right, 76.0, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Right, right - ACC_X);
            }
            Boxed::Week(days) => {
                let (cell, gap) = (12.0, 3.0);
                let x0 = week_cells(c, days, right, 62.0, cell, gap, 3.0, self.trend());
                // Weekday letters over the cells; the last cell is today.
                let n = days.len().min(7);
                for i in 0..n {
                    let wd = (x.now.weekday as usize + 7 * 2 - (n - 1 - i)) % 7;
                    let l = &"SMTWTFS"[wd..wd + 1];
                    let col = if i + 1 == n { c.text2() } else { c.text3() };
                    c.text_base(l, x0 + i as f32 * (cell + gap) + cell / 2.0, 56.0, style::MICRO.size, style::MICRO.weight, col, Align::Center, cell * 2.0);
                }
            }
        }
    }

    /// Large: the separator and the detail region (y 108 to 188).
    fn detail(&self, c: &Canvas, out: &mut Painted, x: &Ctx, open: &str) {
        let (rx, ry, rw, rh) = detail_region(c);
        let bottom = ry + rh;
        if !matches!(self.detail, Detail2::None) {
            separator(c, style::SEP_Y_LARGE);
        }
        let col = line_col(c, self.trend());
        match &self.detail {
            Detail2::None => {}
            Detail2::Chart(label) => {
                let h = if label.is_some() { 64.0 } else { rh };
                c.chart(rx, ry, rw, h, &self.series, col, ChartOpts { fill: true, dot: true, stroke: style::STROKE, smooth: self.series.len() >= 8 });
                if let Some(l) = label {
                    let (fs, wt) = (style::MICRO.size, style::MICRO.weight);
                    c.text_base(l, rx, bottom, fs, wt, c.text3(), Align::Left, rw / 2.0);
                    c.text_base("Today", rx + rw, bottom, fs, wt, c.text3(), Align::Right, rw / 2.0);
                }
            }
            Detail2::ChartFooter(items) => {
                c.chart(rx, ry, rw, CHART_FOOT_H, &self.series, col, ChartOpts { fill: true, dot: true, stroke: style::STROKE, smooth: self.series.len() >= 8 });
                stats_row(c, items, CHART_FOOT_BASE);
            }
            Detail2::Rows(items, summary) => {
                let items: Vec<(&str, &str)> = items.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
                // The first row's capitals on the region's top; the footnote on its bottom.
                rows(c, &items, ry - style::SP[3], style::ROW_L, 3);
                c.text_base(summary, rx, bottom - style::SP[0] - 1.0, style::FOOT.size, 500, c.text3(), Align::Left, rw);
            }
            Detail2::Heat(grid, streak) => heat_weeks(c, grid, *streak, x.now.weekday as usize, self.trend()),
            Detail2::Social(items, next, meter) => {
                if let Some((frac, label)) = next {
                    let fs = style::TITLE.size;
                    let lw = c.text_base(label, rx + rw, SOCIAL_HEAD_BASE, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Right, rw / 2.0);
                    c.text_base("Next milestone", rx, SOCIAL_HEAD_BASE, fs, style::TITLE.weight, c.text1(), Align::Left, rw - lw - style::SP[3]);
                    let mh = style::METER_H;
                    c.rr(rx, SOCIAL_METER_Y, rw, mh, mh / 2.0, c.track());
                    c.rr(rx, SOCIAL_METER_Y, (rw * frac.clamp(0.0, 1.0)).max(mh), mh, mh / 2.0, *meter);
                }
                stats_row(c, items, HDR_FOOT_BASE);
            }
            Detail2::Subs(items, paid, red, note, period) => {
                stats_row(c, items, SUBS_FOOT_BASE);
                // Paid in the badge colour, trials in the same colour, lighter, after it.
                let (my, mh) = (SUBS_METER_Y, style::METER_H);
                if *paid > 0.0 {
                    c.rr(rx, my, rw, mh, mh / 2.0, alpha(*red, 0.35));
                    c.rr(rx, my, (rw * paid.clamp(0.0, 1.0)).max(mh), mh, mh / 2.0, *red);
                }
                // The stats' period once, at the right of the footnote line.
                let pw = if period.is_empty() { 0.0 } else { c.text_base(period, rx + rw, SUBS_NOTE_BASE, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Right, rw / 2.0) + style::SP[5] };
                c.text_base(note, rx, SUBS_NOTE_BASE, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, rw - pw);
            }
            Detail2::Credits(items) => {
                stats_row(c, items, SUBS_FOOT_BASE);
                // A secondary pill on the bottom margin.
                let ph = style::BTN_SECONDARY;
                let py = bottom - ph;
                c.rr(rx, py, rw, ph, ph / 2.0, c.plate());
                let fs = style::TITLE.size;
                c.text_cap("Add credits", rx + rw / 2.0, py + (ph - c.cap_h(fs)) / 2.0, fs, 600, c.text1(), Align::Center, rw);
                if !open.is_empty() {
                    out.hits.push(((rx, py, rw, ph), WAct::Open(open.to_string())));
                }
            }
        }
    }
}

/// GitHub's last 21 weeks as a heat map filling the detail region edge to edge (x 16 to
/// 252, gap 2): the current streak at full intensity, every other active day capped at the
/// three lower steps so the run stands out, today ringed inside its own cell.
fn heat_weeks(c: &Canvas, grid: &[f32], streak: usize, weekday: usize, col: Col) {
    let (rx, ry, rw, rh) = detail_region(c);
    const WEEKS: usize = 21;
    let gap = style::HEAT_GAP;
    let cell = ((rw - (WEEKS - 1) as f32 * gap) / WEEKS as f32).min((rh - 6.0 * gap) / 7.0);
    let pitch = cell + gap;
    let (gw, gh) = (WEEKS as f32 * pitch - gap, 7.0 * pitch - gap);
    let (x0, y0) = (rx + (rw - gw) / 2.0, ry + (rh - gh) / 2.0);
    let r = 2.0;
    let n = grid.len();
    if n == 0 {
        // Loading: the whole grid in the track colour.
        for j in 0..WEEKS {
            for row in 0..7 {
                c.rr(x0 + j as f32 * pitch, y0 + row as f32 * pitch, cell, cell, r, c.track());
            }
        }
        return;
    }
    for j in 0..WEEKS {
        for row in 0..7 {
            let ago = (WEEKS - 1 - j) * 7 + weekday;
            let Some(ago) = ago.checked_sub(row) else { continue };
            if ago >= n {
                continue;
            }
            let v = grid[n - 1 - ago];
            let (x, y) = (x0 + j as f32 * pitch, y0 + row as f32 * pitch);
            let fill = if ago < streak && v > 0.0 {
                col
            } else if v <= 0.0 {
                c.track()
            } else {
                let step = ((v * 3.0).ceil() as usize).clamp(1, 3) - 1;
                alpha(col, style::HEAT_STEPS[step])
            };
            c.rr(x, y, cell, cell, r, fill);
            if ago == 0 {
                // Inside the cell, so the grid's right edge stays on the margin.
                c.rr_stroke(x + 0.75, y + 0.75, cell - 1.5, cell - 1.5, r, c.text1(), 1.5);
            }
        }
    }
}

/// The last seven days of a contribution grid: active or not.
fn last_week(grid: &[f32]) -> Vec<bool> {
    grid.iter().rev().take(7).rev().map(|v| *v > 0.0).collect()
}

fn stat_look(c: &Canvas, def: &Def, conn: &str, f: &Feed, badge: &Badge) -> Look {
    let ex = |k: &str| f.extra.get(k).cloned();
    let v = f.value.unwrap_or(0.0);
    let cur = f.currency.as_deref();
    match conn {
        "revenuecat" => {
            let mut l = Look::new(*badge, headline(f), "MRR");
            l.mini = Some(mini_money(v, cur));
            let subs = ex("active_subscriptions");
            let trials = ex("active_trials");
            if let Some(s) = &subs {
                l.cap = format!("MRR · {s} subscribers");
                l.acc = Acc::Second(s.clone(), "subscribers".into());
            }
            if let Some(t) = &trials {
                l.boxed = Boxed::Stat(t.clone(), "Trials".into());
            }
            let items: Vec<(String, String)> = [("revenue", "Revenue"), ("new_customers", "New"), ("active_users", "Active users")]
                .iter()
                .filter_map(|(k, label)| ex(k).map(|v| (trim_cents(&v), label.to_string())))
                .collect();
            let (p, t) = (subs.as_deref().and_then(parse_amount).unwrap_or(0.0), trials.as_deref().and_then(parse_amount).unwrap_or(0.0));
            if p + t > 0.0 {
                let note = match (&subs, &trials) {
                    (Some(s), Some(t)) => format!("{s} paid · {t} in trial"),
                    (Some(s), None) => format!("{s} paid"),
                    _ => String::new(),
                };
                // RevenueCat's overview counts revenue, new customers and active users over
                // the last 28 days: said once, beside the footnote.
                let period = if items.is_empty() { String::new() } else { "Last 28 days".into() };
                l.detail = Detail2::Subs(items, (p / (p + t)) as f32, badge.bg, note, period);
            } else if !items.is_empty() {
                l.detail = Detail2::Subs(items, 0.0, badge.bg, "Last 28 days".into(), String::new());
            }
            l
        }
        "cursor" => {
            let mut l = Look::new(*badge, headline(f), "This cycle");
            l.mini = Some(mini_money(v, cur));
            let since = ex("cycle_start").and_then(|d| short_date(&d));
            if let Some(s) = &since {
                l.cap = format!("This cycle · since {s}");
            }
            let od = ex("on_demand").filter(|s| parse_amount(s).unwrap_or(0.0) > 0.0);
            let members = ex("members");
            l.acc = match (&od, &members) {
                (Some(o), _) => Acc::Second(o.clone(), "on-demand".into()),
                (None, Some(n)) => Acc::Second(n.clone(), "members".into()),
                _ => Acc::Auto,
            };
            let mut parts = Vec::new();
            if let Some(o) = &od {
                parts.push(format!("{o} on-demand"));
            }
            if let Some(n) = &members {
                parts.push(format!("{n} {}", if n == "1" { "member" } else { "members" }));
            }
            if !parts.is_empty() {
                l.third = Some((String::new(), parts.join(" · ")));
            }
            let n = f.rows.len();
            if n > 0 {
                let shown = n.min(3);
                let items = f.rows.iter().take(shown).map(|r| (r.label.clone(), r.value.clone())).collect();
                let summary = if n > shown {
                    let rest: f64 = f.rows.iter().skip(shown).filter_map(|r| parse_amount(&r.value)).sum();
                    format!("and {} more · {}", n - shown, money(rest, cur))
                } else {
                    let mut s = Vec::new();
                    if let Some(o) = &od {
                        s.push(format!("{o} on-demand"));
                    }
                    if let Some(d) = &since {
                        s.push(format!("since {d}"));
                    }
                    s.join(" · ")
                };
                l.detail = Detail2::Rows(items, summary);
            }
            l
        }
        "gmail" => {
            let n = v.round() as i64;
            let cap = if n == 0 { "All caught up" } else { "Unread" };
            let mut l = Look::new(*badge, super::grouped(n), cap);
            l.mini = Some(mini_count(v));
            if let Some(r) = f.rows.first().filter(|_| n > 0) {
                let first = r.value.split_whitespace().next().unwrap_or("").to_string();
                l.acc = Acc::Note(r.value.clone(), r.label.clone());
                l.small_acc = !first.is_empty();
                l.third = Some((r.value.clone(), r.label.clone()));
            }
            // Large's rows start at the second-newest: the newest is the third line above.
            let skip = if l.third.is_some() { 1 } else { 0 };
            let items: Vec<(String, String)> = f.rows.iter().filter(|_| n > 0).skip(skip).take(3).map(|r| (r.label.clone(), r.value.clone())).collect();
            let mut summary = vec![format!("{} unread", super::grouped(n))];
            if let Some(t) = ex("total") {
                summary.push(format!("{t} in inbox"));
            }
            l.detail = Detail2::Rows(items, summary.join(" · "));
            l
        }
        "github" => {
            let streak = ex("streak").unwrap_or_else(|| "0".into());
            let days: usize = streak.parse().unwrap_or(0);
            let cap = if days == 0 { "Start a streak today" } else { "Current streak" };
            let mut l = Look::new(*badge, streak.clone(), cap);
            l.unit = if days == 1 { "day".into() } else { "days".into() };
            l.mini = Some(streak);
            l.tint = ident(c, conn);
            if !f.grid.is_empty() {
                l.acc = Acc::Week(last_week(&f.grid));
                l.boxed = Boxed::Week(last_week(&f.grid));
                l.detail = Detail2::Heat(f.grid.clone(), days);
            }
            l
        }
        "x" | "tiktok" | "youtube" | "instagram" => {
            let cap = if f.caption.is_empty() { "Followers".to_string() } else { f.caption.clone() };
            let mut l = Look::new(*badge, super::grouped(v.round() as i64), cap);
            l.mini = Some(mini_count(v));
            if let Some(n) = ex("name") {
                l.third = Some((String::new(), n));
            }
            // Medium's accessory: the account's second number.
            let (second, label) = match conn {
                "x" | "instagram" => ("posts", "Posts"),
                "tiktok" => ("likes", "Likes"),
                _ => ("views", "Views"),
            };
            if let Some(sv) = ex(second) {
                l.acc = Acc::Second(sv.clone(), label.into());
                l.boxed = Boxed::Stat(sv, label.into());
            }
            // Large: the account's own numbers in the footer, the box's first (so the box
            // gives way), headed by the next follower milestone.
            let mut items: Vec<(String, String)> = Vec::new();
            let keys: &[(&str, &str)] = match conn {
                "x" => &[("posts", "Posts"), ("following", "Following"), ("listed", "Listed")],
                "tiktok" => &[("likes", "Likes"), ("videos", "Videos"), ("following", "Following")],
                "youtube" => &[("views", "Views"), ("videos", "Videos")],
                _ => &[("posts", "Posts"), ("following", "Following")],
            };
            for (k, label) in keys {
                if let Some(s) = ex(k) {
                    items.push((s, label.to_string()));
                }
            }
            if conn == "youtube" {
                // Views per video: the one derived number creators already track.
                let n = |k: &str| ex(k).and_then(|s| parse_compact(&s));
                if let (Some(views), Some(videos)) = (n("views"), n("videos")) {
                    if videos > 0.0 {
                        items.push((compact_tight((views / videos).round()), "Avg views".into()));
                    }
                }
            }
            items.truncate(3);
            let next = milestone(v).map(|(frac, target)| (frac, format!("{} to {}", super::grouped((target - v).ceil() as i64), compact_tight(target))));
            if !items.is_empty() || next.is_some() {
                // Brand tiles that are black (X, TikTok) fill the meter with the text ink.
                let (r, g, b) = (badge.bg.0, badge.bg.1, badge.bg.2);
                let meter = if 0.2126 * r + 0.7152 * g + 0.0722 * b < 0.2 { c.text1() } else { badge.bg };
                l.detail = Detail2::Social(items, next, meter);
            }
            l
        }
        _ => {
            // Totals with a 30-day trend: revenue, visitors, clicks, emails.
            let value = match (&f.text, conn) {
                (Some(t), "resend") => t.clone(),
                _ if f.currency.is_none() && f.value.is_some() => super::grouped(v.round() as i64),
                _ => headline(f),
            };
            let caption = if f.caption.is_empty() { def.name.to_string() } else { f.caption.clone() };
            let mut l = Look::new(*badge, value, caption.clone());
            l.mini = Some(if f.currency.is_some() { mini_money(v, cur) } else { mini_count(v) });
            l.delta = f.delta_pct;
            l.series = f.series.clone();
            l.tint = ident(c, conn);
            let period = period_of(&caption);
            match conn {
                "plausible" | "umami" => {
                    l.dock_cap = "Visitors".into();
                    l.cap = format!("{} visitors", period_adj(&period));
                }
                "search_console" => {
                    l.dock_cap = "Clicks".into();
                    l.cap = format!("Clicks · {period}");
                }
                "resend" => {
                    // More emails is not better or worse: the change stays neutral.
                    l.delta_col = Some(c.text2());
                    l.dock_cap = "Emails sent".into();
                    l.cap = if f.series.len() > 2 { format!("Emails sent · {} days", f.series.len()) } else { "Emails sent".into() };
                }
                _ => {}
            }
            if l.series.len() > 2 {
                l.boxed = Boxed::Spark;
                l.detail = Detail2::Chart(None);
                let s = &l.series;
                let items: Vec<(String, String)> = match conn {
                    "search_console" => [("impressions", "Impressions"), ("ctr", "CTR"), ("position", "Avg position")].iter().filter_map(|(k, label)| ex(k).map(|v| (v, label.to_string()))).collect(),
                    "resend" => {
                        let week: f64 = s.iter().rev().take(7).sum();
                        vec![(super::grouped(s[s.len() - 1] as i64), "Today".into()), (super::grouped(week as i64), "7 days".into()), (compact(mean(s)), "Daily avg".into())]
                    }
                    _ => Vec::new(),
                };
                if !items.is_empty() {
                    l.detail = Detail2::ChartFooter(items);
                }
            }
            l
        }
    }
}

/// The next follower milestone on a 1 / 2.5 / 5 ladder from 100 (2,400 → 2.5K, 12,400 →
/// 25K), and how far `v` has come from the one before it.
fn milestone(v: f64) -> Option<(f32, f64)> {
    if !v.is_finite() || v < 0.0 {
        return None;
    }
    let (mut prev, mut k) = (0.0, 100.0);
    while k < 1e12 {
        for m in [1.0, 2.5, 5.0] {
            let t = m * k;
            if t > v {
                return Some((((v - prev) / (t - prev)) as f32, t));
            }
            prev = t;
        }
        k *= 10.0;
    }
    None
}

/// "1.2M" → 1200000, "8,900" → 8900.
fn parse_compact(s: &str) -> Option<f64> {
    let t = s.trim();
    let (num, k) = match t.chars().last()? {
        'K' => (&t[..t.len() - 1], 1e3),
        'M' => (&t[..t.len() - 1], 1e6),
        'B' => (&t[..t.len() - 1], 1e9),
        _ => (t, 1.0),
    };
    num.replace(',', "").parse::<f64>().ok().map(|v| v * k)
}

#[allow(clippy::too_many_arguments)]
fn stat(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, open: &str, sh: Shape) {
    let g = got(x, conn);
    let tpl = match conn {
        "revenuecat" => Tpl::Glance { acc: Sk::Stat, boxed: Sk::Stat, third: false, detail: Detail::Subs },
        "x" | "tiktok" | "youtube" | "instagram" => Tpl::Glance { acc: Sk::Stat, boxed: Sk::Stat, third: true, detail: Detail::Footer(3) },
        "cursor" => Tpl::Glance { acc: Sk::Stat, boxed: Sk::None, third: true, detail: Detail::Rows },
        "gmail" => Tpl::Glance { acc: Sk::Note, boxed: Sk::None, third: true, detail: Detail::Rows },
        "github" => Tpl::Glance { acc: Sk::Week, boxed: Sk::Week, third: false, detail: Detail::Heat },
        "search_console" | "resend" => Tpl::Glance { acc: Sk::Trend, boxed: Sk::Trend, third: false, detail: Detail::ChartFooter },
        _ => Tpl::Glance { acc: Sk::Trend, boxed: Sk::Trend, third: false, detail: Detail::Chart },
    };
    if placeholder_tpl(c, out, def.name, badge, conn, &g, sh, &tpl) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, open);
    let look = stat_look(c, def, conn, &f, badge);
    if compactish(sh) {
        look.compact(c, sh);
        return;
    }
    look.glance(c, x, sh == Shape::Large);
    if sh == Shape::Large {
        look.detail(c, out, x, open);
    }
}

#[allow(clippy::too_many_arguments)]
fn usage(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, open: &str, sh: Shape) {
    let g = got(x, conn);
    let credits = conn == "openrouter";
    let tpl = if credits {
        Tpl::Glance { acc: Sk::Trend, boxed: Sk::None, third: false, detail: Detail::Credits }
    } else {
        Tpl::Glance { acc: Sk::Trend, boxed: Sk::Trend, third: true, detail: Detail::Chart }
    };
    if placeholder_tpl(c, out, def.name, badge, conn, &g, sh, &tpl) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, open);
    let v = f.value.unwrap_or(0.0);
    let look = if f.unit == "tokens" {
        // Tokens today, with the last two weeks as the trend (no meter: there is no limit).
        let mut l = Look::new(*badge, compact(v), "tokens today");
        l.mini = Some(mini_count(v));
        l.series = f.series.clone();
        l.tint = ident(c, conn).or(Some(CLAUDE));
        let num = |k: &str| f.extra.get(k).and_then(|s| s.parse::<f64>().ok());
        let mut parts = Vec::new();
        if let Some(w5) = num("tokens_5h") {
            parts.push(format!("5h {}", compact(w5)));
        }
        if let Some(wk) = num("tokens_week") {
            parts.push(format!("week {}", compact(wk)));
        }
        if !parts.is_empty() {
            l.third = Some((String::new(), parts.join(" · ")));
        }
        if l.series.len() > 2 {
            l.boxed = Boxed::Spark;
            l.detail = Detail2::Chart(Some(format!("{} days ago", l.series.len() - 1)));
        }
        l
    } else {
        // Credits left: the meter shows what is left, and warns like a battery.
        let mut l = Look::new(*badge, headline(&f), if f.caption.is_empty() { "Credits left".to_string() } else { f.caption.clone() });
        l.mini = Some(mini_money(v, f.currency.as_deref()));
        if let Some(used) = f.frac {
            let left = (1.0 - used).clamp(0.0, 1.0);
            let col = if left <= 0.1 {
                RED
            } else if left <= 0.2 {
                ORANGE
            } else if credits {
                INDIGO
            } else {
                ident(c, conn).unwrap_or(BLUE)
            };
            l.meter = Some((left, col));
            let mut items = Vec::new();
            if let Some(u) = f.extra.get("total_usage") {
                items.push((trim_cents(u), "Used".to_string()));
            }
            if let Some(t) = f.extra.get("total_credits") {
                items.push((trim_cents(t), "Limit".to_string()));
            }
            items.push((format!("{:.0}%", left * 100.0), "Left".to_string()));
            l.detail = Detail2::Credits(items);
        }
        l
    };
    if compactish(sh) {
        look.compact(c, sh);
        return;
    }
    look.glance(c, x, sh == Shape::Large);
    if sh == Shape::Large {
        look.detail(c, out, x, open);
    }
}

// ------------------------------------------------------------------ Chart (Header)

/// How a Chart face plots its series.
#[derive(Clone, Copy, PartialEq)]
enum Plot {
    /// Daily quantities: bars from zero (revenue, visitors, tokens, requests).
    Bars,
    /// A running total that ends at the headline (month-to-date spend).
    Cumulative,
    /// A price: a line, with a dashed reference at the period's start.
    Quote(f64),
}

struct ChartLook {
    badge: Badge,
    /// Header title (the only naming on the card), and the line under it.
    title: String,
    sub: String,
    /// Small's header label ("30 days").
    period: String,
    value: String,
    mini: String,
    dock_cap: String,
    /// Dock M shows the change after the caption (prices).
    inline_delta: bool,
    delta: Option<f64>,
    col: Col,
    tint: Option<Col>,
    plot: Plot,
    series: Vec<f64>,
    footer: Vec<(String, String)>,
}

impl ChartLook {
    /// What the plot draws: the running total for Cumulative, else the series.
    fn plotted(&self) -> Vec<f64> {
        match self.plot {
            Plot::Cumulative => self
                .series
                .iter()
                .scan(0.0, |acc, v| {
                    *acc += if v.is_finite() { *v } else { 0.0 };
                    Some(*acc)
                })
                .collect(),
            _ => self.series.clone(),
        }
    }

    /// The plot in a box (bars on a baseline, or a line with its end dot).
    fn plot(&self, c: &Canvas, x: f32, y: f32, w: f32, h: f32, compact: bool) {
        let vals = self.plotted();
        match self.plot {
            Plot::Bars => day_bars(c, x, y, w, h, &vals, self.col, true),
            Plot::Cumulative | Plot::Quote(_) => {
                let stroke = if h >= 40.0 { style::STROKE } else { style::STROKE_SPARK };
                if let Plot::Quote(r) = self.plot {
                    if let Some(ry) = chart_y(&vals, y, h, stroke, true, r) {
                        dashed(c, x, x + w - style::CHART_DOT - 1.5, ry.round() + 0.5, alpha(c.text3(), 0.6));
                    }
                } else if !compact {
                    base_line(c, x, w, y + h);
                }
                let vals = if compact { thin(&vals, 24) } else { vals };
                c.chart(x, y, w, h, &vals, line_col(c, self.col), ChartOpts { fill: true, dot: true, stroke, smooth: vals.len() >= 5 });
            }
        }
    }
}

fn chart_look(c: &Canvas, def: &Def, x: &Ctx, conn: &str, f: &Feed, badge: &Badge) -> ChartLook {
    let v = f.value.unwrap_or(0.0);
    let cur = f.currency.as_deref();
    let s = &f.series;
    let last = s.last().copied().unwrap_or(0.0);
    let symbol = f.extra.get("symbol").cloned();
    let caption = if f.caption.is_empty() { "Last 30 days".to_string() } else { f.caption.clone() };
    let mut l = ChartLook {
        badge: *badge,
        title: def.name.trim_end_matches(" Chart").to_string(),
        sub: caption.clone(),
        period: period_of(&caption),
        value: headline(f),
        mini: if cur.is_some() { mini_money(v, cur) } else { mini_count(v) },
        dock_cap: caption.clone(),
        inline_delta: false,
        delta: f.delta_pct,
        col: ident(c, conn).unwrap_or_else(|| trend_col(f.delta_pct)),
        tint: ident(c, conn),
        plot: Plot::Bars,
        series: s.clone(),
        footer: Vec::new(),
    };
    let fmt = |v: f64| if cur.is_some() { money(v, cur) } else { compact(v) };
    if !s.is_empty() {
        l.footer = vec![(fmt(last), "Today".into()), (fmt(mean(s)), "Daily avg".into()), (fmt(max_of(s)), "Best day".into())];
    }
    match conn {
        "claude_code" => {
            let n = s.len();
            let total: f64 = s.iter().sum();
            l.title = def.name.to_string();
            l.sub = format!("{n} days");
            l.period = l.sub.clone();
            l.dock_cap = l.sub.clone();
            l.value = compact(total);
            l.mini = mini_count(total);
            if !s.is_empty() {
                l.footer = vec![(compact(last), "Today".into()), (compact(mean(s)), "Daily avg".into()), (compact(max_of(s)), "Peak".into())];
            }
        }
        "anthropic" | "openai" => {
            l.title = def.name.to_string();
            l.plot = Plot::Cumulative;
            l.period = caption.clone();
            let days = s.len().max(1) as f64;
            let in_month = match x.now.month {
                2 if x.now.year % 4 == 0 && (x.now.year % 100 != 0 || x.now.year % 400 == 0) => 29.0,
                2 => 28.0,
                4 | 6 | 9 | 11 => 30.0,
                _ => 31.0,
            };
            let avg = v / days;
            l.footer = vec![(money(last, cur), "Today".into()), (money(avg, cur), "Daily avg".into()), (money(avg * in_month, cur), "Projected".into())];
        }
        "plausible" | "umami" => {
            l.title = "Visitors".into();
            l.value = super::grouped(v.round() as i64);
            l.dock_cap = "Visitors".into();
            l.footer = vec![(super::grouped(last as i64), "Today".into()), (super::grouped(mean(s).round() as i64), "Daily avg".into()), (super::grouped(max_of(s) as i64), "Best day".into())];
        }
        "supabase" => {
            l.title = "API requests".into();
            l.sub = l.period.clone();
            l.dock_cap = "API requests".into();
            // The usage API returns sub-day buckets over 7 days: summed into 7 daily bars,
            // so every number on the card has a unit ("Daily avg", not a 6-hour average).
            let n = s.len();
            if n >= 7 && n % 7 == 0 {
                let days: Vec<f64> = s.chunks(n / 7).map(|ch| ch.iter().filter(|v| v.is_finite()).sum()).collect();
                l.footer = vec![(compact(days[6]), "Last 24h".into()), (compact(mean(&days)), "Daily avg".into()), (compact(max_of(&days)), "Peak day".into())];
                l.series = days;
            } else {
                l.footer = vec![(compact(last), "Latest".into()), (compact(mean(s)), "Average".into()), (compact(max_of(s)), "Peak".into())];
            }
        }
        "stocks" | "crypto" => {
            let sym = symbol.unwrap_or_else(|| l.title.clone());
            let name = f.extra.get("name").cloned().or_else(|| Some(f.caption.clone()).filter(|c| !c.is_empty() && *c != sym));
            l.title = sym.clone();
            l.sub = name.unwrap_or_default();
            l.value = quote(v, cur);
            l.dock_cap = sym.clone();
            l.inline_delta = true;
            let first = s.first().copied().unwrap_or(v);
            if conn == "stocks" {
                l.period = "Today".into();
                let prev = f.delta_pct.map(|d| v / (1.0 + d / 100.0)).unwrap_or(first);
                l.plot = Plot::Quote(prev);
                l.footer = vec![(quote(prev, cur), "Prev close".into()), (quote(max_of(s).max(v), cur), "Day high".into()), (quote(min_of(s).min(v), cur), "Day low".into())];
            } else {
                l.period = "24 hours".into();
                l.badge = letter_badge(badge, &sym);
                l.plot = Plot::Quote(first);
                l.footer = vec![(quote(max_of(s).max(v), cur), "24h high".into()), (quote(min_of(s).min(v), cur), "24h low".into()), (quote(first, cur), "24h open".into())];
            }
            if s.is_empty() {
                l.footer.clear();
            }
        }
        _ => {}
    }
    l
}

#[allow(clippy::too_many_arguments)]
fn chart(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, open: &str, sh: Shape) {
    let g = got(x, conn);
    // The loading skeleton names the card the way the loaded header will.
    let title = match conn {
        "plausible" | "umami" => "Visitors".to_string(),
        "supabase" => "API requests".to_string(),
        "claude_code" | "anthropic" | "openai" => def.name.to_string(),
        _ => match &g {
            Got::Feed(f) => f.extra.get("symbol").cloned().unwrap_or_else(|| def.name.trim_end_matches(" Chart").to_string()),
            _ => def.name.trim_end_matches(" Chart").to_string(),
        },
    };
    // A coin is a letter tile in every state: before it loads, the first letter of the
    // first coin in its settings (CoinGecko's default list starts with bitcoin).
    let coin = (conn == "crypto").then(|| {
        let coins = x.cfg.widget_settings.get("crypto").and_then(|p| p.get("coins")).map(|s| s.as_str()).unwrap_or("");
        let first = coins.split(|ch: char| ch == ',' || ch.is_whitespace()).find(|s| !s.is_empty()).unwrap_or("bitcoin");
        letter_badge(badge, first)
    });
    let wait_badge = coin.as_ref().unwrap_or(badge);
    if placeholder_tpl(c, out, def.name, wait_badge, conn, &g, sh, &Tpl::Header(title)) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, open);
    let l = chart_look(c, def, x, conn, &f, badge);
    match sh {
        Shape::Mini | Shape::Dock => {
            let mut s = Sum::new(Vis::Badge(l.badge), l.value.clone(), l.dock_cap.clone());
            s.mini = Some(l.mini.clone());
            s.delta = l.delta;
            s.series = l.plotted();
            s.tint = l.tint;
            s.bars = l.plot == Plot::Bars && s.series.len() <= 16;
            s.inline_delta = l.inline_delta;
            compact_sum(c, &s, sh);
        }
        Shape::Small => {
            // Chart-led: badge, period and change on top, the value, then the chart.
            let m = style::M_SMALL;
            let bs = style::BADGE_HEADER;
            let right = c.w - m;
            c.badge(&l.badge, m, m, bs);
            // Tight (4) gaps so "30 days" and a change like "+8.1%" share the row whole.
            let lx = m + bs + style::SP[1];
            let fs = style::FOOT.size;
            let dw = match l.delta {
                Some(d) => {
                    let (t, col) = pct_delta(c, d);
                    c.text_base(&t, right, 28.0, fs, 600, col, Align::Right, right - lx) + style::SP[1]
                }
                None => 0.0,
            };
            // The period beside the change ("30 days"); its short form ("24h") only when
            // both won't fit whole.
            let avail = right - lx - dw;
            let period = if c.measure(&l.period, fs, 400, Font::Text).0 <= avail { l.period.clone() } else { short_period(&l.period) };
            c.text_base(&period, lx, 28.0, fs, 400, c.text2(), Align::Left, avail);
            let (v, vfs) = fit_value(c, &l.value, style::HERO_S, right - m);
            c.text_base(&v, m, 62.0, vfs, style::HERO_S.weight, c.text1(), Align::Left, c.w * 2.0);
            if l.series.len() > 1 {
                l.plot(c, m, 72.0, right - m, c.h - m - 72.0, true);
            }
        }
        _ => {
            // Header: badge, title and value on one baseline; the period (or the name)
            // and the change under them; the plot below.
            let m = style::M_CARD;
            let bs = style::BADGE_HEADER;
            let right = c.w - m;
            c.badge(&l.badge, m, m, bs);
            let tx = m + bs + style::SP[3];
            let (v, vfs) = fit_value(c, &l.value, style::VALUE, (right - tx) * 0.5);
            let vw = c.text_base(&v, right, style::HEADER_BASE, vfs, style::VALUE.weight, c.text1(), Align::Right, c.w);
            c.text_base(&l.title, tx, style::HEADER_BASE, style::TITLE.size, style::TITLE.weight, c.text1(), Align::Left, right - vw - style::SP[5] - tx);
            let line2 = style::HEADER_BASE + 14.0;
            let fs = style::FOOT.size;
            let dw = match l.delta {
                Some(d) => {
                    let (t, col) = pct_delta(c, d);
                    c.text_base(&t, right, line2, fs, 600, col, Align::Right, c.w) + style::SP[5]
                }
                None => 0.0,
            };
            c.text_base(&l.sub, tx, line2, fs, 400, c.text2(), Align::Left, right - dw - tx);
            let large = sh == Shape::Large;
            if l.series.len() < 2 {
                c.text_base("No chart data yet", m, 70.0, style::CAPTION.size, 400, c.text3(), Align::Left, right - m);
                return;
            }
            let week = matches!(conn, "claude_code" | "supabase") && large && l.series.len() <= 16;
            let bottom = if !large {
                c.h - m
            } else if week {
                HDR_PLOT_BOTTOM_L - style::SP[3]
            } else {
                HDR_PLOT_BOTTOM_L
            };
            l.plot(c, m, HDR_PLOT_TOP, right - m, bottom - HDR_PLOT_TOP, false);
            if week {
                // Weekday letters under the bars; today's in the bar colour.
                let n = l.series.len();
                let pitch = (right - m) / n as f32;
                for i in 0..n {
                    let wd = (x.now.weekday as usize + 7 * 3 - (n - 1 - i)) % 7;
                    let col = if i + 1 == n { line_col(c, l.col) } else { c.text3() };
                    c.text_base(&"SMTWTFS"[wd..wd + 1], m + pitch * (i as f32 + 0.5), HDR_PLOT_BOTTOM_L + style::SP[2], style::MICRO.size, style::MICRO.weight, col, Align::Center, pitch);
                }
            }
            if large {
                stats_row(c, &l.footer, HDR_FOOT_BASE);
            }
        }
    }
}

// ------------------------------------------------------------------ rich data cards
// heat, list, watch, next_event, ai_all, goal and wallpaper, plus the pieces only they use
// (names start with `rd_`). Layouts follow docs/widget-design.md §3 with style.rs tokens.

/// First-row baselines for list rows: Small rows sit at 70/88/106, Header-template rows
/// start at 60 (Medium 60/80, Large 60…156 at the 24 pitch).
const RD_ROW1_S: f32 = 70.0;
const RD_ROW1: f32 = 60.0;
/// An inline row meter and the value column to its right are each this wide.
const RD_METER_W: f32 = 56.0;
/// Status dots in rows, pills and capsules.
const RD_DOT: f32 = 6.0;
/// Change capsules: height, and width on Small / Medium and Large.
const RD_CAPSULE_H: f32 = 18.0;
const RD_CAPSULE_S: f32 = 52.0;
const RD_CAPSULE_M: f32 = 56.0;
/// Footnote weight for emphasised short labels ("in 42 min").
const RD_FOOT_SB: Type = Type { weight: 600, ..style::FOOT };
/// Near-black label on white glass buttons over photos.
const RD_ON_GLASS: Col = (0.1, 0.1, 0.12, 1.0);
const RD_GLASS: Col = (1.0, 1.0, 1.0, 0.92);

/// What a rich card's Dock L pill adds in its accessory slot (§3 Dock L).
enum RdAcc {
    None,
    /// A value over a label, right-aligned: value, its colour, its type role, label.
    Pair(String, Col, Type, String),
    /// A label over a 4 px meter.
    Meter(f32, Col, String),
    /// A label over one meter split into coloured shares (fractions of the whole).
    Stack(Vec<(f32, Col)>, String),
    /// A status dot and a label.
    Dot(Col, String),
    /// The last 7 days as heat cells over weekday initials: values (oldest first), colour,
    /// today's weekday (0 = Sunday).
    Week(Vec<f32>, Col, usize),
    /// A label (11 text2) over a sparkline in the colour (1.5 stroke, no fill).
    Spark(Vec<f64>, Col, String),
}

/// The text of a rich Dock pill.
struct DockText<'a> {
    value: &'a str,
    /// The value is a name (an event, a file): Title 13/600, may truncate. Otherwise it is a
    /// Value 17/600 that is never cut.
    name: bool,
    /// A status dot before the value.
    dot: Option<Col>,
    /// Micro suffix after the value ("PM").
    suffix: &'a str,
    caption: &'a str,
    /// A coloured figure after the caption ("+0.6%").
    tag: Option<(&'a str, Col)>,
}

impl<'a> DockText<'a> {
    fn new(value: &'a str, caption: &'a str) -> Self {
        DockText { value, name: false, dot: None, suffix: "", caption, tag: None }
    }
}

/// The largest size from `start` down to `min` at which `s` fits in `max_w`, measured at
/// each step (optical sizes make width non-linear in size); `min` if nothing fits.
fn rd_fit(c: &Canvas, s: &str, start: f32, min: f32, weight: u32, max_w: f32) -> f32 {
    let mut size = start;
    while size > min && c.measure(s, size, weight, Font::Text).0 > max_w {
        size = (size - 0.5).max(min);
    }
    size
}

/// Dock M (badge, value, caption) and Dock L (plus `acc` in the accessory slot).
fn rd_dock(c: &Canvas, vis: &Vis, d: DockText, acc: RdAcc) {
    let k = c.h / 54.0;
    let m = style::M_DOCK * k;
    let bs = style::BADGE_DOCK * k;
    draw_vis(c, vis, m, (c.h - bs) / 2.0, bs);
    let x0 = m + bs + style::SP[4] * k;
    let acc = if c.w >= c.h * 3.4 { acc } else { RdAcc::None };
    let (vb, cb) = (30.0 * k, 44.0 * k);
    let role = if d.name { style::TITLE } else { style::VALUE };
    let vs = role.size * k;
    let fs11 = style::FOOT.size * k;
    let ss = style::MICRO.size * k;
    let dot_w = if d.dot.is_some() { 7.0 * k + style::SP[2] * k } else { 0.0 };
    let sfx_w = if d.suffix.is_empty() { 0.0 } else { c.measure(d.suffix, ss, 600, Font::Text).0 + 3.0 * k };
    // Text accessories take what they need (up to the 34% slot, more when the main text
    // is short), so neither side ellipsizes when both fit.
    let avail = c.w - m - x0 - style::SP[4] * k;
    let main = (dot_w + c.measure(d.value, vs, role.weight, Font::Text).0 + sfx_w).max(c.measure(d.caption, fs11, 400, Font::Text).0);
    let slot = (c.w * 0.34).round();
    let text_need = |need: f32| (need + 1.0).min(slot.max(avail - main)).max(avail * 0.3);
    let slot_w = match &acc {
        RdAcc::None => 0.0,
        RdAcc::Pair(v, _, r, label) => text_need(c.measure(v, r.size * k, r.weight, Font::Text).0.max(c.measure(label, fs11, 400, Font::Text).0)),
        RdAcc::Dot(_, label) => text_need(c.measure(label, fs11, 400, Font::Text).0 + (RD_DOT + style::SP[2]) * k),
        _ => slot,
    };
    let slot_x = c.w - m - slot_w;
    let right = if matches!(acc, RdAcc::None) { c.w - m } else { slot_x - style::SP[4] * k };
    let mut x = x0;
    if let Some(col) = d.dot {
        let r = 3.5 * k;
        c.circle(x + r, vb - c.cap_h(vs) / 2.0, r, col);
        x += dot_w;
    }
    let room = right - x - sfx_w;
    let (v, fs) = if d.name {
        // Names (events, files) step down to 10 before they ellipsize.
        (d.value.to_string(), rd_fit(c, d.value, vs, style::FOOT.min * k, role.weight, room))
    } else {
        fit_value(c, d.value, Type { size: vs, min: role.min * k, ..role }, room)
    };
    let vw = c.text_base(&v, x, vb, fs, role.weight, c.text1(), Align::Left, if d.name { room } else { c.w * 2.0 });
    if !d.suffix.is_empty() {
        c.text_cap(d.suffix, x + vw + 3.0 * k, vb - c.cap_h(fs), ss, 600, c.text2(), Align::Left, c.w);
    }
    let cap_room = right - x0;
    match d.tag {
        Some((t, col)) => {
            let cw = if d.caption.is_empty() { 0.0 } else { c.text_base(d.caption, x0, cb, fs11, 400, c.text2(), Align::Left, cap_room) + style::SP[2] * k };
            c.text_base(t, x0 + cw, cb, fs11, 600, col, Align::Left, (cap_room - cw).max(1.0));
        }
        None => {
            // Names (processes, files) may step down to 10 before they ellipsize.
            let cs = rd_fit(c, d.caption, fs11, style::FOOT.min * k, 400, cap_room);
            c.text_base(d.caption, x0, cb, cs, 400, c.text2(), Align::Left, cap_room);
        }
    }
    let r = c.w - m;
    match acc {
        RdAcc::None => {}
        RdAcc::Pair(v, col, role, label) => {
            let s = rd_fit(c, &v, role.size * k, role.min * k, role.weight, slot_w);
            c.text_base(&v, r, vb, s, role.weight, col, Align::Right, slot_w);
            c.text_base(&label, r, cb, fs11, 400, c.text2(), Align::Right, slot_w);
        }
        RdAcc::Meter(f, col, label) => {
            c.text_base(&label, r, 25.0 * k, fs11, 400, c.text2(), Align::Right, slot_w);
            c.bar(slot_x, 31.0 * k, slot_w, style::METER_H_COMPACT * k, f, col);
        }
        RdAcc::Stack(parts, label) => {
            c.text_base(&label, r, 25.0 * k, fs11, 400, c.text2(), Align::Right, slot_w);
            let (y, h) = (31.0 * k, style::METER_H_COMPACT * k);
            c.rr(slot_x, y, slot_w, h, h / 2.0, c.track());
            let mut sx = slot_x;
            for (f, col) in parts {
                let w = slot_w * f.clamp(0.0, 1.0);
                if w >= 1.0 {
                    c.rr(sx, y, (w - k).max(h), h, h / 2.0, col);
                }
                sx += w;
            }
        }
        RdAcc::Dot(col, label) => {
            let base = (c.h + c.cap_h(fs11)) / 2.0;
            let tw = c.text_base(&label, r, base, fs11, 400, c.text2(), Align::Right, slot_w - (RD_DOT + style::SP[2]) * k);
            let rr = RD_DOT / 2.0 * k;
            c.circle(r - tw - style::SP[2] * k - rr, base - c.cap_h(fs11) / 2.0, rr, col);
        }
        RdAcc::Spark(vals, col, label) => {
            c.text_base(&label, r, 24.0 * k, fs11, 400, c.text2(), Align::Right, slot_w);
            c.chart(slot_x, 30.0 * k, slot_w, 10.0 * k, &vals, col, ChartOpts { fill: false, dot: false, stroke: style::STROKE_SPARK * k, smooth: vals.len() >= 8 });
        }
        RdAcc::Week(vals, col, today) => {
            let (cell, gap) = (8.0 * k, 2.5 * k);
            let n = vals.len().min(7);
            for i in 0..7usize {
                let cx = r - (7 - i) as f32 * (cell + gap) + gap;
                let v = if i + n >= 7 { vals[vals.len() + i - 7] } else { 0.0 };
                c.rr(cx, 18.0 * k, cell, cell, 2.0 * k, rd_heat_fill(c, v, col));
                let wd = (today + 1 + i) % 7;
                let ink = if i == 6 { c.text1() } else { c.text3() };
                c.text_base(&"SMTWTFS"[wd..wd + 1], cx + cell / 2.0, 38.0 * k, style::MICRO.size * k, 600, ink, Align::Center, cell * 2.0);
            }
        }
    }
}

/// `placeholder()` with this family's dock setup state: the name fits 13 → 11 (then the
/// short name) before it ellipsizes, and Dock L puts a blue Connect pill in the accessory
/// slot with the one-line reason under the name.
fn rd_placeholder(c: &Canvas, out: &mut Painted, name: &str, short: &str, badge: &Badge, conn: &'static str, g: &Got, sh: Shape) -> bool {
    let reason = match (g, sh) {
        (Got::Setup(m), Shape::Dock) => m.clone(),
        _ => return placeholder(c, out, name, badge, conn, g, sh),
    };
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Connect(conn)));
    let k = c.h / 54.0;
    let m = style::M_DOCK * k;
    let bs = style::BADGE_DOCK * k;
    c.badge(badge, m, (c.h - bs) / 2.0, bs);
    let x0 = m + bs + style::SP[4] * k;
    let wide = c.w >= c.h * 3.4;
    let mut right = c.w - m;
    if wide {
        let fs = style::CAPTION.size * k;
        let ph = 26.0 * k;
        let pw = c.measure("Connect", fs, 600, Font::Text).0 + 2.0 * 14.0 * k;
        let (px, py) = (c.w - m - pw, (c.h - ph) / 2.0);
        c.rr(px, py, pw, ph, ph / 2.0, c.blue());
        c.text_cap("Connect", px + pw / 2.0, py + (ph - c.cap_h(fs)) / 2.0, fs, 600, WHITE, Align::Center, pw);
        right = px - style::SP[4] * k;
    }
    let tw = right - x0;
    let (big, min) = (style::TITLE.size * k, style::TITLE.min * k);
    let label = if c.measure(name, min, 600, Font::Text).0 <= tw || short.is_empty() { name } else { short };
    let fs = rd_fit(c, label, big, min, 600, tw);
    c.text_base(label, x0, 30.0 * k, fs, 600, c.text1(), Align::Left, tw);
    if wide {
        let cs = rd_fit(c, &reason, style::FOOT.size * k, style::FOOT.min * k, 400, tw);
        c.text_base(&reason, x0, 44.0 * k, cs, 400, c.text2(), Align::Left, tw);
    } else {
        c.text_base("Connect", x0, 44.0 * k, style::FOOT.size * k, 600, c.blue(), Align::Left, tw);
    }
    true
}

/// Small card top row: badge (or visual) 28 at the margin and a top-right accessory text.
fn rd_small_top(c: &Canvas, vis: &Vis, meta: &str, col: Col) {
    let m = style::M_SMALL;
    draw_vis(c, vis, m, m, style::BADGE_SMALL);
    if !meta.is_empty() {
        // §3 Small accessory slot: x 52 to 114.
        let x0 = m + style::BADGE_SMALL + style::SP[5];
        let fs = rd_fit(c, meta, style::FOOT.size, style::FOOT.min, 600, c.w - m - x0);
        c.text_base(meta, c.w - m, 26.0, fs, 600, col, Align::Right, c.w - m - x0);
    }
}

/// Header template with trailing meta (11/500 text2) instead of a value.
fn rd_header(c: &Canvas, badge: &Badge, title: &str, meta: &str) {
    header(c, badge, title, None);
    if !meta.is_empty() {
        c.text_base(meta, c.w - style::M_CARD, style::HEADER_BASE, style::FOOT.size, 500, c.text2(), Align::Right, c.w * 0.45);
    }
}

/// The Large footnote row on the bottom margin: a summary (text3) and an optional
/// right-aligned figure (text2).
fn rd_foot(c: &Canvas, left: &str, right: &str) {
    let m = style::M_CARD;
    let b = c.h - m;
    let fs = style::FOOT.size;
    let rw = if right.is_empty() { 0.0 } else { c.text_base(right, c.w - m, b, fs, 500, c.text2(), Align::Right, (c.w - 2.0 * m) * 0.5) + style::SP[3] };
    c.text_base(left, m, b, fs, 400, c.text3(), Align::Left, c.w - 2.0 * m - rw);
}

/// First baseline, pitch and row count for list rows at a size (§2.5).
fn rd_pitch(sh: Shape) -> (f32, f32, usize) {
    match sh {
        Shape::Small => (RD_ROW1_S, style::ROW_S, style::ROWS_MAX_S),
        Shape::Large => (RD_ROW1, style::ROW_L, style::ROWS_MAX_L),
        _ => (RD_ROW1, style::ROW_M, style::ROWS_MAX_M),
    }
}

/// What leads a list row.
#[derive(Clone, Copy)]
enum Lead {
    None,
    /// A filled status dot.
    Dot(Col),
    /// A hollow status ring (to do).
    Ring,
    /// A 3×14 calendar bar.
    Bar(Col),
    /// A Fluent glyph in text2 (file types).
    Glyph(&'static str),
}

/// One list row.
struct LRow {
    lead: Lead,
    label: String,
    /// Drawn right after the label in text3 and never truncated (".docx").
    ext: String,
    /// A Micro day label ("Tuesday") that takes a row slot.
    micro: bool,
    trail: String,
    /// None = text2.
    trail_col: Option<Col>,
    trail_w: u32,
    /// A status dot before the trailing text (or alone, right-aligned).
    trail_dot: Option<Col>,
    /// An inline meter between label and value (Medium/Large).
    meter: Option<(f32, Col)>,
    act: Option<WAct>,
}

impl LRow {
    fn new(label: impl Into<String>, trail: impl Into<String>) -> Self {
        LRow { lead: Lead::None, label: label.into(), ext: String::new(), micro: false, trail: trail.into(), trail_col: None, trail_w: 500, trail_dot: None, meter: None, act: None }
    }
    fn day(label: impl Into<String>) -> Self {
        LRow { micro: true, ..LRow::new(label, "") }
    }
}

/// Draw one list row on `base` (Small: 11 px labels from x 14; otherwise Body 13 from x 16).
fn rd_row(c: &Canvas, out: &mut Painted, r: &LRow, base: f32, pitch: f32) {
    let small = c.w < 180.0;
    let m = if small { style::M_SMALL } else { style::M_CARD };
    let right = c.w - m;
    if r.micro {
        c.text_base(&r.label, m, base, style::MICRO.size, 600, c.text3(), Align::Left, right - m);
        return;
    }
    let ls = if small { style::FOOT.size } else { style::BODY.size };
    let cy = base - c.cap_h(ls) / 2.0;
    let dot_gap = if small { style::SP[2] } else { style::SP[3] };
    let x = match r.lead {
        Lead::None => m,
        Lead::Dot(col) => {
            c.circle(m + RD_DOT / 2.0, cy, RD_DOT / 2.0, col);
            m + RD_DOT + dot_gap
        }
        Lead::Ring => {
            c.circle_stroke(m + RD_DOT / 2.0, cy, RD_DOT / 2.0 - 0.75, c.text3(), 1.5);
            m + RD_DOT + dot_gap
        }
        Lead::Bar(col) => {
            c.rr(m, cy - 7.0, 3.0, 14.0, 1.5, col);
            m + 3.0 + style::SP[2]
        }
        Lead::Glyph(g) => {
            let gs = if small { 14.0 } else { 16.0 };
            c.glyph(g, m + gs / 2.0, cy, gs, c.text2());
            m + gs + if small { style::SP[1] } else { style::SP[3] }
        }
    };
    let fs = style::FOOT.size;
    let mut tr_left = right;
    let mut tw = 0.0;
    if !r.trail.is_empty() {
        tw = c.text_base(&r.trail, right, base, fs, r.trail_w, r.trail_col.unwrap_or(c.text2()), Align::Right, (right - x) * 0.5);
        tr_left = right - tw - style::SP[3];
    }
    if let Some(col) = r.trail_dot {
        let dx = if tw > 0.0 { right - tw - style::SP[2] - RD_DOT / 2.0 } else { right - RD_DOT / 2.0 };
        c.circle(dx, base - c.cap_h(fs) / 2.0, RD_DOT / 2.0, col);
        tr_left = dx - RD_DOT / 2.0 - style::SP[3];
    }
    if let Some((f, col)) = r.meter {
        let x1 = right - RD_METER_W;
        let h = style::METER_H_COMPACT;
        c.bar(x1 - RD_METER_W, cy - h / 2.0, RD_METER_W, h, f, col);
        tr_left = x1 - RD_METER_W - style::SP[3];
    }
    let ew = if r.ext.is_empty() { 0.0 } else { c.measure(&r.ext, ls, 400, Font::Text).0 };
    let lroom = (tr_left - x - ew).max(1.0);
    // Small labels (11) may step down to 10 before they ellipsize.
    let ls = if small { rd_fit(c, &r.label, ls, style::FOOT.min, 400, lroom) } else { ls };
    let lw = c.text_base(&r.label, x, base, ls, 400, c.text1(), Align::Left, lroom);
    if ew > 0.0 {
        c.text_base(&r.ext, x + lw, base, ls, 400, c.text3(), Align::Left, ew + 1.0);
    }
    if let Some(a) = &r.act {
        let inset = m - style::SP[2];
        out.hits.push(((inset, cy - pitch / 2.0, c.w - 2.0 * inset, pitch), a.clone()));
    }
}

/// A designed zero-items state: a 24 px glyph in text3 with one line under it.
fn rd_empty(c: &Canvas, sh: Shape, glyph: &str, text: &str) {
    let (gy, tb, fs) = match sh {
        Shape::Small => (66.0, 98.0, style::FOOT.size),
        Shape::Large => (96.0, 128.0, style::CAPTION.size),
        _ => (54.0, 80.0, style::CAPTION.size),
    };
    c.glyph(glyph, c.w / 2.0, gy, 24.0, c.text3());
    c.text_base(text, c.w / 2.0, tb, fs, 400, c.text2(), Align::Center, c.w - 2.0 * style::M_SMALL);
}

/// Loading skeleton for Header-template list cards (§7): real badge and title, bars for the rest.
fn rd_list_skeleton(c: &Canvas, badge: &Badge, title: &str, sh: Shape) {
    let (b0, pitch, n) = rd_pitch(sh);
    let bar = 8.0;
    if sh == Shape::Small {
        let m = style::M_SMALL;
        c.badge(badge, m, m, style::BADGE_SMALL);
        for i in 0..n {
            skeleton(c, m, b0 + i as f32 * pitch - bar, (c.w - 2.0 * m) * if i == 0 { 0.7 } else { 0.5 }, bar);
        }
        return;
    }
    let m = style::M_CARD;
    header(c, badge, title, None);
    skeleton(c, c.w - m - 36.0, style::HEADER_BASE - bar, 36.0, bar);
    let col = c.w - 2.0 * m - 32.0 - style::SP[3];
    for i in 0..n {
        let b = b0 + i as f32 * pitch;
        skeleton(c, m, b - bar, col * if i % 2 == 0 { 0.6 } else { 0.45 }, bar);
        skeleton(c, c.w - m - 32.0, b - bar, 32.0, bar);
    }
    if sh == Shape::Large {
        skeleton(c, m, c.h - m - bar, 120.0, bar);
    }
}

/// Three stat columns across the card: value 17/600 and label 11 text2 (label above the
/// value when `label_above`), each with an optional 6 px dot before the label.
fn rd_stats(c: &Canvas, items: &[(String, String, Option<Col>)], top_base: f32, bottom_base: f32, label_above: bool) {
    let m = style::M_CARD;
    let cw = (c.w - 2.0 * m) / 3.0;
    let (vb, lb) = if label_above { (bottom_base, top_base) } else { (top_base, bottom_base) };
    let fs = style::FOOT.size;
    for (i, (v, label, dot)) in items.iter().take(3).enumerate() {
        let x = m + i as f32 * cw;
        let (v, vs) = fit_value(c, v, style::VALUE, cw - style::SP[3]);
        c.text_base(&v, x, vb, vs, style::VALUE.weight, c.text1(), Align::Left, c.w);
        let mut lx = x;
        if let Some(col) = dot {
            c.circle(x + RD_DOT / 2.0, lb - c.cap_h(fs) / 2.0, RD_DOT / 2.0, *col);
            lx += RD_DOT + style::SP[1];
        }
        c.text_base(label, lx, lb, fs, 400, c.text2(), Align::Left, cw - style::SP[0] - (lx - x));
    }
}

/// Words of `s` in at most `lines` lines of `max_w`; the last line keeps the rest (drawing
/// it with `max_w` ellipsizes it).
fn rd_wrap(c: &Canvas, s: &str, size: f32, weight: u32, max_w: f32, lines: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for w in s.split_whitespace() {
        let t = if cur.is_empty() { w.to_string() } else { format!("{cur} {w}") };
        if cur.is_empty() || out.len() + 1 >= lines || c.measure(&t, size, weight, Font::Text).0 <= max_w {
            cur = t;
        } else {
            out.push(std::mem::replace(&mut cur, w.to_string()));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A title in up to two lines: the largest size from `start` to `min` at which it fits,
/// else `min` with the second line ellipsized.
fn rd_wrap_fit(c: &Canvas, s: &str, start: f32, min: f32, weight: u32, max_w: f32) -> (Vec<String>, f32) {
    let mut size = start;
    loop {
        let lines = rd_wrap(c, s, size, weight, max_w, 2);
        let fits = lines.iter().all(|l| c.measure(l, size, weight, Font::Text).0 <= max_w);
        if fits || size <= min {
            return (lines, size);
        }
        size = (size - 2.0).max(min);
    }
}

/// "+0.6%", "−0.4%", "+12%" (§2.2) and its colour: green up, red down, text2 for none.
fn rd_change(c: &Canvas, d: f64) -> (String, Col) {
    if d.abs() < 0.05 {
        return ("0.0%".into(), c.text2());
    }
    let sign = if d > 0.0 { "+" } else { "\u{2212}" };
    let body = if d.abs() >= 10.0 { format!("{:.0}", d.abs()) } else { format!("{:.1}", d.abs()) };
    (format!("{sign}{body}%"), up_down(d))
}

/// A tinted, fully round capsule with centred text, right edge at `right`.
fn rd_capsule(c: &Canvas, right: f32, top: f32, w: f32, h: f32, text: &str, col: Col) {
    c.rr(right - w, top, w, h, h / 2.0, alpha(col, if c.light { 0.14 } else { 0.16 }));
    let fs = style::FOOT.size;
    c.text_cap(text, right - w / 2.0, top + (h - c.cap_h(fs)) / 2.0, fs, 600, col, Align::Center, w);
}

/// Percent: one decimal under 10, whole numbers otherwise.
fn rd_pct(v: f64) -> String {
    if v < 10.0 {
        format!("{v:.1}%")
    } else {
        format!("{v:.0}%")
    }
}

/// Gigabytes: one decimal, or whole at ≥ 10 when `tight` (Mini).
fn rd_gb(v: f64, tight: bool) -> String {
    let s = if tight && v >= 10.0 { format!("{v:.0}") } else { format!("{v:.1}") };
    format!("{} GB", s.strip_suffix(".0").filter(|_| tight || v >= 100.0).unwrap_or(&s))
}

// ---------------------------------------------------------------- heat maps

/// A heat cell's fill: track when empty, else one of four steps of the accent.
fn rd_heat_fill(c: &Canvas, v: f32, col: Col) -> Col {
    if v <= 0.0 {
        return c.track();
    }
    let step = ((v * 4.0).ceil() as usize).clamp(1, 4) - 1;
    // Over the dark glass, the two low steps at 0.35 / 0.55 turn a warm or green accent
    // muddy (brown, olive): dark theme raises them so they keep the accent's hue.
    let a = if c.light { style::HEAT_STEPS[step] } else { [0.48, 0.66, 0.82, 1.0][step] };
    alpha(col, a)
}

/// A contribution grid `weeks` wide ending today at `right` (today's column is cut after
/// today's row). `col` None draws every cell as track (loading).
#[allow(clippy::too_many_arguments)]
fn rd_heat_cells(c: &Canvas, grid: &[f32], today_row: usize, right: f32, top: f32, weeks: usize, cell: f32, gap: f32, radius: f32, col: Option<Col>) {
    let n = grid.len() as isize;
    for w in 0..weeks {
        for row in 0..7usize {
            if w == weeks - 1 && row > today_row {
                continue;
            }
            let back = ((weeks - 1 - w) * 7 + today_row) as isize - row as isize;
            let idx = n - 1 - back;
            let v = if idx >= 0 && idx < n { grid[idx as usize] } else { 0.0 };
            let fill = match col {
                Some(cl) => rd_heat_fill(c, v, cl),
                None => c.track(),
            };
            let x = right - (weeks - w) as f32 * (cell + gap) + gap;
            c.rr(x, top + row as f32 * (cell + gap), cell, cell, radius, fill);
        }
    }
}

/// The longest run of active days in a contribution grid.
fn rd_longest_run(grid: &[f32]) -> u32 {
    let (mut best, mut run) = (0u32, 0u32);
    for v in grid {
        run = if *v > 0.0 { run + 1 } else { 0 };
        best = best.max(run);
    }
    best
}

/// The weekday (0 = Sunday) with the most activity in a grid whose last cell is `today`.
fn rd_busiest_weekday(grid: &[f32], today: usize) -> Option<usize> {
    let mut sum = [0.0f32; 7];
    for (back, v) in grid.iter().rev().enumerate() {
        sum[(today + 7 - back % 7) % 7] += *v;
    }
    let (i, best) = sum.iter().enumerate().fold((0, 0.0f32), |a, (i, v)| if *v > a.1 { (i, *v) } else { a });
    (best > 0.0).then_some(i)
}

/// Contribution levels summed per calendar month for the last `n` months (oldest first),
/// with each month's number (1-12). The grid's last cell is today.
fn rd_month_sums(grid: &[f32], now: &super::Now, n: usize) -> Vec<(u32, f32)> {
    let (mut y, mut mo, mut d) = (now.year, now.month, now.day.max(1));
    let mut out: Vec<(u32, f32)> = vec![(mo, 0.0)];
    for v in grid.iter().rev() {
        if let Some(last) = out.last_mut() {
            last.1 += *v;
        }
        d -= 1;
        if d == 0 {
            if out.len() >= n {
                break;
            }
            mo = if mo <= 1 { 12 } else { mo - 1 };
            if mo == 12 {
                y = y.saturating_sub(1);
            }
            d = super::days_in_month(y, mo).max(1);
            out.push((mo, 0.0));
        }
    }
    out.reverse();
    out
}

#[allow(clippy::too_many_arguments)]
fn heat(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, color: Col, open: &str, sh: Shape) {
    let g = got(x, conn);
    let today = x.now.weekday as usize;
    if matches!(g, Got::Loading) {
        rd_heat_layout(c, None, conn, badge, color, today, sh);
        return;
    }
    if rd_placeholder(c, out, def.name, "", badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    let open = match (conn, x.cfg.widget_settings.get(conn).and_then(|m| m.get("username"))) {
        ("github", Some(u)) => format!("https://github.com/{u}"),
        ("leetcode", Some(u)) => format!("https://leetcode.com/u/{u}"),
        _ => open.to_string(),
    };
    open_hit(out, c, &open);
    rd_heat_layout(c, Some(&*f), conn, badge, color, today, sh);
}

/// Every size of a contribution card; `f` None draws the loading skeleton of the same layout.
fn rd_heat_layout(c: &Canvas, f: Option<&Feed>, conn: &str, badge: &Badge, color: Col, today: usize, sh: Shape) {
    let leet = conn == "leetcode";
    let unit = if leet { "solved" } else { "contributions" };
    let grid: &[f32] = f.map(|f| f.grid.as_slice()).unwrap_or(&[]);
    let col = f.map(|_| color);
    let count = f.and_then(|f| f.value).map(|v| super::grouped(v.round() as i64));
    let streak = f.and_then(|f| f.extra.get("streak")).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    let caption = if streak > 0 { format!("{streak}-day streak") } else { unit.to_string() };
    match sh {
        Shape::Mini => {
            // Visual-led: the last 7 weeks as a 7×7 block.
            let k = c.h / 54.0;
            let (cell, gap) = (4.0 * k, 1.5 * k);
            let s = 7.0 * cell + 6.0 * gap;
            rd_heat_cells(c, grid, today, (c.w + s) / 2.0, (c.h - s) / 2.0, 7, cell, gap, k, col);
        }
        Shape::Dock => match &count {
            Some(v) => {
                let last: Vec<f32> = grid.iter().rev().take(7).rev().copied().collect();
                rd_dock(c, &Vis::Badge(*badge), DockText::new(v, &caption), RdAcc::Week(last, color, today));
            }
            None => {
                let k = c.h / 54.0;
                let (m, bs) = (style::M_DOCK * k, style::BADGE_DOCK * k);
                c.badge(badge, m, (c.h - bs) / 2.0, bs);
                let x0 = m + bs + style::SP[4] * k;
                skeleton(c, x0, (c.h - 10.0 * k) / 2.0, (60.0 * k).min(c.w - m - x0), 10.0 * k);
            }
        },
        Shape::Small => {
            let m = style::M_SMALL;
            let bs = style::BADGE_HEADER;
            c.badge(badge, m, m, bs);
            let tx = m + bs + style::SP[2];
            match &count {
                Some(v) => {
                    c.text_base(v, tx, m + bs / 2.0 + c.cap_h(style::TITLE.size) / 2.0, style::TITLE.size, 600, c.text1(), Align::Left, c.w - m - tx);
                    c.text_base(&caption, m, 106.0, style::FOOT.size, 400, c.text2(), Align::Left, c.w - 2.0 * m);
                }
                None => {
                    skeleton(c, tx, m + bs / 2.0 - 5.0, 40.0, 10.0);
                    skeleton(c, m, 106.0 - 8.0, 40.0, 8.0);
                }
            }
            rd_heat_cells(c, grid, today, c.w - m, 40.0, 13, 5.5, style::HEAT_GAP, 1.5, col);
        }
        _ => {
            let m = style::M_CARD;
            c.badge(badge, m, m, style::BADGE_HEADER);
            match &count {
                Some(v) => {
                    let (v, fs) = fit_value(c, v, style::HERO_S, 80.0);
                    c.text_base(&v, m, 58.0, fs, style::HERO_S.weight, c.text1(), Align::Left, c.w);
                    c.text_base(unit, m, 74.0, style::CAPTION.size, 400, c.text2(), Align::Left, 80.0);
                }
                None => {
                    let ch = c.cap_h(style::HERO_S.size);
                    skeleton(c, m, 58.0 - ch, 48.0, ch);
                    skeleton(c, m, 74.0 - 8.0, 40.0, 8.0);
                }
            }
            // 16 weeks, today on the right margin, centred in the 16…80 band.
            let (cell, gap) = (7.0, style::HEAT_GAP);
            let gh = 7.0 * cell + 6.0 * gap;
            rd_heat_cells(c, grid, today, c.w - m, m + (style::SEP_Y_LARGE - 2.0 * m - gh) / 2.0, 16, cell, gap, 2.0, col);
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let Some(f) = f else {
                    let cw = (c.w - 2.0 * m) / 3.0;
                    for i in 0..3 {
                        skeleton(c, m + i as f32 * cw, 136.0 - 12.0, 40.0, 12.0);
                        skeleton(c, m + i as f32 * cw, 152.0 - 8.0, 52.0, 8.0);
                    }
                    return;
                };
                let active = f.extra.get("active_days").cloned().unwrap_or_else(|| grid.iter().rev().take(365).filter(|v| **v > 0.0).count().to_string());
                if leet {
                    let items: Vec<(String, String, Option<Col>)> = f
                        .rows
                        .iter()
                        .map(|r| {
                            let dot = match r.label.to_ascii_lowercase().as_str() {
                                "easy" => Some(GREEN),
                                "medium" => Some(YELLOW),
                                "hard" => Some(RED),
                                _ => None,
                            };
                            (r.value.clone(), r.label.clone(), dot)
                        })
                        .collect();
                    rd_stats(c, &items, 136.0, 152.0, false);
                    let foot = if streak > 0 { format!("{streak}-day streak · active {active} of the last 365 days") } else { format!("Active {active} of the last 365 days") };
                    rd_foot(c, &foot, "");
                } else {
                    // GitHub Activity stays about contributions (GitHub Overview owns the
                    // profile numbers): streaks and active days, then the busiest weekday.
                    let days = |n: u32| if n == 1 { "1 day".to_string() } else { format!("{n} days") };
                    let items = [
                        (days(streak), "Streak".to_string(), None),
                        (days(rd_longest_run(grid)), "Best streak".to_string(), None),
                        (active, "Active days".to_string(), None),
                    ];
                    rd_stats(c, &items, 136.0, 152.0, false);
                    if let Some(wd) = rd_busiest_weekday(grid, today) {
                        rd_foot(c, &format!("Most active on {}s", DAYS[wd]), "");
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- lists

/// What a list card shows at each size.
struct LModel {
    /// Header trailing meta (Medium/Large) and the Small top-right accessory.
    meta: String,
    meta_small: String,
    /// Mini value ("" = visual-led badge).
    mini: String,
    /// Dock value and caption (also the agenda's Small hero).
    value: String,
    name: bool,
    dot: Option<Col>,
    suffix: String,
    caption: String,
    acc: RdAcc,
    rows: Vec<LRow>,
    foot: String,
    empty: (&'static str, &'static str),
}

impl LModel {
    fn new() -> Self {
        LModel {
            meta: String::new(),
            meta_small: String::new(),
            mini: String::new(),
            value: String::new(),
            name: false,
            dot: None,
            suffix: String::new(),
            caption: String::new(),
            acc: RdAcc::None,
            rows: Vec::new(),
            foot: String::new(),
            empty: (G_CHEV, "Nothing to show"),
        }
    }
}

/// "Today 2:00 PM" → ("Today", "2:00", "PM"); "Nov 21 9:00 AM" → ("Nov 21", "9:00", "AM");
/// all-day values come back whole with no time.
fn rd_split_when(s: &str) -> (&str, &str, &str) {
    let s = s.trim();
    let (rest, ap) = match s.rsplit_once(' ') {
        Some((r, a)) if a == "AM" || a == "PM" => (r, a),
        _ => return (s, "", ""),
    };
    match rest.rsplit_once(' ') {
        Some((d, t)) => (d, t, ap),
        None => ("", rest, ap),
    }
}

/// The day part of a calendar "when": "Today", "Tomorrow", "Tue", "Nov 21".
fn rd_when_day(s: &str) -> String {
    let s = s.trim();
    if s == "All day" {
        return "Today".into();
    }
    if let Some(d) = s.strip_suffix(" all day") {
        return d.to_string();
    }
    match rd_split_when(s) {
        (d, t, _) if !t.is_empty() => if d.is_empty() { "Today".into() } else { d.to_string() },
        _ => s.to_string(),
    }
}

/// The time part: "2:00 PM", or "All day".
fn rd_when_time(s: &str) -> String {
    match rd_split_when(s) {
        (_, t, ap) if !t.is_empty() => format!("{t} {ap}"),
        _ => "All day".into(),
    }
}

/// "Tue" → "Tuesday" for day labels; other day texts pass through.
fn rd_day_full(d: &str) -> String {
    let i = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].iter().position(|w| *w == d);
    i.map(|i| DAYS[i].to_string()).unwrap_or_else(|| d.to_string())
}

/// A Linear status as a row lead: orange started, green done, a hollow ring to do.
fn rd_issue_lead(c: &Canvas, state: &str) -> Lead {
    let s = state.to_ascii_lowercase();
    if s.contains("progress") || s.contains("review") || s.contains("started") {
        Lead::Dot(ORANGE)
    } else if s.contains("done") || s.contains("complete") || s.contains("merged") {
        Lead::Dot(GREEN)
    } else if s.contains("cancel") || s.contains("duplicate") {
        Lead::Dot(c.text3())
    } else {
        Lead::Ring
    }
}

/// A Vercel deployment state's colour: green ready, orange building, red failed.
fn rd_deploy_col(c: &Canvas, state: &str) -> Col {
    let s = state.to_ascii_lowercase();
    if s.contains("ready") {
        GREEN
    } else if s.contains("build") || s.contains("initial") {
        ORANGE
    } else if s.contains("error") || s.contains("fail") {
        RED
    } else {
        c.text3()
    }
}

/// A file's extension (lowercase, no dot) from its path or name.
fn rd_ext(path: &str, name: &str) -> String {
    let p = if path.is_empty() { name } else { path };
    std::path::Path::new(p).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// A Fluent glyph and a plain type name for a file extension.
fn rd_file_kind(ext: &str) -> (&'static str, String) {
    match ext {
        "doc" | "docx" | "rtf" | "odt" => ("\u{E8A5}", "Word document".into()),
        "xls" | "xlsx" | "csv" | "ods" => ("\u{E8A5}", "Spreadsheet".into()),
        "ppt" | "pptx" | "odp" => ("\u{E8A5}", "Presentation".into()),
        "pdf" => ("\u{EA90}", "PDF".into()),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "bmp" | "svg" => (G_IMAGE, "Image".into()),
        "txt" | "md" | "log" => ("\u{E8A5}", "Text".into()),
        "mp4" | "mov" | "mkv" | "avi" => ("\u{E714}", "Video".into()),
        "mp3" | "wav" | "flac" | "m4a" => (G_MUSIC, "Audio".into()),
        "" => ("\u{E8A5}", "File".into()),
        e => ("\u{E8A5}", e.to_ascii_uppercase()),
    }
}

/// Build a list card's content for its connector.
fn rd_list_model(c: &Canvas, conn: &str, f: &Feed, sh: Shape) -> LModel {
    let small = sh == Shape::Small;
    let large = sh == Shape::Large;
    let mut md = LModel::new();
    match conn {
        "top_cpu" | "top_memory" => {
            let mem = conn == "top_memory";
            let accent = if mem { PURPLE } else { BLUE };
            let total = f.value.unwrap_or(0.0);
            let total_gb = f.extra.get("total_gb").and_then(|v| v.parse::<f64>().ok());
            let frac = f.frac.unwrap_or(if mem { total_gb.map(|t| (total / t) as f32).unwrap_or(0.0) } else { (total / 100.0) as f32 });
            md.meta = if mem { format!("{} used", rd_gb(total, false)) } else { format!("{} total", rd_pct(total)) };
            // The Small slot is 62 px: the same one-decimal figure, without "used".
            md.meta_small = if mem { rd_gb(total, false) } else { md.meta.clone() };
            md.mini = if mem { rd_gb(total, true) } else { rd_pct(total) };
            match f.rows.first() {
                Some(r) => {
                    md.value = r.value.clone();
                    md.caption = r.label.clone();
                }
                None => {
                    md.value = md.mini.clone();
                    md.caption = if mem { "Memory in use".into() } else { "CPU in use".into() };
                }
            }
            md.acc = RdAcc::Meter(frac, accent, md.meta.clone());
            let maxf = f.rows.iter().filter_map(|r| r.frac).fold(0.0f32, f32::max);
            md.rows = f
                .rows
                .iter()
                .map(|r| {
                    let mut lr = LRow::new(&r.label, &r.value);
                    if !small && maxf > 0.0 {
                        lr.meter = Some((r.frac.unwrap_or(0.0) / maxf, accent));
                    }
                    lr
                })
                .collect();
            let n = f.rows.len().min(style::ROWS_MAX_L);
            md.foot = if mem {
                match total_gb {
                    Some(t) => format!("{} of {} in use", rd_gb(total, false), rd_gb(t, true)),
                    None => format!("{} in use", rd_gb(total, false)),
                }
            } else {
                let sum: f64 = f.rows.iter().take(n).filter_map(|r| r.value.trim_end_matches('%').trim().parse::<f64>().ok()).sum();
                format!("Top {n} \u{00B7} {} of CPU", rd_pct(sum))
            };
            md.empty = (if mem { "\u{E964}" } else { "\u{E950}" }, "Nothing busy right now");
        }
        "calendar" => {
            let n = f.value.unwrap_or(0.0) as i64;
            md.meta = format!("{n} today");
            md.meta_small = md.meta.clone();
            let next_title = f.extra.get("next_title").cloned().unwrap_or_default();
            let next_time = f.extra.get("next_time").cloned().unwrap_or_default();
            let mins = f.extra.get("next_minutes").and_then(|m| m.parse::<i64>().ok());
            let soon = mins.filter(|m| (0..60).contains(m));
            let today = rd_when_day(&next_time) == "Today";
            let (_, time, ap) = rd_split_when(&next_time);
            let day = rd_when_day(&next_time);
            if next_title.is_empty() {
                md.value = "Free".into();
                md.caption = "No more events today".into();
            } else {
                if time.is_empty() {
                    md.value = "All day".into();
                } else {
                    md.value = time.to_string();
                    md.suffix = ap.to_string();
                }
                md.caption = if today { next_title.clone() } else { format!("{day} · {next_title}") };
                if today && !time.is_empty() {
                    md.mini = time.to_string();
                }
            }
            md.acc = match soon {
                Some(m) => RdAcc::Pair(format!("in {m} min"), RED, RD_FOOT_SB, md.meta.clone()),
                None => RdAcc::Pair(n.to_string(), c.text1(), style::VALUE, "today".into()),
            };
            let mut last_day = "Today".to_string();
            for (i, r) in f.rows.iter().enumerate() {
                let d = rd_when_day(&r.value);
                let is_today = d == "Today";
                if large && !is_today && d != last_day {
                    md.rows.push(LRow::day(rd_day_full(&d)));
                    last_day = d.clone();
                }
                let trail = if large || is_today { rd_when_time(&r.value) } else { r.value.clone() };
                let mut lr = LRow::new(&r.label, trail);
                lr.lead = Lead::Bar(RED);
                if let (0, Some(m)) = (i, soon) {
                    if r.label == next_title {
                        lr.trail = format!("in {m}m");
                        lr.trail_col = Some(RED);
                        lr.trail_w = 600;
                    }
                }
                md.rows.push(lr);
            }
            if large {
                md.rows.truncate(style::ROWS_MAX_L);
                if md.rows.last().is_some_and(|r| r.micro) {
                    md.rows.pop();
                }
            }
            let left = f.rows.iter().filter(|r| rd_when_day(&r.value) == "Today").count();
            md.foot = match (left, soon) {
                (0, _) => "No more events today".into(),
                (l, Some(m)) => format!("{l} left today · next in {m} min"),
                (l, None) => format!("{l} left today"),
            };
            md.empty = (G_CAL, "No more events today");
        }
        "linear" => {
            let count = f.text.clone().unwrap_or_else(|| headline(f));
            let prog = f.rows.iter().filter(|r| matches!(rd_issue_lead(c, &r.value), Lead::Dot(col) if col == ORANGE)).count();
            md.meta = format!("{count} assigned");
            md.meta_small = format!("{count} open");
            md.mini = count.clone();
            md.value = count.clone();
            md.caption = "Assigned".into();
            md.acc = RdAcc::Dot(if prog > 0 { ORANGE } else { c.text3() }, format!("{prog} in progress"));
            md.rows = f
                .rows
                .iter()
                .map(|r| {
                    // "ENG-142 Dock widgets" → title "Dock widgets", id "ENG-142".
                    let (id, title) = match r.label.split_once(' ') {
                        Some((id, t)) if id.contains('-') && id.ends_with(|ch: char| ch.is_ascii_digit()) => (id, t),
                        _ => ("", r.label.as_str()),
                    };
                    let mut lr = LRow::new(title, if small { "" } else { id });
                    lr.lead = rd_issue_lead(c, &r.value);
                    lr.trail_col = Some(c.text3());
                    lr
                })
                .collect();
            md.foot = format!("{count} open · {prog} in progress");
            md.empty = ("\u{E73E}", "Nothing assigned");
        }
        "vercel" => {
            let count = headline(f);
            md.meta = format!("{count} this week");
            md.meta_small = md.meta.clone();
            let state = f.extra.get("latest_state").cloned().or_else(|| f.rows.first().map(|r| r.value.clone())).unwrap_or_default();
            let project = f.extra.get("latest_project").cloned().or_else(|| f.rows.first().map(|r| r.label.clone())).unwrap_or_default();
            md.dot = Some(rd_deploy_col(c, &state));
            md.value = if state.is_empty() { "—".into() } else { state };
            md.caption = project;
            md.acc = RdAcc::Pair(count.clone(), c.text1(), style::VALUE, "this week".into());
            md.rows = f
                .rows
                .iter()
                .map(|r| {
                    let mut lr = LRow::new(&r.label, if small { "" } else { r.value.as_str() });
                    lr.trail_dot = Some(rd_deploy_col(c, &r.value));
                    lr
                })
                .collect();
            let building = f.rows.iter().filter(|r| rd_deploy_col(c, &r.value) == ORANGE).count();
            let failed = f.rows.iter().filter(|r| rd_deploy_col(c, &r.value) == RED).count();
            md.foot = match (failed, building) {
                (0, 0) => format!("{count} deploys this week · all ready"),
                (0, b) => format!("{count} deploys this week · {b} building"),
                (e, _) => format!("{count} deploys this week · {e} failed"),
            };
            md.empty = ("\u{E753}", "No deployments yet");
        }
        _ => {
            // Recent files: stem as the label, extension in text3, a type glyph.
            let parts: Vec<(String, String, String, String)> = f
                .rows
                .iter()
                .map(|r| {
                    let ext = rd_ext(&r.value, &r.label);
                    let dotted = format!(".{ext}");
                    let stem = if !ext.is_empty() && r.label.to_ascii_lowercase().ends_with(&dotted) { r.label[..r.label.len() - dotted.len()].to_string() } else { r.label.clone() };
                    let folder = std::path::Path::new(&r.value).parent().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    (stem, ext, folder, r.value.clone())
                })
                .collect();
            if let Some((stem, ext, _, _)) = parts.first() {
                md.value = stem.clone();
                md.name = true;
                md.caption = rd_file_kind(ext).1;
            } else {
                md.value = "None".into();
                md.caption = "No recent files".into();
            }
            if let Some((stem, ext, _, _)) = parts.get(1) {
                // Body 13/400 text2, so the first file's name stays the dominant value.
                md.acc = RdAcc::Pair(stem.clone(), c.text2(), style::BODY, rd_file_kind(ext).1);
            }
            md.rows = parts
                .iter()
                .map(|(stem, ext, folder, path)| {
                    let mut lr = LRow::new(stem, if small { "" } else { folder.as_str() });
                    lr.lead = Lead::Glyph(rd_file_kind(ext).0);
                    if !ext.is_empty() {
                        lr.ext = format!(".{ext}");
                    }
                    lr.trail_col = Some(c.text3());
                    if !path.is_empty() {
                        lr.act = Some(WAct::File(path.clone()));
                    }
                    lr
                })
                .collect();
            md.foot = format!("{} recent files · newest first", parts.len());
            md.empty = ("\u{E8B7}", "No recent files");
        }
    }
    md
}

#[allow(clippy::too_many_arguments)]
fn list(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, open: &str, sh: Shape) {
    let g = got(x, conn);
    let title = if conn == "github" { "GitHub" } else { def.name };
    if matches!(g, Got::Loading) && !matches!(sh, Shape::Mini | Shape::Dock) {
        match (conn, sh) {
            ("github", _) => rd_github_skeleton(c, badge, &x.now, sh),
            ("calendar", Shape::Small) => rd_hero_skeleton(c, badge, 0.6),
            _ => rd_list_skeleton(c, badge, title, sh),
        }
        return;
    }
    if rd_placeholder(c, out, def.name, "", badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, open);
    if conn == "github" {
        rd_github_overview(c, &f, badge, &x.now, sh);
        return;
    }
    let md = rd_list_model(c, conn, &f, sh);
    let vis = Vis::Badge(*badge);
    match sh {
        Shape::Mini if conn == "vercel" => rd_status_mini(c, badge, md.dot.unwrap_or(c.text3())),
        Shape::Mini => compact_sum(c, &Sum::new(vis, md.mini.clone(), ""), sh),
        Shape::Dock => {
            let d = DockText { value: &md.value, name: md.name, dot: md.dot, suffix: &md.suffix, caption: &md.caption, tag: None };
            rd_dock(c, &vis, d, md.acc);
        }
        Shape::Small if conn == "calendar" => {
            // Agenda Small is an Up Next glance: the next start time as the hero.
            let m = style::M_SMALL;
            rd_small_top(c, &vis, &md.meta_small, c.text2());
            let ss = style::MICRO.size;
            let sfx_w = if md.suffix.is_empty() { 0.0 } else { c.measure(&md.suffix, ss, 600, Font::Text).0 + 3.0 };
            let (v, fs) = fit_value(c, &md.value, style::HERO_S, c.w - 2.0 * m - sfx_w);
            let vw = c.text_base(&v, m, 86.0, fs, style::HERO_S.weight, c.text1(), Align::Left, c.w * 2.0);
            if !md.suffix.is_empty() {
                c.text_cap(&md.suffix, m + vw + 3.0, 86.0 - c.cap_h(fs), ss, 600, c.text2(), Align::Left, c.w);
            }
            c.text_base(&md.caption, m, 106.0, style::CAPTION.size, 400, c.text2(), Align::Left, c.w - 2.0 * m);
        }
        _ => {
            if sh == Shape::Small {
                rd_small_top(c, &vis, &md.meta_small, c.text2());
            } else {
                rd_header(c, badge, title, &md.meta);
            }
            if md.rows.is_empty() {
                rd_empty(c, sh, md.empty.0, md.empty.1);
                return;
            }
            let (b0, pitch, max) = rd_pitch(sh);
            for (i, r) in md.rows.iter().take(max).enumerate() {
                rd_row(c, out, r, b0 + i as f32 * pitch, pitch);
            }
            if sh == Shape::Large {
                rd_foot(c, &md.foot, "");
            }
        }
    }
}

/// Visual-led Mini: the badge with a status dot at its bottom-right corner.
fn rd_status_mini(c: &Canvas, badge: &Badge, col: Col) {
    let k = c.h / 54.0;
    let bs = 30.0 * k;
    let (bx, by) = ((c.w - bs) / 2.0, (c.h - bs) / 2.0);
    c.badge(badge, bx, by, bs);
    let (cx, cy, r) = (bx + bs - 2.0 * k, by + bs - 2.0 * k, 4.0 * k);
    c.circle(cx, cy, r + 1.5 * k, c.halo());
    c.circle(cx, cy, r, col);
}

/// GitHub Overview: a stats card (repositories, stars, followers); Large adds the last 26
/// weeks of contributions.
fn rd_github_overview(c: &Canvas, f: &Feed, badge: &Badge, now: &super::Now, sh: Shape) {
    let stat = |key: &str, labels: &[&str]| f.extra.get(key).cloned().or_else(|| f.rows.iter().find(|r| labels.contains(&r.label.as_str())).map(|r| r.value.clone())).unwrap_or_else(|| "—".into());
    let repos = stat("repos", &["Repos", "Repositories"]);
    let stars = stat("stars", &["Stars"]);
    let followers = stat("followers", &["Followers"]);
    let vis = Vis::Badge(*badge);
    match sh {
        Shape::Mini => compact_sum(c, &Sum::new(vis, stars, ""), sh),
        Shape::Dock => rd_dock(c, &vis, DockText::new(&stars, "Stars"), RdAcc::Pair(followers, c.text1(), style::VALUE, "Followers".into())),
        Shape::Small => {
            let m = style::M_SMALL;
            rd_small_top(c, &vis, &format!("{repos} repos"), c.text2());
            let (v, fs) = fit_value(c, &stars, style::HERO_S, c.w - 2.0 * m);
            c.text_base(&v, m, 86.0, fs, style::HERO_S.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base("Stars", m, 106.0, style::CAPTION.size, 400, c.text2(), Align::Left, c.w - 2.0 * m);
        }
        _ => {
            rd_header(c, badge, "GitHub", "");
            let items = [(repos, "Repositories".to_string(), None), (stars, "Stars".to_string(), None), (followers, "Followers".to_string(), None)];
            rd_stats(c, &items, 64.0, 78.0, false);
            if sh == Shape::Large {
                // Detail: activity per month as columns (GitHub Activity owns the daily grid).
                separator(c, style::SEP_Y_LARGE);
                let months = rd_month_sums(&f.grid, now, 12);
                let max = months.iter().map(|m| m.1).fold(0.0f32, f32::max);
                rd_month_bars(c, &months, if max > 0.0 { Some((max, hex(0x39D353))) } else { None });
                let foot = match f.value {
                    Some(v) => format!("{} contributions in the last year", super::grouped(v.round() as i64)),
                    None => "Contributions in the last year".into(),
                };
                rd_foot(c, &foot, "");
            }
        }
    }
}

/// GitHub Overview's Large detail: one column per month (oldest first) over its initial.
/// `scale` (the largest month and the colour) None draws track columns (loading).
fn rd_month_bars(c: &Canvas, months: &[(u32, f32)], scale: Option<(f32, Col)>) {
    let m = style::M_CARD;
    let n = months.len().max(1);
    let pitch = (c.w - 2.0 * m) / n as f32;
    let bw = 10.0;
    let (top, bottom) = (style::DETAIL_TOP, 154.0);
    let label_base = 170.0;
    for (i, (mo, v)) in months.iter().enumerate() {
        let cx = m + pitch * (i as f32 + 0.5);
        // The current month's initial is emphasised (not while loading: no date yet).
        let last = i + 1 == months.len() && scale.is_some();
        let (h, col) = match scale {
            Some((max, col)) => (((bottom - top) * v / max).max(if *v > 0.0 { bw / 2.0 } else { 0.0 }), col),
            None => ((bottom - top) * 0.5, c.track()),
        };
        if h > 0.0 {
            c.rr(cx - bw / 2.0, bottom - h, bw, h, 3.0, col);
        } else {
            c.rr(cx - bw / 2.0, bottom - 3.0, bw, 3.0, 1.5, c.track());
        }
        let name = MONTHS[(*mo as usize).clamp(1, 12) - 1];
        let ink = if last { c.text1() } else { c.text3() };
        c.text_base(&name[..1], cx, label_base, style::MICRO.size, 600, ink, Align::Center, pitch);
    }
}

/// Loading skeleton of GitHub Overview at Small, Medium and Large.
fn rd_github_skeleton(c: &Canvas, badge: &Badge, now: &super::Now, sh: Shape) {
    if sh == Shape::Small {
        rd_hero_skeleton(c, badge, 0.5);
        return;
    }
    let m = style::M_CARD;
    header(c, badge, "GitHub", None);
    let cw = (c.w - 2.0 * m) / 3.0;
    for i in 0..3 {
        let x = m + i as f32 * cw;
        skeleton(c, x, 64.0 - 12.0, 40.0, 12.0);
        skeleton(c, x, 78.0 - 8.0, 48.0, 8.0);
    }
    if sh == Shape::Large {
        separator(c, style::SEP_Y_LARGE);
        // The same rolling 12 months the data shows, so nothing shifts when it arrives.
        let months: Vec<(u32, f32)> = (0..12u32).map(|i| ((now.month + 12 + i) % 12 + 1, 0.0)).collect();
        rd_month_bars(c, &months, None);
        skeleton(c, m, c.h - m - 8.0, 120.0, 8.0);
    }
}

/// Loading skeleton of a Small hero glance (§7): the badge, a hero bar at baseline 86 that
/// is `frac` of the text width, and a 40×8 caption bar at 106.
fn rd_hero_skeleton(c: &Canvas, badge: &Badge, frac: f32) {
    let m = style::M_SMALL;
    c.badge(badge, m, m, style::BADGE_SMALL);
    let ch = c.cap_h(style::HERO_S.size);
    skeleton(c, m, 86.0 - ch, (c.w - 2.0 * m) * frac, ch);
    skeleton(c, m, 106.0 - 8.0, 40.0, 8.0);
}

// ---------------------------------------------------------------- watchlist

fn watch(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, sh: Shape) {
    let badge = super::canvas::gly("\u{E9D2}", 0x30D158, 0xFFFFFF);
    let g = got(x, conn);
    if matches!(g, Got::Loading) && !matches!(sh, Shape::Mini | Shape::Dock) {
        // The default list has four symbols, so Large shows four rows over the chart.
        let m = if sh == Shape::Small { style::M_SMALL } else { style::M_CARD };
        let rows = rd_watch_rows(c, sh, if sh == Shape::Large { 4 } else { 3 });
        for (top, cap_w, _) in &rows {
            skeleton(c, m, top + (RD_CAPSULE_H - 10.0) / 2.0, 36.0, 10.0);
            c.rr(c.w - m - cap_w, *top, *cap_w, RD_CAPSULE_H, RD_CAPSULE_H / 2.0, c.track());
        }
        if sh == Shape::Large {
            let chart_bot = c.h - m - c.cap_h(style::FOOT.size) - style::SP[5];
            c.line(m, chart_bot - 0.5, c.w - m, chart_bot - 0.5, 1.0, c.sep());
            skeleton(c, m, c.h - m - 8.0, 72.0, 8.0);
            skeleton(c, c.w - m - 56.0, c.h - m - 8.0, 56.0, 8.0);
        }
        return;
    }
    if rd_placeholder(c, out, def.name, "", &badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, "https://finance.yahoo.com");
    let Some(first) = f.rows.first() else {
        if sh == Shape::Mini {
            compact_sum(c, &Sum::new(Vis::Badge(badge), "", ""), sh);
        } else if sh == Shape::Dock {
            rd_dock(c, &Vis::Badge(badge), DockText::new("—", "No symbols"), RdAcc::None);
        } else {
            rd_empty(c, sh, "\u{E9D2}", "Add symbols to follow");
        }
        return;
    };
    let (ch0, col0) = rd_change(c, first.delta.unwrap_or(0.0));
    match sh {
        Shape::Mini => {
            // The ticker is the mark; the day's change is the value.
            let k = c.h / 54.0;
            c.text_base(&first.label, c.w / 2.0, 18.0 * k, style::MICRO.size * k, 600, c.text2(), Align::Center, c.w - 2.0 * style::M_MINI * k);
            // Shrink, never round: compacting "+0.6%" would turn it into "+1%".
            let fs = fit(c, &ch0, style::MINI_VALUE.size * k, style::MINI_VALUE.min * k, 600, Font::Text, c.w - 2.0 * style::M_MINI * k);
            c.text_base(&ch0, c.w / 2.0, 45.0 * k, fs, 600, col0, Align::Center, c.w * 2.0);
        }
        Shape::Dock => {
            // Dock L adds the first symbol's day as a sparkline; the caption keeps
            // "TSLA +1.4%" as in Dock M, so the slot labels the chart rather than repeat it.
            let d = DockText { tag: Some((&ch0, col0)), ..DockText::new(&first.value, &first.label) };
            let acc = if f.series.len() > 2 { RdAcc::Spark(f.series.clone(), col0, "Today".into()) } else { RdAcc::Pair(ch0.clone(), col0, style::VALUE, "Today".into()) };
            rd_dock(c, &Vis::Badge(badge), d, acc);
        }
        _ => {
            let small = sh == Shape::Small;
            let m = if small { style::M_SMALL } else { style::M_CARD };
            let n = f.rows.len().min(if sh == Shape::Large { style::ROWS_MAX_L } else { 3 });
            let fs = style::TITLE.size;
            let lay = rd_watch_rows(c, sh, n);
            for (r, (top, cap_w, _)) in f.rows.iter().zip(lay.iter()) {
                let (t, col) = rd_change(c, r.delta.unwrap_or(0.0));
                let base = top + RD_CAPSULE_H / 2.0 + c.cap_h(fs) / 2.0;
                let tick_w = if small { c.w - 2.0 * m - cap_w - style::SP[3] } else { 64.0 };
                let ts = fit(c, &r.label, fs, style::TITLE.min, 600, Font::Text, tick_w);
                c.text_base(&r.label, m, base, ts, 600, c.text1(), Align::Left, tick_w);
                if !small {
                    let pr = c.w - m - cap_w - style::SP[6];
                    c.text_base(&r.value, pr, base, fs, 400, c.text1(), Align::Right, pr - m - tick_w - style::SP[3]);
                }
                rd_capsule(c, c.w - m, *top, *cap_w, RD_CAPSULE_H, &t, col);
            }
            if sh == Shape::Large && n < style::ROWS_MAX_L {
                let up = f.rows.iter().filter(|r| r.delta.unwrap_or(0.0) >= 0.05).count();
                let down = f.rows.iter().filter(|r| r.delta.unwrap_or(0.0) <= -0.05).count();
                // The feed's series is the first symbol's day (markets.rs watchlist()), the
                // same quote the Mini and Dock pills show.
                let sym = f.extra.get("symbol").cloned().unwrap_or_else(|| first.label.clone());
                let rows_end = lay.last().map(|l| l.0 + RD_CAPSULE_H).unwrap_or(m);
                let chart_top = rows_end + style::SP[6];
                let chart_bot = c.h - m - c.cap_h(style::FOOT.size) - style::SP[5];
                let right = if f.series.len() > 2 && !sym.is_empty() && chart_bot - chart_top >= 32.0 {
                    let col = up_down(f.delta_pct.unwrap_or(0.0));
                    c.chart(m, chart_top, c.w - 2.0 * m, chart_bot - chart_top, &f.series, col, ChartOpts { fill: true, dot: true, stroke: style::STROKE, smooth: f.series.len() >= 8 });
                    format!("{sym} today")
                } else {
                    String::new()
                };
                rd_foot(c, &format!("{up} up · {down} down"), &right);
            }
        }
    }
}

/// Watchlist row boxes: (capsule top, capsule width, pitch). Three rows fill the card
/// margin to margin on Small and Medium; Large uses Medium's pitch so its first three rows
/// are the Medium card's.
fn rd_watch_rows(c: &Canvas, sh: Shape, n: usize) -> Vec<(f32, f32, f32)> {
    let (m, cap_w) = if sh == Shape::Small { (style::M_SMALL, RD_CAPSULE_S) } else { (style::M_CARD, RD_CAPSULE_M) };
    let (bottom, span) = if sh == Shape::Large { (style::SEP_Y_LARGE - m, 3) } else { (c.h - m, n) };
    let pitch = if span < 2 { style::ROW_L } else { (bottom - m - RD_CAPSULE_H) / (span - 1) as f32 };
    (0..n).map(|i| (m + i as f32 * pitch, cap_w, pitch)).collect()
}

// ---------------------------------------------------------------- next event

fn next_event(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, sh: Shape) {
    let badge = super::canvas::gly(G_CAL, 0xFF3B30, 0xFFFFFF);
    let g = got(x, "calendar");
    if matches!(g, Got::Loading) && !matches!(sh, Shape::Mini | Shape::Dock) {
        rd_next_skeleton(c, &badge, sh);
        return;
    }
    if rd_placeholder(c, out, def.name, "", &badge, "calendar", &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    let has = f.extra.contains_key("next_title");
    let title = f.extra.get("next_title").cloned().unwrap_or_else(|| "Nothing scheduled".into());
    let when = f.extra.get("next_time").cloned().unwrap_or_default();
    let mins = f.extra.get("next_minutes").and_then(|m| m.parse::<i64>().ok()).filter(|_| has);
    let day = rd_when_day(&when);
    let today = day == "Today";
    let (_, time, _) = rd_split_when(&when);
    let time_str = rd_when_time(&when);
    let (soon, soon_short) = match mins {
        Some(m) if m <= 0 => ("Now".to_string(), "Now".to_string()),
        Some(m) if m < 60 => (format!("in {m} min"), format!("{m} min")),
        Some(m) if m < 24 * 60 => (format!("in {}h {}m", m / 60, m % 60), format!("{}h {}m", m / 60, m % 60)),
        _ => (String::new(), String::new()),
    };
    let caption = if !has { "Your calendar is clear".to_string() } else if today { time_str.clone() } else { when.clone() };
    match sh {
        Shape::Mini => {
            let v = match mins {
                Some(m) if (0..60).contains(&m) => format!("{m}m"),
                _ if !has => String::new(),
                _ if today && !time.is_empty() => time.to_string(),
                _ if day.chars().count() <= 5 => day.clone(),
                _ => String::new(),
            };
            compact_sum(c, &Sum::new(Vis::Badge(badge), v, ""), sh);
        }
        Shape::Dock => {
            let wide = c.w >= c.h * 3.4;
            let cap = if wide { if soon.is_empty() { "Up next".to_string() } else { soon.clone() } } else if soon.is_empty() { caption.clone() } else { soon.clone() };
            let d = DockText { name: true, ..DockText::new(&title, &cap) };
            let acc = if has { RdAcc::Pair(time_str.clone(), c.text1(), style::TITLE, if today { "Today".into() } else { day.clone() }) } else { RdAcc::None };
            rd_dock(c, &Vis::Badge(badge), d, acc);
        }
        Shape::Small => {
            let m = style::M_SMALL;
            rd_small_top(c, &Vis::Badge(badge), &soon_short, RED);
            let (lines, fs) = rd_wrap_fit(c, &title, style::VALUE.size, style::TITLE.size, 600, c.w - 2.0 * m);
            let lh = (fs * 1.2).round();
            let n = lines.len().min(2);
            for (i, l) in lines.iter().take(2).enumerate() {
                c.text_base(l, m, 86.0 - (n - 1 - i) as f32 * lh, fs, 600, c.text1(), Align::Left, c.w - 2.0 * m);
            }
            c.text_base(&caption, m, 106.0, style::CAPTION.size, 400, c.text2(), Align::Left, c.w - 2.0 * m);
        }
        _ => {
            let m = style::M_CARD;
            let bs = style::BADGE_GLANCE;
            c.badge(&badge, c.w - m - bs, m, bs);
            c.text_cap("Up next", m, m, style::FOOT.size, 600, RED, Align::Left, c.w - 2.0 * m - bs);
            let tw = c.w - 2.0 * m - bs - style::SP[3];
            let fs = fit(c, &title, style::VALUE.size, style::TITLE.size, 600, Font::Text, tw);
            c.text_base(&title, m, 48.0, fs, 600, c.text1(), Align::Left, tw);
            let cap = if has && today { format!("Today {time_str}") } else { caption.clone() };
            let mut cap_right = c.w - m;
            if !soon.is_empty() {
                let fs11 = style::FOOT.size;
                let pw = c.measure(&soon, fs11, 600, Font::Text).0 + 2.0 * style::SP[4];
                let ph = 20.0;
                let cy = 66.0 - c.cap_h(style::CAPTION.size) / 2.0;
                rd_capsule(c, c.w - m, cy - ph / 2.0, pw, ph, &soon, RED);
                cap_right = c.w - m - pw - style::SP[3];
            }
            c.text_base(&cap, m, 66.0, style::CAPTION.size, 400, c.text2(), Align::Left, cap_right - m);
            // The events after this one (the feed lists the next event first).
            let mut skipped = false;
            let after: Vec<_> = f
                .rows
                .iter()
                .filter(|r| {
                    if !skipped && r.label == title {
                        skipped = true;
                        return false;
                    }
                    true
                })
                .collect();
            let when_short = |v: &str| if rd_when_day(v) == "Today" { rd_when_time(v) } else { v.to_string() };
            // §3 third line on the bottom margin: what follows it.
            if has {
                let then = match after.first() {
                    Some(r) => format!("Then {} \u{00B7} {}", r.label, when_short(&r.value)),
                    None => "Nothing else scheduled".to_string(),
                };
                c.text_base(&then, m, c.h.min(style::SEP_Y_LARGE) - m, style::FOOT.size, 400, c.text3(), Align::Left, c.w - 2.0 * m);
            }
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let later: Vec<_> = after.iter().skip(if has { 1 } else { 0 }).take(3).collect();
                let b0 = style::DETAIL_TOP + c.cap_h(style::BODY.size);
                for (i, r) in later.iter().enumerate() {
                    rd_row(c, out, &LRow::new(&r.label, when_short(&r.value)), b0 + i as f32 * style::ROW_L, style::ROW_L);
                }
                // The footnote closes the region at the bottom margin.
                let n = f.value.unwrap_or(0.0) as i64;
                let foot = if later.is_empty() && has && !after.is_empty() { "Nothing after that".to_string() } else if later.is_empty() { "Nothing else scheduled".to_string() } else if n == 1 { "1 event today".to_string() } else { format!("{n} events today") };
                rd_foot(c, &foot, "");
            }
        }
    }
}

/// Loading skeleton of the Up Next layout (§7).
fn rd_next_skeleton(c: &Canvas, badge: &Badge, sh: Shape) {
    if sh == Shape::Small {
        let m = style::M_SMALL;
        c.badge(badge, m, m, style::BADGE_SMALL);
        let ch = c.cap_h(style::VALUE.size);
        skeleton(c, m, 86.0 - ch, (c.w - 2.0 * m) * 0.7, ch);
        skeleton(c, m, 106.0 - 8.0, 40.0, 8.0);
        return;
    }
    let m = style::M_CARD;
    let bs = style::BADGE_GLANCE;
    c.badge(badge, c.w - m - bs, m, bs);
    skeleton(c, m, m, 40.0, 8.0);
    let ch = c.cap_h(style::VALUE.size);
    skeleton(c, m, 48.0 - ch, (c.w - 2.0 * m - bs) * 0.6, ch);
    skeleton(c, m, 66.0 - 8.0, 40.0, 8.0);
    skeleton(c, m, c.h.min(style::SEP_Y_LARGE) - m - 8.0, 96.0, 8.0);
    if sh == Shape::Large {
        separator(c, style::SEP_Y_LARGE);
        let b0 = style::DETAIL_TOP + c.cap_h(style::BODY.size);
        for (i, w) in [0.55, 0.4].iter().enumerate() {
            let b = b0 + i as f32 * style::ROW_L;
            skeleton(c, m, b - 8.0, (c.w - 2.0 * m) * w, 8.0);
            skeleton(c, c.w - m - 40.0, b - 8.0, 40.0, 8.0);
        }
    }
}

// ---------------------------------------------------------------- AI usage

fn ai_all(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let src: [(&'static str, &str, &str, Col); 6] = [
        ("claude_code", "Claude Code", "Claude", CLAUDE),
        ("codex", "Codex", "Codex", hex(0x9AA0A6)),
        ("anthropic", "Claude API", "Anthropic", CLAUDE),
        ("openai", "OpenAI API", "OpenAI", hex(0x10A37F)),
        ("openrouter", "OpenRouter", "OpenRouter", hex(0x6467F2)),
        ("cursor", "Cursor", "Cursor", hex(0xBBBBBB)),
    ];
    struct Src<'a> {
        name: &'a str,
        short: &'a str,
        value: String,
        tokens: Option<f64>,
        col: Col,
        series: Vec<f64>,
        week: f64,
    }
    let mut rows: Vec<Src> = Vec::new();
    for (conn, name, short, col) in src {
        let f = match (x.hub.feed(conn), x.hub.err(conn)) {
            (Some(f), None) => Cow::Borrowed(f),
            _ if x.sample && (conn == "claude_code" || conn == "openai" || conn == "codex") => Cow::Owned(super::hub::sample(conn)),
            _ => continue,
        };
        let tokens = (f.unit == "tokens").then(|| f.value.unwrap_or(0.0));
        let value = match tokens {
            Some(t) => compact_tight(t),
            None => headline(&f),
        };
        let week = f.extra.get("tokens_week").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        rows.push(Src { name, short, value, tokens, col, series: if tokens.is_some() { f.series.clone() } else { Vec::new() }, week });
    }
    let badge = super::canvas::gly(G_AI, 0xD97757, 0xFFFFFF);
    if rows.is_empty() {
        let reason = match sh {
            Shape::Small | Shape::Dock | Shape::Mini => "Add an API key",
            _ => "Claude Code, Codex or an API key",
        };
        rd_placeholder(c, out, "AI Usage", "", &badge, "anthropic", &Got::Setup(reason.into()), sh);
        return;
    }
    let total: f64 = rows.iter().filter_map(|r| r.tokens).sum();
    let (key, caption) = if total > 0.0 { (compact_tight(total), "tokens today".to_string()) } else { (rows[0].value.clone(), rows[0].name.to_string()) };
    match sh {
        Shape::Mini | Shape::Dock => {
            out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Connect("anthropic")));
            if sh == Shape::Mini {
                compact_sum(c, &Sum::new(Vis::Badge(badge), key, ""), sh);
                return;
            }
            let acc = if total > 0.0 {
                let parts: Vec<(f32, Col)> = rows.iter().filter_map(|r| r.tokens.map(|t| ((t / total) as f32, r.col))).collect();
                let top = rows.iter().filter(|r| r.tokens.is_some()).max_by(|a, b| a.tokens.partial_cmp(&b.tokens).unwrap_or(std::cmp::Ordering::Equal));
                let label = top.map(|r| format!("{} {:.0}%", r.short, r.tokens.unwrap_or(0.0) / total * 100.0)).unwrap_or_default();
                RdAcc::Stack(parts, label)
            } else {
                RdAcc::Pair(rows.len().to_string(), c.text1(), style::VALUE, "tools".into())
            };
            rd_dock(c, &Vis::Badge(badge), DockText::new(&key, &caption), acc);
        }
        Shape::Small => {
            rd_small_top(c, &Vis::Badge(badge), "Today", c.text3());
            for (i, r) in rows.iter().take(style::ROWS_MAX_S).enumerate() {
                let mut lr = LRow::new(r.short, r.value.as_str());
                lr.lead = Lead::Dot(r.col);
                rd_row(c, out, &lr, RD_ROW1_S + i as f32 * style::ROW_S, style::ROW_S);
            }
        }
        _ => {
            rd_header(c, &badge, "AI Usage", "Today");
            let items: Vec<(String, String, Option<Col>)> = rows.iter().take(3).map(|r| (r.value.clone(), r.short.to_string(), Some(r.col))).collect();
            rd_stats(c, &items, 56.0, 76.0, true);
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let m = style::M_CARD;
                let len = rows.iter().map(|r| r.series.len()).max().unwrap_or(0);
                let mut sum = vec![0.0f64; len];
                for r in &rows {
                    let off = len - r.series.len();
                    for (i, v) in r.series.iter().enumerate() {
                        sum[off + i] += v;
                    }
                }
                let week: f64 = rows.iter().map(|r| r.week).sum();
                if len > 2 {
                    let top = style::DETAIL_TOP + style::SP[1];
                    let bot = c.h - m - c.cap_h(style::FOOT.size) - style::SP[6];
                    c.chart(m, top, c.w - 2.0 * m, bot - top, &sum, CLAUDE, ChartOpts { fill: true, dot: true, stroke: style::STROKE, smooth: len >= 8 });
                    rd_foot(c, &format!("Tokens · last {len} days"), &if week > 0.0 { format!("{} this week", compact_tight(week)) } else { String::new() });
                } else {
                    for (i, r) in rows.iter().skip(3).take(3).enumerate() {
                        let mut lr = LRow::new(r.name, r.value.as_str());
                        lr.lead = Lead::Dot(r.col);
                        rd_row(c, out, &lr, style::DETAIL_TOP + c.cap_h(style::BODY.size) + i as f32 * style::ROW_L, style::ROW_L);
                    }
                    rd_foot(c, &format!("{} tools connected", rows.len()), "");
                }
            }
        }
    }
}

// ---------------------------------------------------------------- revenue goal

/// "$10.0K" → "$10K": whole thousands and millions without ".0".
fn rd_money_whole(s: String) -> String {
    s.replace(".0K", "K").replace(".0M", "M")
}

fn goal(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let goal: f64 = x.cfg.widget_settings.get("revenue-goal").and_then(|m| m.get("goal")).and_then(|v| v.parse().ok()).unwrap_or(10_000.0);
    let names = [("stripe", "Stripe"), ("lemonsqueezy", "Lemon Squeezy"), ("paddle", "Paddle"), ("shopify", "Shopify"), ("gumroad", "Gumroad"), ("polar", "Polar")];
    let mut total = 0.0;
    let mut from: Vec<&str> = Vec::new();
    let mut window = String::new();
    for (conn, name) in names {
        let f = match (x.hub.feed(conn), x.hub.err(conn)) {
            (Some(f), None) => Cow::Borrowed(f),
            _ if x.sample && conn == "stripe" => Cow::Owned(super::hub::sample(conn)),
            _ => continue,
        };
        total += f.value.unwrap_or(0.0);
        from.push(name);
        if window.is_empty() {
            window = f.caption.clone();
        }
    }
    let badge = super::canvas::gly("\u{E7C1}", 0x30D158, 0xFFFFFF);
    if from.is_empty() {
        let reason = match sh {
            Shape::Small | Shape::Dock | Shape::Mini => "Connect Stripe",
            _ => "Connect Stripe or another provider",
        };
        rd_placeholder(c, out, "Revenue Goal", "Revenue", &badge, "revenue-goal", &Got::Setup(reason.into()), sh);
        return;
    }
    if window.is_empty() {
        window = "Last 30 days".into();
    }
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Connect("revenue-goal")));
    let frac = (total / goal.max(1.0)) as f32;
    let pct = format!("{:.0}%", (frac * 100.0).min(999.0));
    let total_s = money(total, Some("USD"));
    let goal_s = rd_money_whole(money(goal, Some("USD")));
    let left = goal - total;
    match sh {
        Shape::Mini => compact_sum(c, &Sum::new(Vis::Ring(frac, GREEN, pct), "", ""), sh),
        Shape::Dock => {
            let acc = if left > 0.0 { RdAcc::Pair(rd_money_whole(money(left, Some("USD"))), c.text1(), style::VALUE, "to go".into()) } else { RdAcc::Pair("Reached".into(), GREEN, RD_FOOT_SB, "goal".into()) };
            rd_dock(c, &Vis::Ring(frac, GREEN, String::new()), DockText::new(&total_s, &format!("{pct} of goal")), acc);
        }
        Shape::Small => {
            let m = style::M_SMALL;
            draw_vis(c, &Vis::Ring(frac, GREEN, pct), m, m, 44.0);
            let (v, fs) = fit_value(c, &total_s, style::HERO_S, c.w - 2.0 * m);
            c.text_base(&v, m, 86.0, fs, style::HERO_S.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base(&format!("of {goal_s} goal"), m, 106.0, style::CAPTION.size, 400, c.text2(), Align::Left, c.w - 2.0 * m);
        }
        _ => {
            let m = style::M_CARD;
            let ring = 80.0 - m;
            draw_vis(c, &Vis::Ring(frac, GREEN, pct), m, m, ring);
            let tx = m + ring + style::SP[6];
            let (v, fs) = fit_value(c, &total_s, style::HERO, c.w - m - tx);
            c.text_cap(&v, tx, m, fs, style::HERO.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base(&format!("of {goal_s} goal"), tx, 58.0, style::CAPTION.size, 400, c.text2(), Align::Left, c.w - m - tx);
            c.text_base(&window, tx, 76.0, style::FOOT.size, 400, c.text3(), Align::Left, c.w - m - tx);
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let days = if window.to_ascii_lowercase().contains("month") { x.now.day.max(1) as f64 } else { window.split_whitespace().find_map(|w| w.parse::<f64>().ok()).unwrap_or(30.0) };
                let per_day = total / days;
                let (to_go, pace) = if left <= 0.0 {
                    ("Reached".to_string(), "—".to_string())
                } else if per_day > 0.0 {
                    let (mo, d) = rd_add_days(&x.now, (left / per_day).ceil() as u32);
                    (rd_money_whole(money(left, Some("USD"))), format!("{} {d}", &MONTHS[(mo - 1) as usize][..3]))
                } else {
                    (rd_money_whole(money(left, Some("USD"))), "—".to_string())
                };
                let items = [(to_go, "To go".to_string(), None), (rd_money_whole(money(per_day, Some("USD"))), "Per day".to_string(), None), (pace, "At this pace".to_string(), None)];
                rd_stats(c, &items, 136.0, 152.0, false);
                rd_foot(c, &format!("From {}", from.join(", ")), "");
            }
        }
    }
}

/// (month, day) `n` days after today.
fn rd_add_days(now: &super::Now, n: u32) -> (u32, u32) {
    let (mut y, mut mo, mut d) = (now.year, now.month, now.day + n.min(3650));
    while d > super::days_in_month(y, mo) {
        d -= super::days_in_month(y, mo);
        mo += 1;
        if mo > 12 {
            mo = 1;
            y += 1;
        }
    }
    (mo, d)
}

// ---------------------------------------------------------------- wallpaper

/// The photo scrim: one gradient over the whole card shape (clear at the top, darkest at
/// the bottom), so no second shape leaves a notch along the card's side edges.
fn rd_scrim(c: &Canvas, r: f32) {
    c.rr_grad(0.0, 0.0, c.w, c.h, r, (0.0, 0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 0.50));
}

/// A stand-in photo for gallery previews (no image is downloaded there): an original dusk
/// sky with a low sun.
fn rd_sample_scene(c: &Canvas, r: f32) {
    c.rr_grad(0.0, 0.0, c.w, c.h, r, hex(0x2E4A7D), hex(0xE59A6B));
    let s = c.w.min(c.h) * 0.11;
    let (cx, cy) = (c.w * 0.7, c.h * 0.42);
    c.circle(cx, cy, s * 1.5, (1.0, 0.85, 0.6, 0.12));
    c.circle(cx, cy, s, (1.0, 0.9, 0.72, 0.95));
}

/// A fully round white glass button with a near-black label; returns its left edge.
fn rd_glass_pill(c: &Canvas, right: f32, bottom: f32, label: &str) -> f32 {
    let fs = style::CAPTION.size;
    let h = style::BTN_SECONDARY;
    let w = c.measure(label, fs, 600, Font::Text).0 + 2.0 * 14.0;
    c.rr(right - w, bottom - h, w, h, h / 2.0, RD_GLASS);
    c.text_cap(label, right - w / 2.0, bottom - h / 2.0 - c.cap_h(fs) / 2.0, fs, 600, RD_ON_GLASS, Align::Center, w);
    right - w
}

fn wallpaper(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, wic: &IWICImagingFactory, sh: Shape) {
    let badge = super::canvas::gly(G_IMAGE, 0x2C6BED, 0xFFFFFF);
    let g = got(x, "wallpaper");
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    let r = if dense { style::DOCK_R } else { style::CARD_R };
    let f = match &g {
        Got::Feed(f) => Some(f),
        Got::Loading => None,
        _ => {
            // paint() already drew the dock card; Launchpad cards draw their own.
            if !dense {
                bg(c, false);
            }
            rd_placeholder(c, out, def.name, "", &badge, "wallpaper", &g, sh);
            return;
        }
    };
    let img = f.and_then(|f| f.extra.get("image_path")).and_then(|p| x.hub.wallpaper_img.as_ref().filter(|w| &w.0 == p).map(|w| w.1.clone()).or_else(|| load_image(wic, p)));
    match (&img, f) {
        (Some(src), _) => c.image(src, 0.0, 0.0, c.w, c.h, r),
        (None, Some(_)) if x.sample => rd_sample_scene(c, r),
        _ => {
            // Loading, or today's image isn't downloaded yet: a skeleton of the photo card.
            if !dense {
                bg(c, false);
            }
            c.rr(0.0, 0.0, c.w, c.h, r, c.track());
            let k = if dense { c.h / 54.0 } else { 1.0 };
            let lower = matches!(sh, Shape::Medium | Shape::Large);
            c.glyph(G_IMAGE, c.w / 2.0, if lower { (c.h - style::BTN_SECONDARY) / 2.0 } else { c.h / 2.0 }, 24.0 * k, c.text3());
            if lower {
                let (w, h, m) = (96.0, style::BTN_SECONDARY, style::M_CARD);
                c.rr(c.w - m - w, c.h - m - h, w, h, h / 2.0, c.track());
            }
            return;
        }
    }
    let Some(f) = f else { return };
    if f.extra.contains_key("image_path") && !x.sample {
        out.hits.push(((0.0, 0.0, c.w, c.h), WAct::SetWallpaper));
    }
    let title = f.extra.get("title").cloned().filter(|t| !t.is_empty()).or_else(|| f.text.clone()).unwrap_or_else(|| "Image of the day".into());
    let desc = f.extra.get("copyright").map(|s| s.split(" (©").next().unwrap_or("").trim().to_string()).unwrap_or_default();
    let fs = style::TITLE.size;
    match sh {
        Shape::Mini => {}
        Shape::Dock => {
            if c.w >= c.h * 3.4 {
                let k = c.h / 54.0;
                let d = style::BTN_SECONDARY_DOCK * k;
                let cx = c.w - style::M_DOCK * k - d / 2.0;
                c.circle(cx, c.h / 2.0, d / 2.0, RD_GLASS);
                c.glyph(G_IMAGE, cx, c.h / 2.0, 12.0 * k, RD_ON_GLASS);
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            rd_scrim(c, r);
            let lines = rd_wrap(c, &title, fs, 600, c.w - 2.0 * m, 2);
            let n = lines.len();
            for (i, l) in lines.iter().enumerate() {
                c.text_base(l, m, 106.0 - (n - 1 - i) as f32 * style::ROW_S, fs, 600, WHITE, Align::Left, c.w - 2.0 * m);
            }
        }
        _ => {
            let m = style::M_CARD;
            rd_scrim(c, r);
            let px = rd_glass_pill(c, c.w - m, c.h - m, "Set wallpaper");
            let tw = px - style::SP[3] - m;
            let label_base = c.h - m - style::BTN_SECONDARY / 2.0 + c.cap_h(style::CAPTION.size) / 2.0;
            if sh == Shape::Medium || desc.is_empty() {
                // The title in up to 2 lines (13/600) in the column left of the pill, its
                // last baseline on the pill label's; it only ellipsizes past 2 lines.
                let (tl, size) = rd_wrap_fit(c, &title, fs, fs, 600, tw);
                let tlh = (size * 1.25).round();
                let tn = tl.len().min(2);
                for (i, l) in tl.iter().take(2).enumerate() {
                    c.text_base(l, m, label_base - (tn - 1 - i) as f32 * tlh, size, 600, WHITE, Align::Left, tw);
                }
            } else {
                // Large: the title (up to 2 lines, 15 → 13) over the description (up to 2
                // lines), bottom-aligned to the margin in the column left of the pill.
                let ds = style::FOOT.size;
                let lines = rd_wrap(c, &desc, ds, 400, tw, 2);
                let n = lines.len();
                let lh = 14.0;
                for (i, l) in lines.iter().enumerate() {
                    c.text_base(l, m, c.h - m - (n - 1 - i) as f32 * lh, ds, 400, (1.0, 1.0, 1.0, 0.72), Align::Left, tw);
                }
                let (tl, size) = rd_wrap_fit(c, &title, style::VALUE.size - 2.0, fs, 600, tw);
                let tlh = (size * 1.2).round();
                let tb = c.h - m - (n.max(1) - 1) as f32 * lh - c.cap_h(ds) - style::SP[3];
                let tn = tl.len().min(2);
                for (i, l) in tl.iter().take(2).enumerate() {
                    c.text_base(l, m, tb - (tn - 1 - i) as f32 * tlh, size, 600, WHITE, Align::Left, tw);
                }
            }
        }
    }
    c.rr_stroke(0.0, 0.0, c.w, c.h, r, c.card_edge(), 1.0);
}

/// Decode an image file, scaled down for widget use.
pub fn load_image(wic: &IWICImagingFactory, path: &str) -> Option<IWICBitmapSource> {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::*;
    unsafe {
        let dec = wic.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnLoad).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let scaler = wic.CreateBitmapScaler().ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        let k = (640.0 / w.max(1) as f32).min(1.0);
        scaler.Initialize(&frame, ((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1), WICBitmapInterpolationModeFant).ok()?;
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let bmp = wic.CreateBitmapFromSource(&conv, WICBitmapCacheOnLoad).ok()?;
        bmp.cast().ok()
    }
}

pub fn load_image_bytes(wic: &IWICImagingFactory, bytes: &[u8]) -> Option<IWICBitmapSource> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Imaging::*;
    unsafe {
        let stream = wic.CreateStream().ok()?;
        stream.InitializeFromMemory(bytes).ok()?;
        let dec = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let bmp = wic.CreateBitmapFromSource(&conv, WICBitmapCacheOnLoad).ok()?;
        bmp.cast().ok()
    }
}

// ------------------------------------------------------------------ clocks
// Clocks and calendars (docs/widget-design.md §5). Shared rules: time heroes use the
// clock weights (`style::time_weight`), "AM/PM" is Micro top-aligned to the digits' cap top
// 3 px after them, seconds appear only on Large, dials have a rim, and red marks only today.

fn h12(h: u32) -> u32 {
    if h % 12 == 0 {
        12
    } else {
        h % 12
    }
}
fn ampm(h: u32) -> &'static str {
    if h < 12 {
        "AM"
    } else {
        "PM"
    }
}

/// Segoe Fluent Icons cloud: the weather icon while there is no weather.
const G_CLOUD: &str = "\u{E753}";
/// Night indigo (the moon on world clocks), and its darker tone for light cards.
const NIGHT_INDIGO: Col = hex(0x8E8EFF);
const INDIGO_LIGHT: Col = hex(0x5E5CE6);
/// A calendar page is white in both themes, so its ink is always the light-theme ink.
const PAGE: Col = (0.98, 0.98, 0.99, 1.0);
const PAGE_INK: Col = hex(0x1C1C1E);
const PAGE_INK2: Col = (0.235, 0.235, 0.263, 0.72);
/// Analog faces: (face, ink) for a dark and a light face.
const FACE_DARK: (Col, Col) = ((0.11, 0.11, 0.13, 1.0), (1.0, 1.0, 1.0, 0.95));
const FACE_LIGHT: (Col, Col) = ((0.97, 0.97, 0.98, 1.0), hex(0x1C1C1E));

/// Today's disc in calendars: red, and the darker text red on light cards so the white
/// numeral keeps its contrast.
fn today_fill(c: &Canvas) -> Col {
    if c.light {
        hex(0xD70015)
    } else {
        RED
    }
}

/// A filled polygon (Canvas has rounded rectangles and circles only).
fn poly(c: &Canvas, pts: &[(f32, f32)], col: Col) {
    use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED};
    if pts.len() < 3 || col.3 <= 0.0 {
        return;
    }
    let v = |p: (f32, f32)| windows_numerics::Vector2 { X: p.0, Y: p.1 };
    unsafe {
        let Ok(g) = c.factory.CreatePathGeometry() else { return };
        let Ok(s) = g.Open() else { return };
        s.BeginFigure(v(pts[0]), D2D1_FIGURE_BEGIN_FILLED);
        for p in &pts[1..] {
            s.AddLine(v(*p));
        }
        s.EndFigure(D2D1_FIGURE_END_CLOSED);
        let _ = s.Close();
        let t = c.tone(col, false);
        if let Ok(b) = c.dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: t.0, g: t.1, b: t.2, a: t.3 }, None) {
            c.dc.FillGeometry(&g, &b, None);
        }
    }
}

/// A rectangle with rounded top corners and a square bottom edge, drawn as one shape (so a
/// band over a page has no seam).
#[allow(clippy::too_many_arguments)]
fn rr_top(c: &Canvas, x: f32, y: f32, w: f32, h: f32, r: f32, col: Col) {
    let r = r.min(w / 2.0).min(h).max(0.0);
    let steps = 10;
    let mut pts = vec![(x, y + h)];
    for (ox, a0) in [(x + r, 180.0f32), (x + w - r, 270.0)] {
        for i in 0..=steps {
            let a = (a0 + 90.0 * i as f32 / steps as f32).to_radians();
            pts.push((ox + r * a.cos(), y + r + r * a.sin()));
        }
    }
    pts.push((x + w, y + h));
    poly(c, &pts, col);
}

/// A time drawn by its baseline, with "AM/PM" in Micro top-aligned to the digits' cap top,
/// 3 px after them (k scales the dock sizes). Returns the right edge of the group.
#[allow(clippy::too_many_arguments)]
fn time_ap(c: &Canvas, t: &str, ap: &str, x: f32, base: f32, size: f32, weight: u32, col: Col, k: f32) -> f32 {
    c.text_base(t, x, base, size, weight, col, Align::Left, c.w * 2.0);
    let tw = c.measure(t, size, weight, Font::Text).0;
    if ap.is_empty() {
        return x + tw;
    }
    let ms = style::MICRO.size * k;
    let ax = x + tw + 3.0 * k;
    c.text_cap(ap, ax, base - c.cap_h(size), ms, style::MICRO.weight, c.text2(), Align::Left, 40.0 * k);
    ax + c.measure(ap, ms, style::MICRO.weight, Font::Text).0
}

/// The width `time_ap` draws.
fn time_ap_w(c: &Canvas, t: &str, ap: &str, size: f32, weight: u32, k: f32) -> f32 {
    let tw = c.measure(t, size, weight, Font::Text).0;
    if ap.is_empty() {
        tw
    } else {
        tw + 3.0 * k + c.measure(ap, style::MICRO.size * k, style::MICRO.weight, Font::Text).0
    }
}

/// The first label that fits `max_w` (else the last one, which may then truncate).
fn fit_label(c: &Canvas, cands: &[String], size: f32, weight: u32, max_w: f32) -> String {
    cands.iter().find(|s| c.measure(s, size, weight, Font::Text).0 <= max_w).or(cands.last()).cloned().unwrap_or_default()
}

/// ISO week number (weeks start on Monday; week 1 holds the first Thursday).
fn iso_week(n: &super::Now) -> u32 {
    let doy = n.day_of_year() as i32;
    let wd = ((n.weekday + 6) % 7 + 1) as i32;
    let jan1 = (n.weekday as i32 - (doy - 1)).rem_euclid(7);
    // A year has 53 ISO weeks when it starts on a Thursday, or a Wednesday in a leap year.
    let weeks = |jan1: i32, leap: bool| if jan1 == 4 || (leap && jan1 == 3) { 53 } else { 52 };
    let leap = |y: u32| super::days_in_month(y, 2) == 29;
    let w = (doy - wd + 10) / 7;
    if w < 1 {
        let py = n.year - 1;
        let pdays = if leap(py) { 366 } else { 365 };
        weeks((jan1 - pdays).rem_euclid(7), leap(py)) as u32
    } else if w > weeks(jan1, leap(n.year)) {
        1
    } else {
        w as u32
    }
}

fn days_left(n: &super::Now) -> u32 {
    n.days_in_year() - n.day_of_year()
}

/// The (month, day) `off` days from today.
fn shift_day(n: &super::Now, off: i64) -> (u32, u32) {
    let dim = n.days_in_month() as i64;
    let pm = if n.month == 1 { 12 } else { n.month - 1 };
    let nm = if n.month == 12 { 1 } else { n.month + 1 };
    let d = n.day as i64 + off;
    if d < 1 {
        (pm, (d + super::days_in_month(n.year, pm) as i64) as u32)
    } else if d > dim {
        (nm, (d - dim) as u32)
    } else {
        (n.month, d as u32)
    }
}

/// "Nov 15 – 21", or "Nov 29 – Dec 5" across months: this week, Sunday to Saturday.
fn week_range(n: &super::Now) -> String {
    let (m0, d0) = shift_day(n, -(n.weekday as i64));
    let (m1, d1) = shift_day(n, 6 - n.weekday as i64);
    let short = |m: u32| &MONTHS[m as usize - 1][..3];
    if m0 == m1 {
        format!("{} {d0} \u{2013} {d1}", short(m0))
    } else {
        format!("{} {d0} \u{2013} {} {d1}", short(m0), short(m1))
    }
}

/// An upcoming event from the calendar feed: title, day word ("Today", "Tue") and time.
struct Ev {
    title: String,
    day: String,
    time: String,
}

/// The calendar's upcoming events (sample events in previews); None without a calendar.
fn events(x: &Ctx) -> Option<Vec<Ev>> {
    let Got::Feed(f) = got(x, "calendar") else { return None };
    Some(
        f.rows
            .iter()
            .map(|r| {
                let (day, time) = match r.value.split_once(' ') {
                    Some((d, t)) if t.contains(':') && !d.contains(':') => (d.to_string(), t.to_string()),
                    _ => (String::new(), r.value.clone()),
                };
                Ev { title: r.label.clone(), day, time }
            })
            .collect(),
    )
}

/// An event's time with its day unless it is today: "2:00 PM", "Tue 9:00 AM".
fn ev_when(e: &Ev) -> String {
    if e.day.is_empty() || e.day.eq_ignore_ascii_case("today") {
        e.time.clone()
    } else {
        format!("{} {}", e.day, e.time)
    }
}

/// Hours between local time and a zone's (h, m, weekday), and that zone's day word.
fn zone_diff(n: &super::Now, h: u32, m: u32, wd: u32) -> (f32, &'static str) {
    let mut d = (h as i32 * 60 + m as i32) - (n.hour as i32 * 60 + n.minute as i32);
    let name = if wd == n.weekday {
        "Today"
    } else if wd == (n.weekday + 1) % 7 {
        d += 24 * 60;
        "Tomorrow"
    } else {
        d -= 24 * 60;
        "Yesterday"
    };
    (d as f32 / 60.0, name)
}

/// "+14h", "−3h" (U+2212), "+5.5h", "0h".
fn fmt_hours(d: f32) -> String {
    if d.abs() < 0.01 {
        return "0h".into();
    }
    let a = d.abs();
    let v = if (a - a.round()).abs() < 0.01 { format!("{a:.0}") } else { format!("{a:.1}") };
    format!("{}{v}h", if d > 0.0 { "+" } else { "\u{2212}" })
}

/// Short city names for labels and two- or three-letter codes for inside a dial.
const CITIES: [(&str, &str, &str); 18] = [
    ("San Francisco", "SF", "SF"),
    ("New York", "NYC", "NYC"),
    ("New York City", "NYC", "NYC"),
    ("Los Angeles", "LA", "LA"),
    ("Washington", "DC", "DC"),
    ("London", "London", "LON"),
    ("Tokyo", "Tokyo", "TYO"),
    ("Paris", "Paris", "PAR"),
    ("Sydney", "Sydney", "SYD"),
    ("Berlin", "Berlin", "BER"),
    ("Singapore", "Singapore", "SIN"),
    ("Hong Kong", "Hong Kong", "HK"),
    ("Dubai", "Dubai", "DXB"),
    ("Chicago", "Chicago", "CHI"),
    ("Toronto", "Toronto", "TOR"),
    ("Vancouver", "Vancouver", "VAN"),
    ("Mexico City", "Mexico City", "MEX"),
    ("Rio de Janeiro", "Rio", "RIO"),
];

/// A city's short label: the curated short form, else the full name if it fits, else its
/// first word. Never cut mid-word.
fn city_short(c: &Canvas, label: &str, size: f32, weight: u32, max_w: f32) -> String {
    if let Some((_, s, _)) = CITIES.iter().find(|(full, _, _)| full.eq_ignore_ascii_case(label.trim())) {
        return s.to_string();
    }
    if c.measure(label, size, weight, Font::Text).0 <= max_w {
        return label.to_string();
    }
    label.split_whitespace().next().unwrap_or(label).to_string()
}

/// A city's code for inside a small dial: curated, else initials, else its first letters.
fn city_code(label: &str) -> String {
    if let Some((_, _, code)) = CITIES.iter().find(|(full, _, _)| full.eq_ignore_ascii_case(label.trim())) {
        return code.to_string();
    }
    let words: Vec<&str> = label.split_whitespace().collect();
    if words.len() > 1 {
        words.iter().take(3).filter_map(|w| w.chars().next()).collect::<String>().to_uppercase()
    } else {
        label.chars().take(3).collect::<String>().to_uppercase()
    }
}

/// The time in a world-clock city: (h, m, s, weekday), local time if the zone is unknown.
fn city_time(n: &super::Now, key: &str) -> (u32, u32, u32, u32) {
    super::tz_now(key).unwrap_or((n.hour, n.minute, n.second, n.weekday))
}

/// This PC's Windows time zone ("Pacific Standard Time").
fn local_zone() -> Option<String> {
    use windows::Win32::System::Time::{GetDynamicTimeZoneInformation, DYNAMIC_TIME_ZONE_INFORMATION};
    let mut z = DYNAMIC_TIME_ZONE_INFORMATION::default();
    unsafe { GetDynamicTimeZoneInformation(&mut z) };
    let k = &z.TimeZoneKeyName;
    let name = String::from_utf16_lossy(&k[..k.iter().position(|&ch| ch == 0).unwrap_or(k.len())]);
    (!name.is_empty()).then_some(name)
}

/// An analog dial: face with a rim, ticks (a minute track from 90 px), hands from the centre
/// (only the seconds hand has a counterweight) and a machined hub. `code` is a city code
/// printed inside the face; `k` scales it on the dock.
#[allow(clippy::too_many_arguments)]
fn dial(c: &Canvas, cx: f32, cy: f32, r: f32, h: u32, m: u32, s: Option<u32>, dark: bool, code: Option<&str>, k: f32) {
    let (face, ink) = if dark { FACE_DARK } else { FACE_LIGHT };
    c.circle(cx, cy, r, face);
    // A 1 px rim so the face keeps its edge on the glass in both themes.
    let rim = match (c.light, dark) {
        (true, false) => Some(c.sep()),
        (true, true) => Some((0.0, 0.0, 0.0, 0.12)),
        (false, true) => Some(c.sep()),
        (false, false) => None,
    };
    if let Some(col) = rim {
        c.circle_stroke(cx, cy, r - 0.5, col, 1.0);
    }
    let tick = |i: u32, n: u32, r0: f32, r1: f32, t: f32, col: Col| {
        let a = (i as f32 * 360.0 / n as f32).to_radians();
        c.line(cx + r0 * a.sin(), cy - r0 * a.cos(), cx + r1 * a.sin(), cy - r1 * a.cos(), t, col);
    };
    if r * 2.0 >= 90.0 {
        // Minute track, then hour ticks; round caps kept inside the rim.
        for i in (0..60).filter(|i| i % 5 != 0) {
            tick(i, 60, r * 0.92 + 0.5, r * 0.97 - 0.5, 1.0, alpha(ink, 0.30));
        }
        let t = (r * 0.029).max(1.5);
        for i in 0..12 {
            tick(i, 12, r * 0.84 + t / 2.0, r * 0.97 - t / 2.0, t, alpha(ink, 0.85));
        }
    } else {
        for i in 0..12 {
            if i % 3 == 0 {
                let t = (r * 0.07).max(1.5);
                tick(i, 12, r * 0.72 + t / 2.0, r * 0.92 - t / 2.0, t, alpha(ink, 0.8));
            } else {
                let t = (r * 0.04).max(1.0);
                tick(i, 12, r * 0.8 + t / 2.0, r * 0.92 - t / 2.0, t, alpha(ink, 0.55));
            }
        }
    }
    if let Some(code) = code {
        let fs = style::MICRO.size * k;
        c.text_cap(code, cx, cy + r * 0.42 - c.cap_h(fs) / 2.0, fs, style::MICRO.weight, alpha(ink, 0.55), Align::Center, r * 2.0);
    }
    let hand = |deg: f32, len: f32, tail: f32, t: f32, col: Col| {
        let a = deg.to_radians();
        c.line(cx - tail * a.sin(), cy + tail * a.cos(), cx + len * a.sin(), cy - len * a.cos(), t, col);
    };
    hand((h % 12) as f32 * 30.0 + m as f32 * 0.5, r * 0.5, 0.0, (r * 0.085).max(1.5), ink);
    hand(m as f32 * 6.0 + s.unwrap_or(0) as f32 * 0.1, r * 0.78, 0.0, (r * 0.06).max(1.2), ink);
    if let Some(s) = s {
        hand(s as f32 * 6.0, r * 0.86, r * 0.18, (r * 0.022).max(1.0), ORANGE);
    }
    c.circle(cx, cy, (r * 0.075).max(2.0), ink);
    c.circle(cx, cy, (r * 0.045).max(1.2), ORANGE);
    if r >= 40.0 {
        c.circle(cx, cy, r * 0.018, c.halo());
    }
}

/// The analog face on a rounded panel (w × h, corner `rad`), with numerals at 12, 3, 6 and 9
/// on the panel's inner ellipse when there's room.
#[allow(clippy::too_many_arguments)]
fn square_dial(c: &Canvas, x0: f32, y0: f32, w: f32, h: f32, rad: f32, hh: u32, mm: u32, sec: u32) {
    let (face, ink) = if c.light { FACE_LIGHT } else { FACE_DARK };
    c.rr(x0, y0, w, h, rad, face);
    c.rr_stroke(x0, y0, w, h, rad, if c.light { c.sep() } else { (1.0, 1.0, 1.0, 0.08) }, 1.0);
    let s = w.min(h);
    let (cx, cy) = (x0 + w / 2.0, y0 + h / 2.0);
    if s >= 60.0 {
        let fs = s * 0.13;
        for (num, dx, dy) in [("12", 0.0, -1.0), ("3", 1.0, 0.0), ("6", 0.0, 1.0), ("9", -1.0, 0.0)] {
            let (px, py) = (cx + dx * w * 0.37, cy + dy * h * 0.37);
            c.text_cap(num, px, py - c.cap_h(fs) / 2.0, fs, 500, alpha(ink, 0.85), Align::Center, fs * 3.0);
        }
    } else {
        for i in 0..4 {
            let a = (i as f32 * 90.0).to_radians();
            c.circle(cx + w * 0.4 * a.sin(), cy - h * 0.4 * a.cos(), (s * 0.03).max(1.0), alpha(ink, 0.7));
        }
    }
    let hand = |deg: f32, len: f32, tail: f32, t: f32, col: Col| {
        let a = deg.to_radians();
        c.line(cx - tail * a.sin(), cy + tail * a.cos(), cx + len * a.sin(), cy - len * a.cos(), t, col);
    };
    hand((hh % 12) as f32 * 30.0 + mm as f32 * 0.5, s * 0.20, 0.0, (s * 0.045).max(1.5), ink);
    hand(mm as f32 * 6.0 + sec as f32 * 0.1, s * 0.30, 0.0, (s * 0.032).max(1.2), ink);
    hand(sec as f32 * 6.0, s * 0.30, s * 0.06, (s * 0.015).max(1.0), ORANGE);
    c.circle(cx, cy, (s * 0.034).max(2.0), ink);
    c.circle(cx, cy, (s * 0.02).max(1.2), ORANGE);
    if s >= 80.0 {
        c.circle(cx, cy, s * 0.008, c.halo());
    }
}

/// One seven-segment digit: chamfered segments with a hairline gap at the joins.
#[allow(clippy::too_many_arguments)]
fn seg_digit(c: &Canvas, x: f32, y: f32, w: f32, h: f32, d: u32, on: Col, off: Col) {
    // a b c d e f g
    const SEGS: [u8; 10] = [0x3F, 0x06, 0x5B, 0x4F, 0x66, 0x6D, 0x7D, 0x07, 0x7F, 0x6F];
    let m = SEGS[d as usize % 10];
    let ht = w * 0.09;
    let gp = (ht * 0.25).clamp(0.4, 1.2);
    let (xl, xr) = (x + ht, x + w - ht);
    let (yt, ym, yb) = (y + ht, y + h / 2.0, y + h - ht);
    let hseg = |yc: f32| vec![(xl + gp, yc), (xl + gp + ht, yc - ht), (xr - gp - ht, yc - ht), (xr - gp, yc), (xr - gp - ht, yc + ht), (xl + gp + ht, yc + ht)];
    let vseg = |xc: f32, ya: f32, yz: f32| vec![(xc, ya + gp), (xc + ht, ya + gp + ht), (xc + ht, yz - gp - ht), (xc, yz - gp), (xc - ht, yz - gp - ht), (xc - ht, ya + gp + ht)];
    let segs = [hseg(yt), vseg(xr, yt, ym), vseg(xr, ym, yb), hseg(yb), vseg(xl, ym, yb), vseg(xl, yt, ym), hseg(ym)];
    for (i, p) in segs.iter().enumerate() {
        poly(c, p, if m & (1 << i) != 0 { on } else { off });
    }
}

/// The leading hour digit of an LCD time: lit, a ghost "8", or left out.
#[derive(Clone, Copy, PartialEq)]
enum LcdLead {
    Digit,
    Ghost,
    None,
}

/// Width of an LCD time `dh` tall with `nd` digits (a colon after the hours).
fn seg_w(dh: f32, nd: usize) -> f32 {
    let dw = dh * 0.55;
    nd as f32 * dw + (nd as f32 - 2.0) * dw * 0.25 + dw * 0.75
}

/// An LCD time from (x0, y0), `dh` tall. The colon sits centred in its gutter.
#[allow(clippy::too_many_arguments)]
fn seg_time(c: &Canvas, x0: f32, y0: f32, dh: f32, d: [u32; 4], lead: LcdLead, on: Col, off: Col, colon: Col) {
    let dw = dh * 0.55;
    let mut sx = x0;
    for (i, v) in d.iter().enumerate() {
        if i == 0 && lead == LcdLead::None {
            continue;
        }
        if i == 2 {
            let cx = sx - dw * 0.25 + dw * 0.75 / 2.0;
            let r = (dw * 0.1).max(1.0);
            c.circle(cx, y0 + dh * 0.3, r, colon);
            c.circle(cx, y0 + dh * 0.7, r, colon);
            sx += dw * 0.75 - dw * 0.25;
        }
        if i == 0 && lead == LcdLead::Ghost {
            seg_digit(c, sx, y0, dw, dh, 8, off, off);
        } else {
            seg_digit(c, sx, y0, dw, dh, *v, on, off);
        }
        sx += dw * 1.25;
    }
}

/// This week as a strip: weekday letters over dates, today's letter red and its date on a
/// red disc. `month_scope` greys the days outside this month.
#[allow(clippy::too_many_arguments)]
fn week_row(c: &Canvas, n: &super::Now, x0: f32, w: f32, letter_base: f32, date_base: f32, ls: f32, fs: f32, weight: u32, disc: f32, month_scope: bool) {
    let colw = w / 7.0;
    for i in 0..7u32 {
        let off = i as i64 - n.weekday as i64;
        let (m, d) = shift_day(n, off);
        let cx = x0 + colw * (i as f32 + 0.5);
        let today = off == 0;
        c.text_base(&DAYS[i as usize][..1], cx, letter_base, ls, style::MICRO.weight, if today { RED } else { c.text3() }, Align::Center, colw);
        if today {
            c.circle(cx, date_base - c.cap_h(fs) / 2.0, disc / 2.0, today_fill(c));
        }
        let col = if today {
            WHITE
        } else if month_scope && m != n.month {
            c.text3()
        } else {
            c.text1()
        };
        c.text_base(&d.to_string(), cx, date_base, fs, if today { 700 } else { weight }, col, Align::Center, colw.max(disc));
    }
}

/// This month as a grid: weekday letters, then a row per week; weekends in text2, today on
/// a red disc.
#[allow(clippy::too_many_arguments)]
fn month_grid(c: &Canvas, n: &super::Now, x0: f32, w: f32, letter_base: f32, first_base: f32, pitch: f32, fs: f32, weight: u32, disc: f32) {
    let colw = w / 7.0;
    for (i, d) in ["S", "M", "T", "W", "T", "F", "S"].iter().enumerate() {
        let today = i as u32 == n.weekday;
        c.text_base(d, x0 + colw * (i as f32 + 0.5), letter_base, style::MICRO.size, style::MICRO.weight, if today { RED } else { c.text3() }, Align::Center, colw);
    }
    let first = n.first_weekday();
    for day in 1..=n.days_in_month() {
        let idx = first + day - 1;
        let col = idx % 7;
        let cx = x0 + colw * (col as f32 + 0.5);
        let base = first_base + pitch * (idx / 7) as f32;
        let today = day == n.day;
        if today {
            c.circle(cx, base - c.cap_h(fs) / 2.0, disc / 2.0, today_fill(c));
        }
        let ink = if today {
            WHITE
        } else if col == 0 || col == 6 {
            c.text2()
        } else {
            c.text1()
        };
        c.text_base(&day.to_string(), cx, base, fs, if today { 700 } else { weight }, ink, Align::Center, colw.max(disc));
    }
}

/// Rows in this month's grid.
fn month_rows(n: &super::Now) -> u32 {
    (n.first_weekday() + n.days_in_month()).div_ceil(7)
}

/// A tear-off calendar page `s` square: a white tile with a red band (one shape, no seam),
/// the month in the band and the day under it. Light cards give it a hairline.
#[allow(clippy::too_many_arguments)]
fn day_tile(c: &Canvas, n: &super::Now, x: f32, y: f32, s: f32, r: f32, band: f32, mfs: f32, nfs: f32) {
    c.rr(x, y, s, s, r, PAGE);
    rr_top(c, x, y, s, band, r, RED);
    let mon = MONTHS[n.month as usize - 1][..3].to_uppercase();
    c.text_cap(&mon, x + s / 2.0, y + (band - c.cap_h(mfs)) / 2.0, mfs, 700, WHITE, Align::Center, s);
    // Optical centre: a pixel above the middle of the white.
    let mid = y + band + (s - band) / 2.0 - 1.0;
    c.text_cap(&n.day.to_string(), x + s / 2.0, mid - c.cap_h(nfs) / 2.0, nfs, 600, PAGE_INK, Align::Center, s);
    if c.light {
        c.rr_stroke(x, y, s, s, r, c.sep(), 1.0);
    }
}

/// The Mini day tile shared by the calendar faces: 44, centred, concentric with the pill.
fn mini_tile(c: &Canvas, n: &super::Now, k: f32) {
    let s = 44.0 * k;
    day_tile(c, n, (c.w - s) / 2.0, (c.h - s) / 2.0, s, style::concentric(style::DOCK_R, 5.0) * k, 14.0 * k, style::MICRO.size * k, 22.0 * k);
}

/// Everything the clock faces share for one paint.
struct Cl {
    n: super::Now,
    time: String,
    ap: &'static str,
    mon: &'static str,
    day: &'static str,
    /// Dock scale (the spec is drawn at a 54 px dock).
    k: f32,
    /// Dock L: the wide pill with an accessory slot.
    wide: bool,
}

impl Cl {
    /// "Mon Nov 16"
    fn date_short(&self) -> String {
        format!("{} {} {}", &self.day[..3], &self.mon[..3], self.n.day)
    }
    /// "Monday, November 16"
    fn date_long(&self) -> String {
        format!("{}, {} {}", self.day, self.mon, self.n.day)
    }
    /// The Dock L accessory slot: (x, width).
    fn slot(&self, c: &Canvas) -> (f32, f32) {
        let w = (c.w * 0.34).round();
        (c.w - style::M_DOCK * self.k - w, w)
    }
}

fn clock(c: &Canvas, out: &mut Painted, kind: ClockKind, x: &Ctx, sh: Shape) {
    let n = x.now;
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    let t = Cl {
        n,
        time: format!("{}:{:02}", h12(n.hour), n.minute),
        ap: ampm(n.hour),
        mon: MONTHS[n.month as usize - 1],
        day: DAYS[n.weekday as usize],
        k: if dense { c.h / 54.0 } else { 1.0 },
        wide: sh == Shape::Dock && c.w >= c.h * 3.4,
    };
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("ms-settings:dateandtime".into())));
    match kind {
        ClockKind::Digital => digital(c, x, &t, sh, false),
        ClockKind::ClockWeather => digital(c, x, &t, sh, true),
        ClockKind::Flip => flip(c, &t, sh),
        ClockKind::World => world(c, x, &t, sh),
        ClockKind::WorldTime => world_time(c, x, &t, sh),
        ClockKind::WeekStrip => week(c, x, &t, sh),
        ClockKind::Today => today(c, x, &t, sh),
        ClockKind::Date => date(c, &t, sh),
        ClockKind::DayPage => day_page(c, &t, sh),
        ClockKind::Analog => analog(c, &t, sh),
        ClockKind::AnalogSquare => analog_square(c, &t, sh),
        ClockKind::Segment => segment(c, &t, sh),
        ClockKind::Month => month(c, &t, sh),
    }
}

/// A weather icon in a `size` box: the condition's emoji, or a quiet cloud glyph when there
/// is no weather (so nothing moves when it arrives).
fn wx_icon(c: &Canvas, wx: Option<&laminar_sys::weather::Weather>, x: f32, y: f32, size: f32) {
    match wx {
        Some(w) => draw_vis(c, &Vis::Emoji(weather_emoji(w.code, w.is_day)), x, y, size),
        None => c.glyph(G_CLOUD, x + size / 2.0, y + size / 2.0, size * 0.7, c.text3()),
    }
}

/// Today's date as a small day tile for a Dock L slot: "MON" over "16", right-aligned.
fn day_slot(c: &Canvas, t: &Cl) {
    let k = t.k;
    let r = c.w - style::M_DOCK * k;
    c.text_base(&t.day[..3].to_uppercase(), r, 24.0 * k, style::MICRO.size * k, style::MICRO.weight, RED, Align::Right, 40.0 * k);
    c.text_base(&t.n.day.to_string(), r, 44.0 * k, style::VALUE.size * k, style::VALUE.weight, c.text1(), Align::Right, 40.0 * k);
}

/// Where a dock text column must end: before the day tile on Dock L, else the margin.
fn day_slot_left(c: &Canvas, t: &Cl) -> f32 {
    let k = t.k;
    let r = c.w - style::M_DOCK * k;
    if !t.wide {
        return r;
    }
    let w = c.measure(&t.day[..3].to_uppercase(), style::MICRO.size * k, style::MICRO.weight, Font::Text).0.max(c.measure(&t.n.day.to_string(), style::VALUE.size * k, style::VALUE.weight, Font::Text).0);
    r - w - style::SP[5] * k
}

/// A 1 px separator before a Dock L slot.
fn slot_rule(c: &Canvas, t: &Cl) {
    let k = t.k;
    let (sx, _) = t.slot(c);
    c.rr((sx - 8.0 * k).round(), 14.0 * k, 1.0, 26.0 * k, 0.0, c.sep());
}

/// The time (Value + AM/PM) over a caption in a dock text column from `x0`.
fn dock_time(c: &Canvas, t: &Cl, x0: f32, caption: &str, right: f32) {
    let k = t.k;
    time_ap(c, &t.time, t.ap, x0, 30.0 * k, style::VALUE.size * k, style::VALUE.weight, c.text1(), k);
    c.text_base(caption, x0, 44.0 * k, style::FOOT.size * k, style::FOOT.weight, c.text2(), Align::Left, right - x0);
}

/// Up to `max` event rows from baseline `base0`: time column, a calendar-colour bar, title.
fn event_rows(c: &Canvas, evs: &[&Ev], base0: f32, max: usize) -> usize {
    let m = style::M_CARD;
    let shown: Vec<&&Ev> = evs.iter().take(max).collect();
    let tcol = shown.iter().map(|e| c.measure(&ev_when(e), style::FOOT.size, 500, Font::Text).0).fold(52.0f32, f32::max) + style::SP[3];
    for (i, e) in shown.iter().enumerate() {
        let base = base0 + i as f32 * style::ROW_L;
        c.text_base(&ev_when(e), m, base, style::FOOT.size, 500, c.text2(), Align::Left, tcol);
        let bx = m + tcol;
        c.rr(bx, base - 11.0, 3.0, 13.0, 1.5, BLUE);
        c.text_base(&e.title, bx + 8.0, base, style::BODY.size, style::BODY.weight, c.text1(), Align::Left, c.w - m - bx - 8.0);
    }
    shown.len()
}

/// Large fallback: Week / Day of year / Days left.
fn year_stats(c: &Canvas, n: &super::Now, label_base: f32) {
    let (wk, doy, left) = (iso_week(n).to_string(), n.day_of_year().to_string(), days_left(n).to_string());
    let top = label_base - style::SP[6] - c.cap_h(style::VALUE.size);
    stat_footer(c, [(wk.as_str(), "Week"), (doy.as_str(), "Day of year"), (left.as_str(), "Days left")], top);
}

/// Clock and Clock & Weather.
fn digital(c: &Canvas, x: &Ctx, t: &Cl, sh: Shape, with_wx: bool) {
    let n = &t.n;
    let k = t.k;
    let wx = if with_wx { x.weather } else { None };
    let temp = wx.map(|w| format!("{:.0}°", d0(w.temp_c))).unwrap_or_else(|| "--°".into());
    let temp_col = if wx.is_some() { c.text1() } else { c.text3() };
    let hi_lo = wx.map(|w| format!("H {:.0}° L {:.0}°", d0(w.high_c), d0(w.low_c))).unwrap_or_else(|| "H --° L --°".into());
    let md = format!("{} {}", &t.mon[..3], n.day);
    match sh {
        Shape::Mini => {
            if with_wx {
                // Value-led: the weather mark over the time.
                let v = style::BADGE_MINI * k;
                wx_icon(c, wx, (c.w - v) / 2.0, 8.0 * k, v);
                let role = Type { size: style::MINI_VALUE.size * k, min: style::MINI_VALUE.min * k, ..style::MINI_VALUE };
                let (s, fs) = fit_value(c, &t.time, role, c.w - 2.0 * style::M_MINI * k);
                c.text_base(&s, c.w / 2.0, 45.0 * k, fs, role.weight, c.text1(), Align::Center, c.w * 2.0);
            } else {
                let fs = fit(c, &t.time, 15.0 * k, 12.0 * k, 600, Font::Text, c.w - 2.0 * style::M_MINI * k);
                c.text_base(&t.time, c.w / 2.0, 32.0 * k, fs, 600, c.text1(), Align::Center, c.w * 2.0);
                c.text_base(t.ap, c.w / 2.0, 45.0 * k, style::MICRO.size * k, style::MICRO.weight, c.text3(), Align::Center, c.w);
            }
        }
        Shape::Dock => {
            let m = style::M_DOCK * k;
            let (sx, sw) = t.slot(c);
            let right = if t.wide { sx - 16.0 * k } else { c.w - m };
            if !with_wx {
                dock_time(c, t, m, &t.date_short(), right);
                if t.wide {
                    // This week as dots: weekday letters, today in red.
                    let colw = sw / 7.0;
                    for i in 0..7u32 {
                        let cx = sx + colw * (i as f32 + 0.5);
                        let today = i == n.weekday;
                        let ms = style::MICRO.size * k;
                        c.text_cap(&DAYS[i as usize][..1], cx, 14.0 * k, ms, style::MICRO.weight, if today { RED } else { c.text3() }, Align::Center, colw);
                        c.circle(cx, 33.0 * k, 2.0 * k, if today { RED } else { c.track() });
                    }
                }
            } else if t.wide {
                dock_time(c, t, m, &format!("{}, {md}", &t.day[..3]), right);
                slot_rule(c, t);
                let vs = style::VALUE.size * k;
                let tw = c.measure(&temp, vs, style::VALUE.weight, Font::Text).0;
                c.text_base(&temp, c.w - m, 30.0 * k, vs, style::VALUE.weight, temp_col, Align::Right, sw);
                let is = 18.0 * k;
                wx_icon(c, wx, c.w - m - tw - style::SP[1] * k - is, 30.0 * k - c.cap_h(vs) / 2.0 - is / 2.0, is);
                c.text_base(&hi_lo, c.w - m, 44.0 * k, style::FOOT.size * k, style::FOOT.weight, c.text2(), Align::Right, sw);
            } else {
                time_ap(c, &t.time, t.ap, m, 30.0 * k, style::VALUE.size * k, style::VALUE.weight, c.text1(), k);
                // Caption: icon, temperature (never cut), then the condition (may truncate).
                let fs = style::FOOT.size * k;
                let is = 12.0 * k;
                wx_icon(c, wx, m, 44.0 * k - c.cap_h(fs) / 2.0 - is / 2.0, is);
                let tx = m + is + style::SP[1] * k;
                c.text_base(&temp, tx, 44.0 * k, fs, style::FOOT.weight, c.text2(), Align::Left, c.w * 2.0);
                let cx = tx + c.measure(&temp, fs, style::FOOT.weight, Font::Text).0 + style::SP[1] * k;
                if let Some(w) = wx {
                    c.text_base(&w.description, cx, 44.0 * k, fs, style::FOOT.weight, c.text2(), Align::Left, c.w - m - cx);
                }
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            if with_wx {
                wx_icon(c, wx, m, 12.0, 24.0);
                c.text_base(&temp, c.w - m, 30.0, style::VALUE.size, style::VALUE.weight, temp_col, Align::Right, 60.0);
            } else {
                c.text_cap(&t.day[..3].to_uppercase(), m, 16.0, style::MICRO.size, style::MICRO.weight, RED, Align::Left, 60.0);
            }
            let aw = c.measure(t.ap, style::MICRO.size, style::MICRO.weight, Font::Text).0 + 3.0;
            let fs = fit(c, &t.time, 40.0, if with_wx { 26.0 } else { 30.0 }, 300, Font::Text, c.w - 2.0 * m - aw);
            time_ap(c, &t.time, t.ap, m, 86.0, fs, style::time_weight(fs), c.text1(), 1.0);
            let cap = if with_wx { format!("{}, {md}", &t.day[..3]) } else { format!("{} {}", t.mon, n.day) };
            c.text_base(&cap, m, 106.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, c.w - 2.0 * m);
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let hs = style::HERO_XL.size;
            let base = m + c.cap_h(hs);
            time_ap(c, &t.time, t.ap, m, base, hs, style::time_weight(hs), c.text1(), 1.0);
            let large = sh == Shape::Large;
            if large && !with_wx {
                // Seconds stack under AM/PM, on the hero's baseline.
                let tw = c.measure(&t.time, hs, style::time_weight(hs), Font::Text).0;
                c.text_base(&format!(":{:02}", n.second), m + tw + 3.0, base, style::BODY.size, 400, c.text3(), Align::Left, 40.0);
            }
            if with_wx {
                let is = 28.0;
                wx_icon(c, wx, c.w - m - is, 14.0, is);
                let ts = 20.0;
                let tw = c.measure(&temp, ts, style::time_weight(ts), Font::Text).0;
                c.text_base(&temp, c.w - m, 64.0, ts, style::time_weight(ts), temp_col, Align::Right, 80.0);
                let cw = c.w - 2.0 * m - tw - style::SP[4];
                let cap = fit_label(c, &[t.date_long(), format!("{}, {md}", &t.day[..3])], style::CAPTION.size, 400, cw);
                c.text_base(&cap, m, 64.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, cw);
                let third = wx.map(|w| format!("{} · {hi_lo}", w.description)).unwrap_or_else(|| "Weather unavailable".into());
                c.text_base(&third, m, 80.0, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, c.w - 2.0 * m);
            } else {
                // The date, and this week in the right accessory box.
                let bx = 148.0;
                let cap = fit_label(c, &[t.date_long(), format!("{}, {md}", &t.day[..3])], style::CAPTION.size, 400, bx - m - style::SP[3]);
                c.text_base(&cap, m, 70.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, bx - m - style::SP[3]);
                week_row(c, n, bx, c.w - m - bx, 48.0 + c.cap_h(style::MICRO.size), 75.0, style::MICRO.size, style::MICRO.size, 500, 16.0, false);
            }
            if large {
                separator(c, style::SEP_Y_LARGE);
                let base0 = style::DETAIL_TOP + 16.0;
                if with_wx {
                    // The next hours: hour, icon, temperature in six columns.
                    let (rx, _, rw, _) = detail_region(c);
                    let cw = rw / 6.0;
                    for i in 0..6usize {
                        let cx = rx + cw * (i as f32 + 0.5);
                        let hour = (n.hour as usize + i) % 24;
                        let (label, w_i, tt) = match wx {
                            Some(w) if i == 0 => ("Now".to_string(), Some((w.code, w.is_day)), format!("{:.0}°", d0(w.temp_c))),
                            Some(w) if w.hourly.len() >= i => {
                                let (hh, tc, code) = w.hourly[i - 1];
                                (format!("{} {}", h12(hh as u32), ampm(hh as u32)), Some((code, (6..19).contains(&hh))), format!("{:.0}°", d0(tc)))
                            }
                            _ => (if i == 0 { "Now".into() } else { format!("{} {}", h12(hour as u32), ampm(hour as u32)) }, None, "--°".into()),
                        };
                        c.text_base(&label, cx, base0, style::FOOT.size, 500, c.text2(), Align::Center, cw);
                        let is = 22.0;
                        match w_i {
                            Some((code, day)) => draw_vis(c, &Vis::Emoji(weather_emoji(code, day)), cx - is / 2.0, 134.0, is),
                            None => c.glyph(G_CLOUD, cx, 134.0 + is / 2.0, is * 0.7, c.text3()),
                        }
                        c.text_base(&tt, cx, 178.0, style::TITLE.size, style::TITLE.weight, if w_i.is_some() { c.text1() } else { c.text3() }, Align::Center, cw);
                    }
                } else {
                    match events(x).filter(|e| !e.is_empty()) {
                        Some(evs) => {
                            event_rows(c, &evs.iter().collect::<Vec<_>>(), base0, 3);
                        }
                        None => year_stats(c, n, 164.0),
                    }
                }
            }
        }
    }
}

/// One split-flap: a dark flap with a lighter upper half, a hinge split and notches, an
/// optional character (cap height 62% of the flap) and an "AM/PM" in its bottom-left.
#[allow(clippy::too_many_arguments)]
fn flap(c: &Canvas, x: f32, y: f32, w: f32, h: f32, r: f32, ch: Option<&str>, fs: f32, ap: Option<&str>, k: f32) {
    let fill = if c.light { hex(0x2C2C2E) } else { (0.06, 0.06, 0.075, 1.0) };
    if c.light {
        // Soft 0/1/2 shadow so the flap sits on the card rather than in it.
        c.rr(x - 0.5, y + 1.0, w + 1.0, h + 1.0, r + 0.5, (0.0, 0.0, 0.0, 0.06));
        c.rr(x, y + 1.0, w, h, r, (0.0, 0.0, 0.0, 0.12));
    }
    c.rr(x, y, w, h, r, fill);
    rr_top(c, x, y, w, h / 2.0, r, (1.0, 1.0, 1.0, 0.04));
    if let Some(s) = ch {
        c.text_cap(s, x + w / 2.0, y + (h - c.cap_h(fs)) / 2.0, fs, 600, (0.97, 0.97, 0.98, 1.0), Align::Center, w * 1.5);
    }
    if let Some(ap) = ap {
        let ms = style::MICRO.size * k;
        c.text_base(ap, x + 4.0 * k, y + h - 4.0 * k, ms, style::MICRO.weight, (1.0, 1.0, 1.0, 0.55), Align::Left, w);
    }
    // The split: a gap in the card colour, with a notch at each side.
    let hy = y + h / 2.0;
    c.rr(x, hy - 0.5, w, 1.0, 0.0, if c.light { (0.0, 0.0, 0.0, 0.5) } else { c.halo() });
    if h >= 40.0 * k {
        c.rr(x, hy - 1.5 * k, 2.0 * k, 3.0 * k, 0.0, c.halo());
        c.rr(x + w - 2.0 * k, hy - 1.5 * k, 2.0 * k, 3.0 * k, 0.0, c.halo());
    }
    if c.light {
        c.rr_stroke(x - 1.0, y - 1.0, w + 2.0, h + 2.0, r + 1.0, (0.0, 0.0, 0.0, 0.10), 1.0);
    }
}

/// The font size whose cap height is 62% of a flap `h` tall.
fn flap_fs(c: &Canvas, h: f32) -> f32 {
    let guess = h * 0.62 / 0.7;
    h * 0.62 / (c.cap_h(guess) / guess)
}

fn flip(c: &Canvas, t: &Cl, sh: Shape) {
    let n = &t.n;
    let k = t.k;
    let hr = h12(n.hour);
    // A 12-hour clock has no leading zero: that flap stays blank, like a real split-flap.
    let digits = [(hr >= 10).then(|| (hr / 10).to_string()), Some((hr % 10).to_string()), Some((n.minute / 10).to_string()), Some((n.minute % 10).to_string())];
    match sh {
        Shape::Mini | Shape::Small => {
            // Square spaces: hours above minutes.
            let mini = sh == Shape::Mini;
            let (pad, gap) = if mini { (6.0 * k, 2.0 * k) } else { (style::M_SMALL, style::SP[2]) };
            let r = if mini { style::concentric(style::DOCK_R, 6.0) * k } else { style::concentric(style::CARD_R, style::M_SMALL) };
            let fw = (c.w - 2.0 * pad - gap) / 2.0;
            let fh = (c.h - 2.0 * pad - gap) / 2.0;
            let fs = flap_fs(c, fh);
            for (i, d) in digits.iter().enumerate() {
                let (col, row) = ((i % 2) as f32, (i / 2) as f32);
                let ap = (!mini && i == 0).then_some(t.ap);
                flap(c, pad + col * (fw + gap), pad + row * (fh + gap), fw, fh, r, d.as_deref(), fs, ap, k);
            }
        }
        Shape::Dock => {
            // Dock L keeps Dock M's flaps and adds AM/PM and the date in its slot.
            let (pad, gap, mid) = (6.0 * k, 2.0 * k, 9.0 * k);
            let span = c.w.min(2.6 * c.h);
            let fw = (span - 2.0 * pad - 2.0 * gap - mid) / 4.0;
            let fh = c.h - 2.0 * pad;
            let fs = flap_fs(c, fh).min(fw * 1.3);
            let x0 = if t.wide { pad } else { (c.w - (4.0 * fw + 2.0 * gap + mid)) / 2.0 };
            flap_row(c, &digits, x0, pad, fw, fh, gap, mid, style::concentric(style::DOCK_R, 6.0) * k, fs, None, k);
            if t.wide {
                let r = c.w - style::M_DOCK * k;
                c.text_base(t.ap, r, 24.0 * k, style::MICRO.size * k, style::MICRO.weight, c.text2(), Align::Right, 60.0 * k);
                c.text_base(&format!("{} {}", &t.day[..3], n.day), r, 40.0 * k, style::FOOT.size * k, 600, c.text1(), Align::Right, 60.0 * k);
            }
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let (gap, mid) = (style::SP[2], style::SP[6]);
            let fw = (c.w - 2.0 * m - 2.0 * gap - mid) / 4.0;
            let fh = 80.0 - m;
            let r = style::concentric(style::CARD_R, m);
            flap_row(c, &digits, m, m, fw, fh, gap, mid, r, flap_fs(c, fh), Some(t.ap), 1.0);
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                // The date as a smaller split-flap board: MON · 16 · NOV.
                let groups = [t.day[..3].to_uppercase(), n.day.to_string(), t.mon[..3].to_uppercase()];
                let (bw, bh, bg_, gg) = (22.0, 34.0, style::SP[0], style::SP[5]);
                let total: f32 = groups.iter().map(|g| g.chars().count() as f32 * (bw + bg_) - bg_).sum::<f32>() + gg * 2.0;
                let mut bx = (c.w - total) / 2.0;
                let by = 120.0;
                for g in &groups {
                    for ch in g.chars() {
                        flap(c, bx, by, bw, bh, style::R_MIN, Some(&ch.to_string()), 20.0, None, 1.0);
                        bx += bw + bg_;
                    }
                    bx += gg - bg_;
                }
                c.text_base(&t.date_long(), c.w / 2.0, 178.0, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Center, c.w - 2.0 * m);
            }
        }
    }
}

/// Four flaps in a row (hours, colon gutter, minutes); AM/PM inside the first hour flap.
#[allow(clippy::too_many_arguments)]
fn flap_row(c: &Canvas, digits: &[Option<String>; 4], x0: f32, y: f32, fw: f32, fh: f32, gap: f32, mid: f32, r: f32, fs: f32, ap: Option<&str>, k: f32) {
    let mut fx = x0;
    for (i, d) in digits.iter().enumerate() {
        if i == 2 {
            let cx = fx - gap + mid / 2.0;
            let rr = (mid * 0.16).max(1.5);
            c.circle(cx, y + fh * 0.36, rr, c.text2());
            c.circle(cx, y + fh * 0.64, rr, c.text2());
            fx += mid - gap;
        }
        flap(c, fx, y, fw, fh, r.max(style::R_MIN * k), d.as_deref(), fs, if i == 0 { ap } else { None }, k);
        fx += fw + if i == 1 { 0.0 } else { gap };
    }
}

fn world(c: &Canvas, x: &Ctx, t: &Cl, sh: Shape) {
    let n = &t.n;
    let k = t.k;
    let cities = super::world_cities(x.cfg);
    let night = |h: u32| !(6..18).contains(&h);
    match sh {
        Shape::Mini | Shape::Dock => {
            let nc = match sh {
                Shape::Mini => 1,
                _ if t.wide => 3,
                _ => 2,
            }
            .min(cities.len());
            let (x0, x1) = if sh == Shape::Mini { (0.0, c.w) } else { (style::M_DOCK * k, c.w - style::M_DOCK * k) };
            let slot = (x1 - x0) / nc as f32;
            let r = if sh == Shape::Mini { 20.0 * k } else { 19.0 * k };
            for (i, (label, key)) in cities.iter().take(nc).enumerate() {
                let (h, m, s, _) = city_time(n, key);
                dial(c, x0 + slot * (i as f32 + 0.5), c.h / 2.0, r, h, m, Some(s), night(h), Some(&city_code(label)), k);
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let (sw, gap) = (46.0, style::SP[3]);
            for (i, (label, key)) in cities.iter().take(2).enumerate() {
                let (h, mm, s, wd) = city_time(n, key);
                let cx = m + sw / 2.0 + i as f32 * (sw + gap);
                dial(c, cx, m + sw / 2.0, sw / 2.0, h, mm, Some(s), night(h), None, 1.0);
                let tw = sw + gap;
                c.text_base(&city_short(c, label, style::FOOT.size, 600, tw), cx, 78.0, style::FOOT.size, 600, c.text1(), Align::Center, tw);
                c.text_base(&format!("{}:{:02} {}", h12(h), mm, ampm(h)), cx, 94.0, style::FOOT.size, 400, c.text2(), Align::Center, tw);
                c.text_base(&fmt_hours(zone_diff(n, h, mm, wd).0), cx, 108.0, style::MICRO.size, style::MICRO.weight, c.text3(), Align::Center, tw);
            }
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let slot = (c.w - 2.0 * m) / 3.0;
            let r = 25.0;
            for (i, (label, key)) in cities.iter().take(3).enumerate() {
                let (h, mm, s, _) = city_time(n, key);
                let cx = m + slot * (i as f32 + 0.5);
                dial(c, cx, m - 2.0 + r, r, h, mm, Some(s), night(h), None, 1.0);
                c.text_base(&city_short(c, label, style::FOOT.size, 600, slot - 4.0), cx, 80.0, style::FOOT.size, 600, c.text1(), Align::Center, slot);
            }
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                zone_rows(c, n, &cities.iter().take(3).collect::<Vec<_>>());
            }
        }
    }
}

/// Large world-clock rows from the detail top, one per 24: the city, its day and offset,
/// and its time (Value + AM/PM) right-aligned. The offset column follows the longest name.
fn zone_rows(c: &Canvas, n: &super::Now, cities: &[&(String, String)]) {
    let m = style::M_CARD;
    let names_w = cities.iter().map(|(l, _)| c.measure(l, style::BODY.size, style::BODY.weight, Font::Text).0).fold(0.0f32, f32::max);
    let ox = (m + names_w + style::SP[5]).min(112.0);
    for (i, (label, key)) in cities.iter().enumerate() {
        let (h, mm, _, wd) = city_time(n, key);
        let base = style::DETAIL_TOP + 16.0 + i as f32 * style::ROW_L;
        let (diff, dname) = zone_diff(n, h, mm, wd);
        let tm = format!("{}:{:02}", h12(h), mm);
        let vs = style::VALUE.size;
        let gw = time_ap_w(c, &tm, ampm(h), vs, style::VALUE.weight, 1.0);
        time_ap(c, &tm, ampm(h), c.w - m - gw, base, vs, style::VALUE.weight, c.text1(), 1.0);
        c.text_base(label, m, base, style::BODY.size, style::BODY.weight, c.text1(), Align::Left, ox - m - style::SP[2]);
        c.text_base(&format!("{dname}, {}", fmt_hours(diff)), ox, base, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, c.w - m - gw - ox - style::SP[1]);
    }
}

fn world_time(c: &Canvas, x: &Ctx, t: &Cl, sh: Shape) {
    let n = &t.n;
    let k = t.k;
    let cities = super::world_cities(x.cfg);
    let hero = 2.min(cities.len().saturating_sub(1));
    let (label, key) = cities.get(hero).cloned().unwrap_or(("Tokyo".into(), "Tokyo Standard Time".into()));
    let (h, m, _, wd) = city_time(n, &key);
    let (diff, dname) = zone_diff(n, h, m, wd);
    let tm = format!("{}:{:02}", h12(h), m);
    let tap = ampm(h);
    let offset = format!("{dname}, {}", fmt_hours(diff));
    // Day or night there: the one mark that says this isn't your clock.
    let (g, gcol) = if (6..18).contains(&h) { (G_SUN, if c.light { ORANGE } else { YELLOW }) } else { (G_MOON, if c.light { INDIGO_LIGHT } else { NIGHT_INDIGO }) };
    match sh {
        Shape::Mini => {
            let gs = 18.0 * k;
            c.glyph(g, c.w / 2.0, 8.0 * k + gs / 2.0, gs, gcol);
            let role = Type { size: style::MINI_VALUE.size * k, min: style::MINI_VALUE.min * k, ..style::MINI_VALUE };
            let (s, fs) = fit_value(c, &tm, role, c.w - 2.0 * style::M_MINI * k);
            c.text_base(&s, c.w / 2.0, 45.0 * k, fs, role.weight, c.text1(), Align::Center, c.w * 2.0);
        }
        Shape::Dock => {
            let mg = style::M_DOCK * k;
            let ps = style::BADGE_DOCK * k;
            c.rr(mg, mg, ps, ps, ps * style::BADGE_R, c.plate());
            c.glyph(g, mg + ps / 2.0, mg + ps / 2.0, 16.0 * k, gcol);
            let x0 = mg + ps + style::SP[4] * k;
            let (sx, sw) = t.slot(c);
            let right = if t.wide { sx - 16.0 * k } else { c.w - mg };
            time_ap(c, &tm, tap, x0, 30.0 * k, style::VALUE.size * k, style::VALUE.weight, c.text1(), k);
            c.text_base(&label, x0, 44.0 * k, style::FOOT.size * k, style::FOOT.weight, c.text2(), Align::Left, right - x0);
            if t.wide {
                c.text_base(&fmt_hours(diff), c.w - mg, 30.0 * k, style::VALUE.size * k, style::VALUE.weight, c.text1(), Align::Right, sw);
                c.text_base(dname, c.w - mg, 44.0 * k, style::FOOT.size * k, style::FOOT.weight, c.text2(), Align::Right, sw);
            }
        }
        Shape::Small => {
            let mg = style::M_SMALL;
            let ts = style::TITLE.size;
            c.glyph(g, mg + 6.0, 16.0 + c.cap_h(ts) / 2.0, 12.0, gcol);
            c.text_cap(&label, mg + 16.0, 16.0, ts, style::TITLE.weight, c.text1(), Align::Left, c.w - 2.0 * mg - 16.0);
            let aw = c.measure(tap, style::MICRO.size, style::MICRO.weight, Font::Text).0 + 3.0;
            let fs = fit(c, &tm, 36.0, 26.0, 300, Font::Text, c.w - 2.0 * mg - aw);
            time_ap(c, &tm, tap, mg, 86.0, fs, style::time_weight(fs), c.text1(), 1.0);
            c.text_base(&offset, mg, 106.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, c.w - 2.0 * mg);
        }
        Shape::Medium | Shape::Large => {
            let mg = style::M_CARD;
            let ts = style::TITLE.size;
            // Here: your own time, for comparison.
            let here = format!("{} {}", t.time, t.ap);
            let hs = 20.0;
            let hw = c.measure(&here, hs, style::time_weight(hs), Font::Text).0;
            c.text_base("Here", c.w - mg, 58.0, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Right, 60.0);
            c.text_base(&here, c.w - mg, 80.0, hs, style::time_weight(hs), c.text2(), Align::Right, hw + 4.0);
            let left_w = c.w - 2.0 * mg - hw - style::SP[6];
            c.glyph(g, mg + 6.0, 16.0 + c.cap_h(ts) / 2.0, 12.0, gcol);
            c.text_cap(&label, mg + 16.0, 16.0, ts, style::TITLE.weight, c.text1(), Align::Left, c.w - 2.0 * mg - 16.0);
            let hero_s = 40.0;
            time_ap(c, &tm, tap, mg, 62.0, hero_s, style::time_weight(hero_s), c.text1(), 1.0);
            c.text_base(&offset, mg, 80.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, left_w);
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let others: Vec<&(String, String)> = cities.iter().enumerate().filter(|(i, _)| *i != hero).map(|(_, v)| v).take(3).collect();
                zone_rows(c, n, &others);
                if others.len() < 3 {
                    let base = style::DETAIL_TOP + 16.0 + others.len() as f32 * style::ROW_L;
                    let note = local_zone().map(|z| format!("Here: {z}")).unwrap_or_else(|| "Times update automatically".into());
                    c.text_base(&note, mg, base, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, c.w - 2.0 * mg);
                }
            }
        }
    }
}

/// The week strip in a dock pill (week and month): Dock L adds the month and week number.
fn dock_week(c: &Canvas, t: &Cl) {
    let k = t.k;
    let n = &t.n;
    if t.wide {
        let m = style::M_DOCK * k;
        c.text_base(&t.mon[..3].to_uppercase(), m, 22.0 * k, style::MICRO.size * k, style::MICRO.weight, c.text1(), Align::Left, 40.0 * k);
        c.text_base(&format!("Wk {}", iso_week(n)), m, 40.0 * k, style::FOOT.size * k, style::FOOT.weight, c.text2(), Align::Left, 40.0 * k);
        c.rr((52.0 * k).round(), 14.0 * k, 1.0, 26.0 * k, 0.0, c.sep());
        let x0 = 60.0 * k;
        week_row(c, n, x0, c.w - m - x0, 20.0 * k, 42.0 * k, style::MICRO.size * k, 13.0 * k, 600, 22.0 * k, false);
    } else {
        let x0 = 10.0 * k;
        week_row(c, n, x0, c.w - 2.0 * x0, 20.0 * k, 41.0 * k, style::MICRO.size * k, style::FOOT.size * k, 600, 18.0 * k, false);
    }
}

/// Header template for the calendar faces: title at the left, meta at the right.
fn cal_header(c: &Canvas, title: &str, meta: &str) {
    let m = style::M_CARD;
    let mw = c.measure(meta, style::FOOT.size, style::FOOT.weight, Font::Text).0;
    c.text_base(meta, c.w - m, style::HEADER_BASE, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Right, mw + 4.0);
    c.text_base(title, m, style::HEADER_BASE, style::TITLE.size, style::TITLE.weight, c.text1(), Align::Left, c.w - 2.0 * m - mw - style::SP[3]);
}

/// The Medium week strip (week and month).
fn medium_week(c: &Canvas, n: &super::Now, dy: f32, month_scope: bool) {
    let m = style::M_CARD;
    week_row(c, n, m, c.w - 2.0 * m, 52.0 + dy, 77.0 + dy, style::MICRO.size, 15.0, 600, 24.0, month_scope);
}

fn week(c: &Canvas, x: &Ctx, t: &Cl, sh: Shape) {
    let n = &t.n;
    match sh {
        Shape::Mini => mini_tile(c, n, t.k),
        Shape::Dock => dock_week(c, t),
        Shape::Small => {
            let m = style::M_SMALL;
            c.text_base(&format!("Week {}", iso_week(n)), m, 28.0, style::TITLE.size, style::TITLE.weight, c.text1(), Align::Left, c.w - 2.0 * m);
            c.text_base(&week_range(n), m, 44.0, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, c.w - 2.0 * m);
            week_row(c, n, m, c.w - 2.0 * m, 84.0, 104.0, style::MICRO.size, style::MICRO.size, 600, 16.0, false);
        }
        Shape::Medium | Shape::Large => {
            cal_header(c, t.mon, &format!("Week {}", iso_week(n)));
            medium_week(c, n, 0.0, false);
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let m = style::M_CARD;
                match events(x).filter(|e| !e.is_empty()) {
                    Some(evs) => {
                        // A day chip ("TODAY", "TUE"), the title, the time at the right.
                        let chip = |e: &Ev| match e.day.to_lowercase().as_str() {
                            "" => String::new(),
                            d if d.chars().count() > 5 => d.chars().take(3).collect::<String>().to_uppercase(),
                            d => d.to_uppercase(),
                        };
                        let shown: Vec<&Ev> = evs.iter().take(3).collect();
                        let cw = shown.iter().map(|e| c.measure(&chip(e), style::MICRO.size, style::MICRO.weight, Font::Text).0).fold(0.0f32, f32::max);
                        let tx = (m + cw + style::SP[3]).max(52.0);
                        for (i, e) in shown.iter().enumerate() {
                            let base = style::DETAIL_TOP + 16.0 + i as f32 * style::ROW_L;
                            c.text_base(&chip(e), m, base, style::MICRO.size, style::MICRO.weight, c.text3(), Align::Left, cw + 4.0);
                            let tw = c.measure(&e.time, style::FOOT.size, 500, Font::Text).0;
                            c.text_base(&e.time, c.w - m, base, style::FOOT.size, 500, c.text2(), Align::Right, tw + 4.0);
                            c.text_base(&e.title, tx, base, style::BODY.size, style::BODY.weight, c.text1(), Align::Left, c.w - m - tw - style::SP[3] - tx);
                        }
                    }
                    None => year_stats(c, n, 164.0),
                }
            }
        }
    }
}

fn today(c: &Canvas, x: &Ctx, t: &Cl, sh: Shape) {
    let n = &t.n;
    let k = t.k;
    let evs = events(x).unwrap_or_default();
    let today_evs: Vec<&Ev> = evs.iter().filter(|e| e.day.eq_ignore_ascii_case("today")).collect();
    let next = today_evs.first().copied();
    let weekday = t.day.to_uppercase();
    match sh {
        Shape::Mini => {
            mini_tile(c, n, k);
            if next.is_some() {
                // Events left today: a blue dot at the tile's corner.
                let s = 44.0 * k;
                let (dx, dy) = ((c.w + s) / 2.0 - 2.0 * k, (c.h - s) / 2.0 + 2.0 * k);
                c.circle(dx, dy, 4.5 * k, c.halo());
                c.circle(dx, dy, 3.0 * k, BLUE);
            }
        }
        Shape::Dock => {
            let m = style::M_DOCK * k;
            let (sx, sw) = t.slot(c);
            let right = if t.wide { sx - 16.0 * k } else { c.w - m };
            c.text_cap(&weekday, m, 12.0 * k, style::MICRO.size * k, style::MICRO.weight, RED, Align::Left, right - m);
            c.text_base(&format!("{} {}", &t.mon[..3], n.day), m, 42.0 * k, style::VALUE.size * k, style::VALUE.weight, c.text1(), Align::Left, c.w * 2.0);
            if t.wide {
                slot_rule(c, t);
                let fs = style::FOOT.size * k;
                match next {
                    Some(e) => {
                        c.text_base(&e.title, sx, 26.0 * k, fs, 600, c.text1(), Align::Left, sw);
                        c.text_base(&e.time, sx, 42.0 * k, fs, style::FOOT.weight, c.text2(), Align::Left, sw);
                    }
                    None => {
                        c.text_cap("No events", sx, (c.h - c.cap_h(fs)) / 2.0, fs, style::FOOT.weight, c.text2(), Align::Left, sw);
                    }
                }
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            c.text_cap(&weekday, m, 14.0, style::MICRO.size, style::MICRO.weight, RED, Align::Left, c.w - 2.0 * m);
            c.text_base(&n.day.to_string(), m, 62.0, style::HERO_XL.size, style::HERO_XL.weight, c.text1(), Align::Left, c.w * 2.0);
            match next {
                Some(e) => {
                    c.rr(m, 88.0 - c.cap_h(style::CAPTION.size) / 2.0 - 6.0, 3.0, 12.0, 1.5, BLUE);
                    c.text_base(&e.title, m + 7.0, 88.0, style::CAPTION.size, 600, c.text1(), Align::Left, c.w - 2.0 * m - 7.0);
                    c.text_base(&e.time, m + 7.0, 104.0, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, c.w * 2.0);
                }
                None => {
                    c.text_base(t.mon, m, 88.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, c.w - 2.0 * m);
                    c.text_base("No more events", m, 104.0, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, c.w - 2.0 * m);
                }
            }
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let mb = 16.0 + c.cap_h(style::MICRO.size);
            c.text_base(&weekday, m, mb, style::MICRO.size, style::MICRO.weight, RED, Align::Left, 120.0);
            if !today_evs.is_empty() {
                let cnt = if today_evs.len() == 1 { "1 event".to_string() } else { format!("{} events", today_evs.len()) };
                c.text_base(&cnt, c.w - m, mb, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Right, 100.0);
            }
            let (s, fs) = fit_value(c, &format!("{} {}", t.mon, n.day), style::HERO, c.w - 2.0 * m);
            c.text_base(&s, m, 56.0, fs, style::HERO.weight, c.text1(), Align::Left, c.w * 2.0);
            match next {
                Some(e) => {
                    let when = format!(" · {}", e.time);
                    let ww = c.measure(&when, style::CAPTION.size, 400, Font::Text).0;
                    c.rr(m, 78.0 - 11.0, 3.0, 13.0, 1.5, BLUE);
                    let tx = m + 8.0;
                    let tw = c.text_base(&e.title, tx, 78.0, style::BODY.size, 600, c.text1(), Align::Left, c.w - m - tx - ww);
                    c.text_base(&when, tx + tw, 78.0, style::CAPTION.size, 400, c.text2(), Align::Left, ww + 4.0);
                }
                None => {
                    c.text_base("No more events today", m, 78.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, c.w - 2.0 * m);
                }
            }
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let rest: Vec<&Ev> = evs.iter().filter(|e| next.map(|nx| !std::ptr::eq(*e, nx)).unwrap_or(true)).collect();
                let base0 = style::DETAIL_TOP + 16.0;
                if evs.is_empty() {
                    // No calendar (or nothing coming): this week, centred in the region.
                    medium_week(c, n, 84.0, false);
                } else {
                    let shown = event_rows(c, &rest, base0, 3);
                    if shown < 3 {
                        let base = base0 + shown as f32 * style::ROW_L;
                        let note = format!("{} upcoming \u{00B7} {} today", evs.len(), today_evs.len());
                        c.text_base(&note, m, base, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, c.w - 2.0 * m);
                    }
                }
            }
        }
    }
}

fn date(c: &Canvas, t: &Cl, sh: Shape) {
    let n = &t.n;
    let k = t.k;
    let md = format!("{} {}", &t.mon[..3], n.day);
    match sh {
        Shape::Mini => mini_tile(c, n, k),
        Shape::Dock => {
            let m = style::M_DOCK * k;
            c.text_base(&md, m, 30.0 * k, style::VALUE.size * k, style::VALUE.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base(t.day, m, 44.0 * k, style::FOOT.size * k, style::FOOT.weight, c.text2(), Align::Left, c.w - 2.0 * m);
            if t.wide {
                let (_, sw) = t.slot(c);
                c.text_base(&iso_week(n).to_string(), c.w - m, 30.0 * k, style::VALUE.size * k, style::VALUE.weight, c.text1(), Align::Right, sw);
                c.text_base("week", c.w - m, 44.0 * k, style::FOOT.size * k, style::FOOT.weight, c.text2(), Align::Right, sw);
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            c.text_cap(t.day, m, 14.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, c.w - 2.0 * m);
            c.text_base(&n.day.to_string(), m, 86.0, style::HERO_XL.size, style::HERO_XL.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base(t.mon, m, 106.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, c.w - 2.0 * m);
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            c.text_cap(t.day, m, 16.0, style::BODY.size, style::BODY.weight, c.text2(), Align::Left, c.w - 2.0 * m);
            let (s, fs) = fit_value(c, &format!("{} {}", t.mon, n.day), style::HERO, c.w - 2.0 * m);
            c.text_base(&s, m, 58.0, fs, style::HERO.weight, c.text1(), Align::Left, c.w * 2.0);
            let third = format!("Week {} \u{00B7} Day {} of {}", iso_week(n), n.day_of_year(), n.days_in_year());
            c.text_base(&third, m, 78.0, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, c.w - 2.0 * m);
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                // How far through the year: a neutral meter, then three stats.
                let frac = n.day_of_year() as f32 / n.days_in_year() as f32;
                c.bar(m, 114.0, c.w - 2.0 * m, style::METER_H, frac, c.text2());
                c.text_base(&format!("Day {} of {}", n.day_of_year(), n.days_in_year()), m, 136.0, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, c.w - 2.0 * m);
                c.text_base(&format!("{:.0}%", frac * 100.0), c.w - m, 136.0, style::FOOT.size, 500, c.text2(), Align::Right, 60.0);
                let (wk, q, left) = (iso_week(n).to_string(), format!("Q{}", (n.month - 1) / 3 + 1), days_left(n).to_string());
                stat_footer(c, [(wk.as_str(), "Week"), (q.as_str(), "Quarter"), (left.as_str(), "Days left")], 168.0 - c.cap_h(style::VALUE.size));
            }
        }
    }
}

/// Day Page: on the Launchpad the card itself is the calendar page (full bleed).
fn day_page(c: &Canvas, t: &Cl, sh: Shape) {
    let n = &t.n;
    let k = t.k;
    let page = |band: f32| {
        c.rr(0.0, 0.0, c.w, c.h, style::CARD_R, PAGE);
        rr_top(c, 0.0, 0.0, c.w, band, style::CARD_R, RED);
        if c.light {
            c.rr_stroke(0.0, 0.0, c.w, c.h, style::CARD_R, c.card_edge(), 1.0);
        }
    };
    let wk = format!("Week {}", iso_week(n));
    match sh {
        Shape::Mini => mini_tile(c, n, k),
        Shape::Dock => {
            let m = style::M_DOCK * k;
            let s = style::BADGE_DOCK * k;
            day_tile(c, n, m, m, s, style::R_MIN * k, 11.0 * k, style::MICRO.size * k, 14.0 * k);
            let x0 = m + s + style::SP[4] * k;
            let (sx, sw) = t.slot(c);
            let right = if t.wide { sx - 16.0 * k } else { c.w - m };
            c.text_base(t.day, x0, 30.0 * k, style::VALUE.size * k, style::VALUE.weight, c.text1(), Align::Left, right - x0);
            c.text_base(&wk, x0, 44.0 * k, style::FOOT.size * k, style::FOOT.weight, c.text2(), Align::Left, right - x0);
            if t.wide {
                slot_rule(c, t);
                // The week so far: past days, today, days to come.
                let (d, g) = (6.0 * k, 4.0 * k);
                let x1 = c.w - m - (7.0 * d + 6.0 * g);
                for i in 0..7u32 {
                    let col = match i.cmp(&n.weekday) {
                        std::cmp::Ordering::Less => c.text3(),
                        std::cmp::Ordering::Equal => RED,
                        std::cmp::Ordering::Greater => c.track(),
                    };
                    c.circle(x1 + d / 2.0 + i as f32 * (d + g), 26.0 * k, d / 2.0, col);
                }
                c.text_base(&format!("Day {}", n.day_of_year()), c.w - m, 44.0 * k, style::FOOT.size * k, style::FOOT.weight, c.text2(), Align::Right, sw);
            }
        }
        Shape::Small => {
            page(30.0);
            c.text_base(&t.day.to_uppercase(), c.w / 2.0, 20.0, style::FOOT.size, 700, WHITE, Align::Center, c.w - 2.0 * style::M_SMALL);
            c.text_base(&n.day.to_string(), c.w / 2.0, 86.0, 52.0, 600, PAGE_INK, Align::Center, c.w);
            c.text_base(t.mon, c.w / 2.0, 106.0, style::CAPTION.size, style::CAPTION.weight, PAGE_INK2, Align::Center, c.w - 2.0 * style::M_SMALL);
        }
        Shape::Medium => {
            let m = style::M_CARD;
            page(26.0);
            let yr = n.year.to_string();
            let yw = c.measure(&yr, style::FOOT.size, 600, Font::Text).0;
            c.text_base(&yr, c.w - m, 18.0, style::FOOT.size, 600, alpha(WHITE, 0.8), Align::Right, yw + 4.0);
            c.text_base(&format!("{}, {}", t.day, t.mon).to_uppercase(), m, 18.0, style::FOOT.size, 700, WHITE, Align::Left, c.w - 2.0 * m - yw - style::SP[3]);
            c.text_base(&n.day.to_string(), m, 82.0, style::HERO_XL.size, 600, PAGE_INK, Align::Left, c.w);
            c.text_base(&wk, c.w - m, 62.0, style::TITLE.size, style::TITLE.weight, PAGE_INK, Align::Right, 120.0);
            c.text_base(&format!("Day {} of {}", n.day_of_year(), n.days_in_year()), c.w - m, 80.0, style::FOOT.size, style::FOOT.weight, PAGE_INK2, Align::Right, 120.0);
        }
        Shape::Large => {
            page(56.0);
            let w = c.w - 2.0 * style::M_CARD;
            c.text_base(&t.mon.to_uppercase(), c.w / 2.0, 36.0, style::VALUE.size, 700, WHITE, Align::Center, w);
            c.text_base(&n.day.to_string(), c.w / 2.0, 150.0, 96.0, 600, PAGE_INK, Align::Center, c.w);
            c.text_base(t.day, c.w / 2.0, 176.0, style::VALUE.size, style::VALUE.weight, PAGE_INK, Align::Center, w);
            c.text_base(&format!("{wk} \u{00B7} Day {}", n.day_of_year()), c.w / 2.0, 192.0, style::FOOT.size, style::FOOT.weight, PAGE_INK2, Align::Center, w);
        }
    }
}

fn analog(c: &Canvas, t: &Cl, sh: Shape) {
    let n = &t.n;
    let k = t.k;
    let s = Some(n.second);
    let dark = !c.light;
    match sh {
        Shape::Mini => dial(c, c.w / 2.0, c.h / 2.0, 20.0 * k, n.hour, n.minute, s, dark, None, k),
        Shape::Dock => {
            let r = 20.0 * k;
            dial(c, style::M_MINI * k + r, c.h / 2.0, r, n.hour, n.minute, s, dark, None, k);
            let x0 = 56.0 * k;
            dock_time(c, t, x0, &t.date_short(), day_slot_left(c, t));
            if t.wide {
                day_slot(c, t);
            }
        }
        Shape::Small => dial(c, c.w / 2.0, c.h / 2.0, 46.0, n.hour, n.minute, s, dark, None, 1.0),
        Shape::Medium => {
            let m = style::M_CARD;
            dial(c, m + 32.0, m + 32.0, 32.0, n.hour, n.minute, s, dark, None, 1.0);
            side_time(c, t, 96.0, 20.0, 72.0);
        }
        Shape::Large => {
            let m = style::M_CARD;
            dial(c, m + 86.0, m + 86.0, 86.0, n.hour, n.minute, s, dark, None, 1.0);
            date_column(c, t, 200.0, 20.0, 32.0, 188.0);
        }
    }
}

/// Medium analog faces: the digital time and full date beside the dial, from `x0`.
fn side_time(c: &Canvas, t: &Cl, x0: f32, cap_top: f32, cap_base: f32) {
    let hs = style::HERO.size;
    time_ap(c, &t.time, t.ap, x0, cap_top + c.cap_h(hs), hs, style::time_weight(hs), c.text1(), 1.0);
    let w = c.w - style::M_CARD - x0;
    let cap = fit_label(c, &[t.date_long(), format!("{}, {} {}", &t.day[..3], &t.mon[..3], t.n.day)], style::CAPTION.size, 400, w);
    c.text_base(&cap, x0, cap_base, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, w);
}

/// Large analog faces: weekday, day and month at the top of a narrow right column, the
/// digital time on the dial's bottom line.
fn date_column(c: &Canvas, t: &Cl, x0: f32, top: f32, ds: f32, bottom: f32) {
    let w = c.w - style::M_CARD - x0;
    c.text_cap(&t.day[..3].to_uppercase(), x0, top, style::MICRO.size, style::MICRO.weight, RED, Align::Left, w);
    let dtop = top + c.cap_h(style::MICRO.size) + style::SP[3];
    let dbase = dtop + c.cap_h(ds);
    c.text_base(&t.n.day.to_string(), x0, dbase, ds, 400, c.text1(), Align::Left, c.w);
    c.text_base(&t.mon[..3], x0, dbase + style::SP[6], style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, w);
    let tm = format!("{} {}", t.time, t.ap);
    let fs = fit(c, &tm, style::FOOT.size, style::MICRO.size, 500, Font::Text, w);
    c.text_base(&tm, x0, bottom, fs, 500, c.text2(), Align::Left, c.w);
}

fn analog_square(c: &Canvas, t: &Cl, sh: Shape) {
    let n = &t.n;
    let k = t.k;
    let (h, m, s) = (n.hour, n.minute, n.second);
    if !matches!(sh, Shape::Mini | Shape::Dock) {
        // The Launchpad dispatch leaves this face's card to it.
        bg(c, false);
    }
    match sh {
        Shape::Mini => {
            let p = c.h - 12.0 * k;
            square_dial(c, (c.w - p) / 2.0, 6.0 * k, p, p, style::concentric(style::DOCK_R, 6.0) * k, h, m, s);
        }
        Shape::Dock => {
            let p = c.h - 12.0 * k;
            square_dial(c, 6.0 * k, 6.0 * k, p, p, style::concentric(style::DOCK_R, 6.0) * k, h, m, s);
            dock_time(c, t, 58.0 * k, &t.date_short(), day_slot_left(c, t));
            if t.wide {
                day_slot(c, t);
            }
        }
        Shape::Small => {
            let i = style::SP[2];
            square_dial(c, i, i, c.w - 2.0 * i, c.h - 2.0 * i, style::concentric(style::CARD_R, i), h, m, s);
        }
        Shape::Medium => {
            let i = style::SP[3];
            square_dial(c, i, i, 80.0, 80.0, style::concentric(style::CARD_R, i), h, m, s);
            side_time(c, t, 104.0, 20.0, 72.0);
        }
        Shape::Large => {
            let i = style::SP[3];
            let p = c.h - 2.0 * i;
            square_dial(c, i, i, p, p, style::concentric(style::CARD_R, i), h, m, s);
            date_column(c, t, i + p + style::SP[5], 18.0, 26.0, 188.0);
        }
    }
}

fn segment(c: &Canvas, t: &Cl, sh: Shape) {
    let n = &t.n;
    let k = t.k;
    let on = RED;
    let off = c.ink(0.06);
    // The colon blinks between full and dim, never off, so a still glance reads as a time.
    let colon = if n.second % 2 == 0 { on } else { alpha(on, 0.35) };
    let hr = h12(n.hour);
    let d = [hr / 10, hr % 10, n.minute / 10, n.minute % 10];
    let lead = if hr >= 10 { LcdLead::Digit } else { LcdLead::Ghost };
    let md = format!("{} {}", &t.day[..3], n.day);
    match sh {
        Shape::Mini => {
            // No ghost here: "5:38" alone reads at a glance; "85:38" didn't.
            let (lead, nd) = if hr >= 10 { (LcdLead::Digit, 4) } else { (LcdLead::None, 3) };
            let room = c.w - 2.0 * style::M_MINI * k;
            let dh = room.min(room / seg_w(1.0, nd));
            seg_time(c, (c.w - seg_w(dh, nd)) / 2.0, (c.h - dh) / 2.0, dh, d, lead, on, off, colon);
        }
        Shape::Dock => {
            let dh = 30.0 * k;
            let w = seg_w(dh, 4);
            let x0 = if t.wide { style::M_DOCK * k } else { (c.w - w) / 2.0 };
            seg_time(c, x0, (c.h - dh) / 2.0, dh, d, lead, on, off, colon);
            if t.wide {
                let r = c.w - style::M_DOCK * k;
                c.text_base(t.ap, r, 24.0 * k, style::MICRO.size * k, style::MICRO.weight, RED, Align::Right, 60.0 * k);
                c.text_base(&md, r, 40.0 * k, style::FOOT.size * k, 600, c.text2(), Align::Right, 60.0 * k);
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            c.text_cap(t.ap, m, 16.0, style::MICRO.size, style::MICRO.weight, RED, Align::Left, 40.0);
            c.text_cap(&md, c.w - m, 16.0, style::MICRO.size, style::MICRO.weight, c.text2(), Align::Right, 80.0);
            let room = c.w - 2.0 * m;
            let dh = room / seg_w(1.0, 4);
            seg_time(c, m, 34.0 + (72.0 - dh) / 2.0, dh, d, lead, on, off, colon);
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let base = 16.0 + c.cap_h(style::MICRO.size);
            c.text_base(t.ap, m, base, style::MICRO.size, style::MICRO.weight, RED, Align::Left, 40.0);
            c.text_base(&t.date_short(), c.w - m, base, style::FOOT.size, 500, c.text2(), Align::Right, 140.0);
            let dh = 44.0;
            seg_time(c, (c.w - seg_w(dh, 4)) / 2.0, 36.0, dh, d, lead, on, off, colon);
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let w = c.w - 2.0 * m;
                // An LCD weekday strip: today lit.
                let colw = w / 7.0;
                for i in 0..7u32 {
                    let today = i == n.weekday;
                    let lbl = DAYS[i as usize][..3].to_uppercase();
                    c.text_cap(&lbl, m + colw * (i as f32 + 0.5), 112.0, style::MICRO.size, style::MICRO.weight, if today { RED } else { c.ink(0.22) }, Align::Center, colw);
                }
                // The seconds: 60 segment ticks, lit up to now.
                let pitch = (w - 2.0) / 59.0;
                for i in 0..60u32 {
                    c.rr(m + i as f32 * pitch, 140.0, 2.0, 12.0, 0.5, if i <= n.second { on } else { c.ink(0.10) });
                }
                for s in [0u32, 15, 30, 45] {
                    let tx = m + s as f32 * pitch + 1.0;
                    let (ax, al) = if s == 0 { (m, Align::Left) } else { (tx, Align::Center) };
                    c.text_base(&s.to_string(), ax, 172.0, style::MICRO.size, style::MICRO.weight, c.text3(), al, 30.0);
                }
            }
        }
    }
}

fn month(c: &Canvas, t: &Cl, sh: Shape) {
    let n = &t.n;
    match sh {
        Shape::Mini => mini_tile(c, n, t.k),
        Shape::Dock => dock_week(c, t),
        Shape::Small => {
            let m = style::M_SMALL;
            c.text_cap(&t.mon.to_uppercase(), m, 14.0, style::MICRO.size, style::MICRO.weight, c.text1(), Align::Left, c.w - 2.0 * m);
            let rows = month_rows(n);
            let pitch = ((106.0 - 52.0) / (rows as f32 - 1.0)).min(14.0);
            month_grid(c, n, m, c.w - 2.0 * m, 38.0, 52.0, pitch, style::MICRO.size, 500, 14.0);
        }
        Shape::Medium => {
            cal_header(c, t.mon, &format!("{} days left", n.days_in_month() - n.day));
            medium_week(c, n, 0.0, true);
        }
        Shape::Large => {
            let m = style::M_CARD;
            cal_header(c, t.mon, &n.year.to_string());
            let rows = month_rows(n);
            let pitch = ((184.0 - 76.0) / (rows as f32 - 1.0)).min(26.0);
            month_grid(c, n, m, c.w - 2.0 * m, 52.0, 76.0, pitch, style::BODY.size, 500, 26.0);
        }
    }
}

// ------------------------------------------------------------------ weather

pub fn weather_emoji(code: i32, day: bool) -> &'static str {
    match code {
        0 => {
            if day {
                "☀️"
            } else {
                "🌙"
            }
        }
        1 => {
            if day {
                "🌤️"
            } else {
                "🌙"
            }
        }
        // Partly cloudy: the sun behind a cloud by day, just the cloud at night.
        2 => {
            if day {
                "⛅"
            } else {
                "☁️"
            }
        }
        3 => "☁️",
        45 | 48 => "🌫️",
        51..=57 | 80..=82 => "🌦️",
        61..=67 => "🌧️",
        71..=77 | 85 | 86 => "🌨️",
        95..=99 => "⛈️",
        _ => "🌡️",
    }
}

/// Rounded temperature without a "-0".
fn d0(t: f32) -> f32 {
    let r = t.round();
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

/// "17°".
fn deg(t: f32) -> String {
    format!("{:.0}°", d0(t))
}

fn mins(hm: &str) -> f32 {
    let (h, m) = hm.split_once(':').unwrap_or(("0", "0"));
    h.parse::<f32>().unwrap_or(0.0) * 60.0 + m.parse::<f32>().unwrap_or(0.0)
}

fn weekday_of(ymd: &str) -> String {
    // Sakamoto's algorithm.
    let p: Vec<i32> = ymd.split('-').filter_map(|s| s.parse().ok()).collect();
    if p.len() != 3 {
        return String::new();
    }
    let (mut y, m, d) = (p[0], p[1], p[2]);
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    if m < 3 {
        y -= 1;
    }
    let wd = (y + y / 4 - y / 100 + y / 400 + T[(m - 1) as usize] + d).rem_euclid(7);
    DAYS[wd as usize][..3].to_string()
}

/// "4:58" and "PM" for minutes after midnight: 12-hour, no leading zero.
fn clock12(m: f32) -> (String, &'static str) {
    let m = (m.round() as i64).rem_euclid(1440) as u32;
    (format!("{}:{:02}", h12(m / 60), m % 60), ampm(m / 60))
}

/// "2PM" for an hour of the day.
fn hour12(h: u32) -> String {
    format!("{}{}", h12(h), ampm(h))
}

/// A time with its AM/PM in Micro, top-aligned to the digits' cap top, 3 px after them
/// (§2.2). `x` is the left, centre or right of the whole. Returns its width.
#[allow(clippy::too_many_arguments)]
fn time_ampm(c: &Canvas, t: &str, ap: &str, x: f32, base: f32, size: f32, weight: u32, col: Col, ap_col: Col, align: Align) -> f32 {
    let mic = style::MICRO;
    let tw = c.measure(t, size, weight, Font::Text).0;
    let (gap, aw) = if ap.is_empty() { (0.0, 0.0) } else { (3.0, c.measure(ap, mic.size, mic.weight, Font::Text).0) };
    let total = tw + gap + aw;
    let left = match align {
        Align::Left => x,
        Align::Center => x - total / 2.0,
        Align::Right => x - total,
    };
    c.text_base(t, left, base, size, weight, col, Align::Left, tw + 4.0);
    if !ap.is_empty() {
        c.text_cap(ap, left + tw + gap, base - c.cap_h(size), mic.size, mic.weight, ap_col, Align::Left, aw + 4.0);
    }
    total
}

/// Weather emoji measured to the ink (Segoe UI Emoji): at font size `s` the picture fills an
/// `s` square that starts this far (× s) below and right of the text box's top-left.
const EMOJI_TOP: f32 = 0.24;
const EMOJI_LEFT: f32 = 0.18;

/// A weather emoji `size` px square, centred on `cx`, with its top at `top`.
fn draw_emoji(c: &Canvas, e: &str, cx: f32, top: f32, size: f32, col: Col) {
    let (x, y) = (cx - size * (EMOJI_LEFT + 0.5), top - size * EMOJI_TOP);
    if c.light {
        emoji_shadow(c, e, x, y, size);
    }
    c.text(e, x, y, size, 400, col, Align::Left, size * 2.0, Font::Text);
}

/// Light cards: white emoji (clouds, snow) have almost no edge on pale glass. Under the
/// canvas's own crisp silhouette, a soft one (about 1 px down, 2 px blur, black ≈ 0.2 at its
/// core) gives them a shape. The blur is a ring of faint copies of the glyph's silhouette.
fn emoji_shadow(c: &Canvas, e: &str, x: f32, y: f32, size: f32) {
    use windows::core::HSTRING;
    use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
    use windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_NONE;
    use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_WORD_WRAPPING_NO_WRAP};
    let wide: Vec<u16> = e.encode_utf16().collect();
    unsafe {
        let Ok(f) = c.dw.CreateTextFormat(&HSTRING::from(Canvas::family(size, Font::Text)), None, DWRITE_FONT_WEIGHT(400), DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, &HSTRING::from("en-us")) else { return };
        let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        let Ok(l) = c.dw.CreateTextLayout(&wide, &f, size * 2.0, size * 2.0) else { return };
        let Ok(b) = c.dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.045 }, None) else { return };
        // Eight taps on a 1 px circle around (0, 1) plus the centre: overlapping, they build
        // to ≈ 0.2 under the glyph and fade out over the last 2 px.
        let taps = [(0.0, 1.0), (-1.0, 1.0), (1.0, 1.0), (0.0, 0.0), (0.0, 2.0), (-0.7, 0.3), (0.7, 0.3), (-0.7, 1.7), (0.7, 1.7)];
        for (dx, dy) in taps {
            c.dc.DrawTextLayout(windows_numerics::Vector2 { X: x + dx, Y: y + dy }, &l, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }
}

/// A temperature centred on its digits' ink, so the ° hangs off to the right.
fn temp_mid(c: &Canvas, s: &str, cx: f32, base: f32, size: f32, weight: u32, col: Col) {
    let dw = c.measure(s.trim_end_matches('°'), size, weight, Font::Text).0;
    c.text_base(s, cx - dw / 2.0, base, size, weight, col, Align::Left, c.w * 2.0);
}

/// The temperature colour scale for range bars: ≤0° blue, 10° teal, 20° yellow, 30° orange,
/// ≥35° red (light theme: the accents' light fills).
fn temp_col(c: &Canvas, t: f32) -> Col {
    let scale = [(0.0, BLUE), (10.0, hex(0x40C8E0)), (20.0, YELLOW), (30.0, ORANGE), (35.0, RED)];
    let k = |i: usize| c.tone(scale[i].1, false);
    if t <= scale[0].0 {
        return k(0);
    }
    for i in 1..scale.len() {
        if t <= scale[i].0 {
            let f = (t - scale[i - 1].0) / (scale[i].0 - scale[i - 1].0);
            let (a, b) = (k(i - 1), k(i));
            return (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f, a.2 + (b.2 - a.2) * f, 1.0);
        }
    }
    k(scale.len() - 1)
}

/// A fully round bar filled with a horizontal gradient whose stops (0..1) run from `gx0`
/// to `gx1`, so bars sharing a scale share colours at the same x.
#[allow(clippy::too_many_arguments)]
fn grad_bar(c: &Canvas, x: f32, y: f32, w: f32, h: f32, gx0: f32, gx1: f32, stops: &[(f32, Col)]) {
    use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D1_GRADIENT_STOP, D2D_RECT_F};
    use windows::Win32::Graphics::Direct2D::{ID2D1RenderTarget, D2D1_EXTEND_MODE_CLAMP, D2D1_GAMMA_2_2, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_ROUNDED_RECT};
    use windows_numerics::Vector2;
    if w <= 0.0 || h <= 0.0 || stops.is_empty() {
        return;
    }
    let st: Vec<D2D1_GRADIENT_STOP> = stops
        .iter()
        .map(|(p, col)| {
            let k = c.tone(*col, false);
            D2D1_GRADIENT_STOP { position: p.clamp(0.0, 1.0), color: D2D1_COLOR_F { r: k.0, g: k.1, b: k.2, a: k.3 } }
        })
        .collect();
    unsafe {
        let rt: &ID2D1RenderTarget = c.dc;
        let Ok(coll) = rt.CreateGradientStopCollection(&st, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) else { return };
        let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: gx0, Y: 0.0 }, endPoint: Vector2 { X: gx1.max(gx0 + 1.0), Y: 0.0 } };
        let Ok(b) = c.dc.CreateLinearGradientBrush(&props, None, &coll) else { return };
        let r = h / 2.0;
        c.dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }, radiusX: r, radiusY: r }, &b);
    }
}

/// A temperature range bar: track from `x` to `x + w` for `lo_t`..`hi_t`, the `a`..`b` part
/// filled with the temperature scale, and an optional current-temperature dot.
#[allow(clippy::too_many_arguments)]
fn range_bar(c: &Canvas, x: f32, cy: f32, w: f32, (lo_t, hi_t): (f32, f32), (a, b): (f32, f32), now: Option<f32>, track: Col, dot: Col, ring: Col) {
    let h = style::METER_H_COMPACT;
    let span = (hi_t - lo_t).max(1.0);
    let at = |t: f32| x + w * ((t - lo_t) / span).clamp(0.0, 1.0);
    c.rr(x, cy - h / 2.0, w, h, h / 2.0, track);
    let mut stops = vec![(0.0, temp_col(c, lo_t))];
    for t in [0.0, 10.0, 20.0, 30.0, 35.0] {
        if t > lo_t && t < hi_t {
            stops.push(((t - lo_t) / span, temp_col(c, t)));
        }
    }
    stops.push((1.0, temp_col(c, hi_t)));
    let (xa, xb) = (at(a), at(b));
    let (x0, x1) = if xb - xa < h { ((xa + xb - h) / 2.0, (xa + xb + h) / 2.0) } else { (xa, xb) };
    grad_bar(c, x0, cy - h / 2.0, x1 - x0, h, x, x + w, &stops);
    if let Some(t) = now {
        let px = at(t).clamp(x0 + 1.0, x1 - 1.0);
        c.circle(px, cy, 3.0 + 1.5, ring);
        c.circle(px, cy, 3.0, dot);
    }
}

/// Clear daytime sky (also the banner's setup and loading card).
const SKY_CLEAR: (u32, u32) = (0x3F86D8, 0x2A66B6);

/// Sky colours for the banner, by condition (two-stop vertical gradient). Every top stop
/// keeps white text at 3.7:1 or better: snow is a pale wintry blue, overcast a neutral grey,
/// rain a darker slate, night a deep navy.
fn sky(w: &laminar_sys::weather::Weather) -> (Col, Col) {
    let (top, bottom) = if !w.is_day {
        (0x1B2550, 0x0E1430)
    } else {
        match w.code {
            71..=77 | 85 | 86 => (0x6F86A6, 0x4F6585),
            51..=67 | 80..=82 | 95..=99 => (0x5B6B80, 0x3A4657),
            3 | 45 | 48 => (0x6C7A8B, 0x4B5767),
            _ => SKY_CLEAR,
        }
    };
    (hex(top), hex(bottom))
}

/// Secondary text on the banner's sky.
const SKY_T2: f32 = 0.9;

/// The banner's card: the sky gradient with a faint white inner edge.
fn sky_card(c: &Canvas, (top, bottom): (Col, Col), sh: Shape) {
    let r = if matches!(sh, Shape::Mini | Shape::Dock) { style::DOCK_R } else { style::CARD_R };
    c.rr_grad(0.0, 0.0, c.w, c.h, r, top, bottom);
    c.rr_stroke(0.0, 0.0, c.w, c.h, r, (1.0, 1.0, 1.0, 0.12), 1.0);
}

/// One forecast day.
struct WDay {
    label: String,
    emoji: &'static str,
    hi: f32,
    lo: f32,
}

/// Colours a weather card draws with: the theme's, or white on the banner's sky.
struct Inks {
    t1: Col,
    t2: Col,
    track: Col,
    sep: Col,
    /// Behind the current-temperature dot (the card colour).
    ring: Col,
}

fn weather(c: &Canvas, out: &mut Painted, k: WeatherKind, x: &Ctx, sh: Shape) {
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open(format!("https://www.msn.com/en-ca/weather/forecast/in-{}", x.cfg.weather_city.replace(' ', "-")))));
    let Some(w) = x.weather else {
        weather_waiting(c, out, k, x, sh);
        return;
    };
    if k == WeatherKind::Sun {
        sun(c, w, x, sh);
        return;
    }
    let city = x.cfg.weather_city.trim();
    let temp = deg(w.temp_c);
    let emoji = weather_emoji(w.code, w.is_day);
    let hl = format!("H:{} L:{}", deg(w.high_c), deg(w.low_c));
    // Now, then the next full hours (moons after sunset).
    let (rise, set) = if w.sunrise.is_empty() || w.sunset.is_empty() { (360.0, 1140.0) } else { (mins(&w.sunrise), mins(&w.sunset)) };
    let day_at = |h: u8| (rise..set).contains(&(h as f32 * 60.0));
    let hours: Vec<(String, &'static str, String)> = std::iter::once(("Now".to_string(), emoji, temp.clone())).chain(w.hourly.iter().map(|(h, t, code)| (hour12(*h as u32), weather_emoji(*code, day_at(*h)), deg(*t)))).collect();
    let days: Vec<WDay> = w.daily.iter().enumerate().map(|(i, (d, hi, lo, code))| WDay { label: if i == 0 { "Today".to_string() } else { weekday_of(d) }, emoji: weather_emoji(*code, true), hi: *hi, lo: *lo }).collect();
    let banner = k == WeatherKind::Banner;
    let ink = if banner {
        let s = sky(w);
        sky_card(c, s, sh);
        Inks { t1: WHITE, t2: alpha(WHITE, SKY_T2), track: alpha(WHITE, 0.22), sep: alpha(WHITE, 0.2), ring: s.1 }
    } else {
        Inks { t1: c.text1(), t2: c.text2(), track: c.track(), sep: c.sep(), ring: c.halo() }
    };
    let feels = format!("Feels like {}", deg(w.feels_c));
    let humid = format!("{:.0}%", w.humidity);
    match sh {
        Shape::Mini => {
            let k = c.h / 54.0;
            draw_emoji(c, emoji, c.w / 2.0, 8.0 * k, style::BADGE_MINI * k, ink.t1);
            temp_mid(c, &temp, c.w / 2.0, 45.0 * k, style::MINI_VALUE.size * k, style::MINI_VALUE.weight, ink.t1);
        }
        Shape::Dock => {
            let kk = c.h / 54.0;
            let m = style::M_DOCK * kk;
            let bs = style::BADGE_DOCK * kk;
            let es = bs - 2.0 * kk;
            draw_emoji(c, emoji, m + bs / 2.0, (c.h - es) / 2.0, es, ink.t1);
            let x0 = m + bs + style::SP[4] * kk;
            let wide = c.w >= c.h * 3.4;
            let slot_w = (c.w * 0.34).round();
            let slot_x = c.w - m - slot_w;
            let right = if wide { slot_x - style::SP[4] * kk } else { c.w - m };
            c.text_base(&temp, x0, 30.0 * kk, style::VALUE.size * kk, style::VALUE.weight, ink.t1, Align::Left, c.w);
            let caption = match k {
                WeatherKind::Detail => feels.clone(),
                WeatherKind::Hourly => hours.get(1).map(|(l, _, t)| format!("{t} at {l}")).unwrap_or_else(|| w.description.clone()),
                WeatherKind::Week => days.get(1).map(|d| format!("Tomorrow {}", deg(d.hi))).unwrap_or_else(|| w.description.clone()),
                _ => w.description.clone(),
            };
            let fs = style::FOOT.size * kk;
            c.text_base(&caption, x0, 44.0 * kk, fs, 400, ink.t2, Align::Left, right - x0);
            if !wide {
                return;
            }
            let r = c.w - m;
            match k {
                WeatherKind::Detail => {
                    c.text_base(&humid, r, 28.0 * kk, style::TITLE.size * kk, 600, ink.t1, Align::Right, slot_w);
                    c.text_base("Humidity", r, 41.0 * kk, fs, 400, ink.t2, Align::Right, slot_w);
                }
                WeatherKind::Hourly | WeatherKind::Week => {
                    let cols: Vec<(String, String)> = if k == WeatherKind::Hourly { hours.iter().skip(1).take(3).map(|(l, _, t)| (l.clone(), t.clone())).collect() } else { days.iter().skip(1).take(3).map(|d| (d.label.clone(), deg(d.hi))).collect() };
                    let cw = slot_w / 3.0;
                    for (i, (l, t)) in cols.iter().enumerate() {
                        let cx = slot_x + cw * (i as f32 + 0.5);
                        c.text_base(l, cx, 24.0 * kk, style::MICRO.size * kk, style::MICRO.weight, ink.t2, Align::Center, cw + 4.0);
                        temp_mid(c, t, cx, 38.0 * kk, fs, 600, ink.t1);
                    }
                }
                _ => {
                    c.text_base(&format!("H {}", deg(w.high_c)), r, 26.0 * kk, fs, 500, ink.t1, Align::Right, slot_w);
                    c.text_base(&format!("L {}", deg(w.low_c)), r, 40.0 * kk, fs, 400, ink.t2, Align::Right, slot_w);
                }
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let r = c.w - m;
            let inner = r - m;
            let cap = style::CAPTION;
            match k {
                WeatherKind::Hourly | WeatherKind::Week => {
                    let mut cw = inner;
                    if k == WeatherKind::Week {
                        let base = m + c.cap_h(style::TITLE.size);
                        cw -= c.text_base(&temp, r, base, style::FOOT.size, 400, ink.t2, Align::Right, inner) + style::SP[3];
                    }
                    c.text_cap(city, m, m, style::TITLE.size, style::TITLE.weight, ink.t1, Align::Left, cw);
                    let colw = inner / 3.0;
                    for i in 0..3 {
                        let cx = m + colw * (i as f32 + 0.5);
                        if k == WeatherKind::Hourly {
                            let Some((l, e, t)) = hours.get(i) else { break };
                            c.text_base(l, cx, 60.0, style::MICRO.size, style::MICRO.weight, if i == 0 { ink.t1 } else { ink.t2 }, Align::Center, colw + 4.0);
                            draw_emoji(c, e, cx, 66.0, 20.0, ink.t1);
                            temp_mid(c, t, cx, 106.0, style::TITLE.size, 600, ink.t1);
                        } else {
                            let Some(d) = days.get(i) else { break };
                            c.text_base(&d.label, cx, 52.0, style::MICRO.size, style::MICRO.weight, ink.t2, Align::Center, colw + 4.0);
                            draw_emoji(c, d.emoji, cx, 58.0, 20.0, ink.t1);
                            temp_mid(c, &deg(d.hi), cx, 92.0, style::TITLE.size, 600, ink.t1);
                            temp_mid(c, &deg(d.lo), cx, 106.0, style::FOOT.size, 400, ink.t2);
                        }
                    }
                }
                _ => {
                    let es = 24.0;
                    draw_emoji(c, emoji, r - es / 2.0, 12.0, es, ink.t1);
                    c.text_cap(city, m, m, style::TITLE.size, style::TITLE.weight, ink.t1, Align::Left, inner - es - style::SP[3]);
                    c.text_base(&temp, m, 66.0, style::HERO.size, style::time_weight(style::HERO.size), ink.t1, Align::Left, c.w * 2.0);
                    let (l1, l2) = match k {
                        WeatherKind::Detail => (feels.clone(), format!("Humidity {humid}")),
                        _ => (w.description.clone(), hl.clone()),
                    };
                    c.text_base(&l1, m, 90.0, cap.size, if banner { 600 } else { cap.weight }, if banner { WHITE } else { ink.t2 }, Align::Left, inner);
                    c.text_base(&l2, m, 106.0, cap.size, cap.weight, ink.t2, Align::Left, inner);
                }
            }
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let r = c.w - m;
            let inner = r - m;
            let large = sh == Shape::Large;
            let hero_w = style::time_weight(style::HERO.size);
            let cap = style::CAPTION;
            // The week's range, for the range bars (and today's in the Now card).
            let shown = if k == WeatherKind::Week { 7 } else { 4 };
            let range = days.iter().take(shown).fold((w.temp_c, w.temp_c), |a, d| (a.0.min(d.lo), a.1.max(d.hi)));
            match k {
                WeatherKind::Now => {
                    let es = 40.0;
                    draw_emoji(c, emoji, r - es / 2.0, 14.0, es, ink.t1);
                    c.text_base(city, m, 28.0, style::TITLE.size, style::TITLE.weight, ink.t1, Align::Left, inner - es - style::SP[3]);
                    c.text_base(&temp, m, 61.0, style::HERO.size, hero_w, ink.t1, Align::Left, c.w * 2.0);
                    c.text_base(&w.description, m, 80.0, cap.size, cap.weight, ink.t2, Align::Left, 148.0 - m - style::SP[3]);
                    // Today's range in the accessory box: low, bar with now, high.
                    let fs = style::FOOT.size;
                    let base = 78.0;
                    let cy = base - c.cap_h(fs) / 2.0;
                    c.text_base(&deg(w.low_c), 168.0, base, fs, 400, ink.t2, Align::Right, 40.0);
                    range_bar(c, 174.0, cy, 52.0, range, (w.low_c, w.high_c), Some(w.temp_c), ink.track, ink.t1, ink.ring);
                    c.text_base(&deg(w.high_c), 230.0, base, fs, 600, ink.t1, Align::Left, r - 230.0 + 8.0);
                }
                WeatherKind::Detail => {
                    let col_r = 124.0;
                    c.text_base(city, m, 28.0, style::TITLE.size, style::TITLE.weight, ink.t1, Align::Left, col_r - m);
                    let tw = c.text_base(&temp, m, 61.0, style::HERO.size, hero_w, ink.t1, Align::Left, c.w * 2.0);
                    let es = style::BADGE_GLANCE;
                    draw_emoji(c, emoji, m + tw + style::SP[3] + es / 2.0, 61.0 - c.cap_h(style::HERO.size) / 2.0 - es / 2.0, es, ink.t1);
                    c.text_base(&w.description, m, 80.0, cap.size, cap.weight, ink.t2, Align::Left, col_r - m);
                    c.line(130.5, 16.0, 130.5, 80.0, 1.0, ink.sep);
                    let rows = [("Feels like", deg(w.feels_c)), ("Humidity", humid.clone()), ("Wind", format!("{:.0} km/h", w.wind_kmh)), ("UV", format!("{:.0}", w.uv))];
                    for (i, (l, v)) in rows.iter().enumerate() {
                        let base = 28.0 + i as f32 * (80.0 - 28.0) / 3.0;
                        let vw = c.text_base(v, r, base, style::TITLE.size, 600, ink.t1, Align::Right, r - 140.0);
                        c.text_base(l, 140.0, base, style::FOOT.size, 400, ink.t2, Align::Left, r - 140.0 - vw - style::SP[2]);
                    }
                }
                WeatherKind::Banner => {
                    let hs = style::HERO_XL.size;
                    let hw = c.measure(&temp, hs, 300, Font::Text).0;
                    c.text_cap(&temp, r, m, hs, 300, WHITE, Align::Right, hw + 8.0);
                    let es = 28.0;
                    draw_emoji(c, emoji, r - es / 2.0, 80.0 - es, es, WHITE);
                    let left_w = inner - hw - style::SP[6];
                    c.text_cap(&w.description, m, m, style::VALUE.size, style::VALUE.weight, WHITE, Align::Left, left_w);
                    c.text_base(city, m, 50.0, cap.size, cap.weight, ink.t2, Align::Left, left_w);
                    c.text_base(&hl, m, 80.0, cap.size, cap.weight, ink.t2, Align::Left, left_w);
                }
                WeatherKind::Hourly | WeatherKind::Week => {
                    let meta = if k == WeatherKind::Week { format!("{temp} {}", w.description) } else { w.description.clone() };
                    let mw = c.text_base(&meta, r, style::HEADER_BASE, style::FOOT.size, 400, ink.t2, Align::Right, inner * 0.6);
                    c.text_base(city, m, style::HEADER_BASE, style::TITLE.size, style::TITLE.weight, ink.t1, Align::Left, inner - mw - style::SP[3]);
                    if k == WeatherKind::Hourly || !large {
                        let n = if k == WeatherKind::Hourly { 6 } else { 5 };
                        let colw = inner / n as f32;
                        for i in 0..n {
                            let cx = m + colw * (i as f32 + 0.5);
                            let (l, e, hi, lo) = if k == WeatherKind::Hourly {
                                let Some((l, e, t)) = hours.get(i) else { break };
                                (l.clone(), *e, t.clone(), None)
                            } else {
                                let Some(d) = days.get(i) else { break };
                                (d.label.clone(), d.emoji, deg(d.hi), Some(deg(d.lo)))
                            };
                            let lc = if k == WeatherKind::Hourly && i == 0 { ink.t1 } else { ink.t2 };
                            // Label, emoji and temperature share y 42..80 with even gaps.
                            c.text_base(&l, cx, 49.0, style::MICRO.size, style::MICRO.weight, lc, Align::Center, colw + 4.0);
                            draw_emoji(c, e, cx, 52.5, 16.0, ink.t1);
                            match lo {
                                None => temp_mid(c, &hi, cx, 80.0, style::TITLE.size, 600, ink.t1),
                                Some(lo) => {
                                    let hw = c.measure(&hi, style::TITLE.size, 600, Font::Text).0;
                                    let lw = c.measure(&lo, style::FOOT.size, 400, Font::Text).0;
                                    let x0 = cx - (hw + style::SP[1] + lw) / 2.0;
                                    c.text_base(&hi, x0, 80.0, style::TITLE.size, 600, ink.t1, Align::Left, hw + 4.0);
                                    c.text_base(&lo, x0 + hw + style::SP[1], 80.0, style::FOOT.size, 400, ink.t2, Align::Left, lw + 4.0);
                                }
                            }
                        }
                    }
                }
                WeatherKind::Sun => {}
            }
            if !large {
                return;
            }
            match k {
                WeatherKind::Detail => detail_grid(c, w, &days),
                WeatherKind::Week => {
                    let pitch = (188.0 - style::CONTENT_TOP) / 7.0;
                    day_rows(c, &days[..days.len().min(7)], style::CONTENT_TOP + pitch / 2.0, pitch, range, w.temp_c, &ink);
                }
                _ => {
                    c.line(m, style::SEP_Y_LARGE + 0.5, r, style::SEP_Y_LARGE + 0.5, 1.0, ink.sep);
                    let pitch = style::ROW_M;
                    day_rows(c, &days[..days.len().min(4)], style::DETAIL_TOP + pitch / 2.0, pitch, range, w.temp_c, &ink);
                }
            }
        }
    }
}

/// Forecast rows (Large): day, emoji, low, a temperature-scale range bar, high. The first
/// row (today) marks the current temperature on its bar. `first` is the first row's centre.
fn day_rows(c: &Canvas, days: &[WDay], first: f32, pitch: f32, range: (f32, f32), now: f32, ink: &Inks) {
    let m = style::M_CARD;
    let fs = style::BODY.size;
    for (i, d) in days.iter().enumerate() {
        let cy = first + i as f32 * pitch;
        let base = cy + c.cap_h(fs) / 2.0;
        c.text_base(&d.label, m, base, fs, 600, ink.t1, Align::Left, 56.0);
        draw_emoji(c, d.emoji, 88.0, cy - 8.0, 16.0, ink.t1);
        c.text_base(&deg(d.lo), 132.0, base, fs, 400, ink.t2, Align::Right, 34.0);
        range_bar(c, 142.0, cy, 72.0, range, (d.lo, d.hi), (i == 0).then_some(now), ink.track, ink.t1, ink.ring);
        c.text_base(&deg(d.hi), c.w - m, base, fs, 600, ink.t1, Align::Right, 34.0);
    }
}

/// weather-detail Large: two rows of three stats under the separator.
fn detail_grid(c: &Canvas, w: &laminar_sys::weather::Weather, days: &[WDay]) {
    let m = style::M_CARD;
    separator(c, style::SEP_Y_LARGE);
    let cw = (c.w - 2.0 * m) / 3.0;
    let has_sun = !w.sunrise.is_empty() && !w.sunset.is_empty();
    let (rise, set) = (mins(&w.sunrise), mins(&w.sunset));
    let vs = style::VALUE.size;
    let (t1, t2) = (c.text1(), c.text2());
    let label = |i: usize, base: f32, s: &str| {
        c.text_base(s, m + i as f32 * cw, base + 15.0, style::FOOT.size, 400, t2, Align::Left, cw - style::SP[3]);
    };
    let value = |i: usize, base: f32, s: &str| {
        let (v, fs) = fit_value(c, s, style::VALUE, cw - style::SP[3]);
        c.text_base(&v, m + i as f32 * cw, base, fs, style::VALUE.weight, t1, Align::Left, c.w);
    };
    let (a, b) = (126.0, 168.0);
    value(0, a, &deg(w.high_c));
    label(0, a, "High");
    value(1, a, &deg(w.low_c));
    label(1, a, "Low");
    value(2, a, &if has_sun && set > rise { dur(((set - rise) * 60.0) as u64) } else { "—".into() });
    label(2, a, "Daylight");
    for (i, (hm, l)) in [(rise, "Sunrise"), (set, "Sunset")].iter().enumerate() {
        if has_sun {
            let (t, ap) = clock12(*hm);
            time_ampm(c, &t, ap, m + i as f32 * cw, b, vs, 600, t1, t2, Align::Left);
        } else {
            value(i, b, "—");
        }
        label(i, b, l);
    }
    if let Some(d) = days.get(1) {
        let x0 = m + 2.0 * cw;
        let es = 16.0;
        draw_emoji(c, d.emoji, x0 + es / 2.0, b - c.cap_h(vs) / 2.0 - es / 2.0, es, t1);
        c.text_base(&deg(d.hi), x0 + es + style::SP[1], b, vs, style::VALUE.weight, t1, Align::Left, cw);
        label(2, b, "Tomorrow");
    }
}

/// An original sunrise/sunset mark: a half sun resting on a horizon, a chevron above it
/// (up for sunrise, down for sunset). `size` is the mark's box, `top` its top.
fn sun_mark(c: &Canvas, cx: f32, top: f32, size: f32, rising: bool) {
    let k = size / 20.0;
    let hy = top + size * 0.82;
    let r = if size >= 26.0 { 8.0 * size / 30.0 } else { 6.0 * k };
    c.line(cx - size / 2.0 + 0.75, hy, cx + size / 2.0 - 0.75, hy, 1.5 * k.min(1.2), c.text2());
    // The sun: a disc cut off just above the horizon line.
    let cut = hy - 1.75 * k.min(1.2);
    c.clip(cx - r - 1.0, cut - r - 1.0, 2.0 * r + 2.0, r + 1.0, || c.circle(cx, hy, r, YELLOW));
    let (cw, ch) = (6.0 * k.min(1.33), 3.0 * k.min(1.33));
    let cyy = top + (hy - r - top) / 2.0 - 0.5;
    let (tip, ends) = if rising { (cyy - ch / 2.0, cyy + ch / 2.0) } else { (cyy + ch / 2.0, cyy - ch / 2.0) };
    c.line(cx - cw / 2.0, ends, cx, tip, 1.5, ORANGE);
    c.line(cx, tip, cx + cw / 2.0, ends, 1.5, ORANGE);
}

/// The sun's path: a horizon hairline from `x0` to `x1` at `hy`, a track-coloured arc of
/// radius `r` centred on `cx`, and (by day) the travelled part in orange with the sun on it.
#[allow(clippy::too_many_arguments)]
fn sun_path(c: &Canvas, cx: f32, hy: f32, r: f32, (x0, x1): (f32, f32), frac: Option<f32>, (track_t, done_t): (f32, f32), dot_r: f32, night: Option<f32>) {
    c.line(x0, hy + 0.5, x1, hy + 0.5, 1.0, c.sep());
    c.arc(cx, hy, r, track_t, 180.0, 360.0, c.track());
    if let Some(f) = frac {
        c.arc(cx, hy, r, done_t, 180.0, 180.0 + 180.0 * f, ORANGE);
        let a = (180.0 + 180.0 * f).to_radians();
        let (px, py) = (cx + r * a.cos(), hy + r * a.sin());
        // Cut out of the arc by a ring of the card colour; a fine orange rim on light cards.
        c.circle(px, py, dot_r + 1.5, c.halo());
        c.circle(px, py, dot_r, YELLOW);
        if c.light {
            c.circle_stroke(px, py, dot_r - 0.25, alpha(hex(0xC45500), 0.4), 0.5);
        }
    }
    // At night the sun travels back under the horizon, from sunset (right) to sunrise (left):
    // a small grey dot just below the line says where it is.
    if let Some(f) = night {
        let d = (dot_r * 0.75).max(2.0);
        let px = cx + r * (std::f32::consts::PI * f).cos();
        c.circle(px, hy + 1.0 + d + 1.5, d, c.text2());
    }
}

/// sunrise-sunset: the next sunrise or sunset, and where the sun is on today's path.
fn sun(c: &Canvas, w: &laminar_sys::weather::Weather, x: &Ctx, sh: Shape) {
    let (rise, set) = (mins(&w.sunrise), mins(&w.sunset));
    let now_m = (x.now.hour * 60 + x.now.minute) as f32;
    let up = set > rise && now_m >= rise && now_m < set;
    let frac = up.then(|| ((now_m - rise) / (set - rise)).clamp(0.0, 1.0));
    // How far through the night it is (0 at sunset, 1 at sunrise).
    let night = (!up && set > rise).then(|| {
        let gone = if now_m >= set { now_m - set } else { now_m + 1440.0 - set };
        (gone / (1440.0 - (set - rise)).max(1.0)).clamp(0.0, 1.0)
    });
    let (next_l, next_m, other_l, other_m) = if up { ("Sunset", set, "Sunrise", rise) } else { ("Sunrise", rise, "Sunset", set) };
    let until = if up {
        set - now_m
    } else if now_m < rise {
        rise - now_m
    } else {
        1440.0 - now_m + rise
    };
    let (t, ap) = clock12(next_m);
    let (t1, t2, t3) = (c.text1(), c.text2(), c.text3());
    match sh {
        Shape::Mini => {
            let k = c.h / 54.0;
            sun_mark(c, c.w / 2.0, 8.0 * k, style::BADGE_MINI * k, !up);
            let role = Type { size: style::MINI_VALUE.size * k, ..style::MINI_VALUE };
            let (v, fs) = fit_value(c, &t, role, c.w - 2.0 * style::M_MINI * k);
            c.text_base(&v, c.w / 2.0, 45.0 * k, fs, role.weight, t1, Align::Center, c.w * 2.0);
        }
        Shape::Dock => {
            let k = c.h / 54.0;
            let m = style::M_DOCK * k;
            let bs = style::BADGE_DOCK * k;
            sun_mark(c, m + bs / 2.0, (c.h - bs) / 2.0, bs, !up);
            let x0 = m + bs + style::SP[4] * k;
            let wide = c.w >= c.h * 3.4;
            let slot_w = (c.w * 0.34).round();
            let slot_x = c.w - m - slot_w;
            let right = if wide { slot_x - style::SP[4] * k } else { c.w - m };
            time_ampm(c, &t, ap, x0, 30.0 * k, style::VALUE.size * k, style::VALUE.weight, t1, t2, Align::Left);
            c.text_base(next_l, x0, 44.0 * k, style::FOOT.size * k, 400, t2, Align::Left, right - x0);
            if wide {
                let hy = 38.0 * k;
                let r = 22.0 * k;
                sun_path(c, slot_x + slot_w / 2.0, hy, r, (slot_x, slot_x + slot_w), frac, (1.5, 1.5), 3.0 * k, night);
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            // No night dot here: under the horizon it would land on the hero time. At night
            // the arc is all track, which already says the sun is down.
            sun_path(c, c.w / 2.0, 58.0, 40.0, (m, c.w - m), frac, (2.0, 2.5), 4.0, None);
            let hs = style::HERO_S.size;
            time_ampm(c, &t, ap, m, 86.0, hs, style::time_weight(hs), t1, t2, Align::Left);
            c.text_base(next_l, m, 106.0, style::CAPTION.size, style::CAPTION.weight, t2, Align::Left, c.w - 2.0 * m);
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let hs = style::HERO.size;
            time_ampm(c, &t, ap, m, m + c.cap_h(hs), hs, style::time_weight(hs), t1, t2, Align::Left);
            let left_w = 148.0 - m - style::SP[3];
            c.text_base(&format!("{next_l} · in {}", dur((until * 60.0) as u64)), m, 58.0, style::CAPTION.size, style::CAPTION.weight, t2, Align::Left, left_w);
            let (ot, oap) = clock12(other_m);
            c.text_base(&format!("{other_l} {ot} {oap}"), m, 76.0, style::FOOT.size, 400, t3, Align::Left, left_w);
            sun_path(c, 200.0, 72.0, 48.0, (148.0, c.w - m), frac, (2.0, 2.5), 4.0, night);
            if sh != Shape::Large {
                return;
            }
            separator(c, style::SEP_Y_LARGE);
            let cw = (c.w - 2.0 * m) / 3.0;
            let day = (set - rise).max(0.0);
            let (nt, nap) = clock12((rise + set) / 2.0);
            let vs = style::VALUE.size;
            c.text_base(&dur((day * 60.0) as u64), m, 128.0, vs, 600, t1, Align::Left, cw);
            time_ampm(c, &nt, nap, m + cw, 128.0, vs, 600, t1, t2, Align::Left);
            c.text_base(&dur(((1440.0 - day) * 60.0) as u64), m + 2.0 * cw, 128.0, vs, 600, t1, Align::Left, cw);
            for (i, l) in ["Daylight", "Solar noon", "Night"].iter().enumerate() {
                c.text_base(l, m + i as f32 * cw, 143.0, style::FOOT.size, 400, t2, Align::Left, cw - style::SP[3]);
            }
            // The whole day: daylight span on a 24-hour strip, with a tick for now.
            let (x0, sw) = (m, c.w - 2.0 * m);
            let at = |mm: f32| x0 + sw * (mm / 1440.0).clamp(0.0, 1.0);
            let h = style::METER_H;
            let cy = 160.0;
            c.rr(x0, cy - h / 2.0, sw, h, h / 2.0, c.track());
            if set > rise {
                let (a, b) = (at(rise), at(set));
                grad_bar(c, a, cy - h / 2.0, b - a, h, a, b, &[(0.0, ORANGE), (0.5, YELLOW), (1.0, ORANGE)]);
            }
            let nx = at(now_m);
            c.rr(nx - 1.0, cy - 6.0, 2.0, 12.0, 1.0, t1);
            let mic = style::MICRO;
            for (i, l) in ["12AM", "6AM", "12PM", "6PM"].iter().enumerate() {
                let px = x0 + sw * i as f32 / 4.0;
                let al = if i == 0 { Align::Left } else { Align::Center };
                c.text_base(l, px, 186.0, mic.size, mic.weight, t3, al, 40.0);
            }
        }
    }
}

/// No weather yet: the setup state when no location is set (§7), else a skeleton of the
/// real layout while the first forecast loads.
fn weather_waiting(c: &Canvas, out: &mut Painted, k: WeatherKind, x: &Ctx, sh: Shape) {
    let city = x.cfg.weather_city.trim();
    let banner = k == WeatherKind::Banner;
    if banner {
        sky_card(c, (hex(SKY_CLEAR.0), hex(SKY_CLEAR.1)), sh);
    }
    let (t1, t2) = if banner { (WHITE, alpha(WHITE, SKY_T2)) } else { (c.text1(), c.text2()) };
    let bar = |bx: f32, by: f32, bw: f32, bh: f32| {
        if banner {
            c.rr(bx, by, bw, bh, (bh / 2.0).min(6.0), alpha(WHITE, 0.22));
        } else {
            skeleton(c, bx, by, bw, bh);
        }
    };
    let plate = |px: f32, py: f32, s: f32| {
        c.rr(px, py, s, s, s * style::BADGE_R, if banner { alpha(WHITE, 0.2) } else { c.plate() });
        c.glyph(G_SUN, px + s / 2.0, py + s / 2.0, s * style::GLYPH_IN, if banner { WHITE } else { c.yellow() });
    };
    if city.is_empty() {
        // Setup: the whole card opens the location form.
        out.hits.clear();
        out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Connect("weather")));
        // The widget's own short name, shorter still where it would not fit.
        let (full, short) = match k {
            WeatherKind::Sun => ("Sunrise & Sunset", "Sunrise"),
            WeatherKind::Hourly | WeatherKind::Week => ("Forecast", "Forecast"),
            _ => ("Weather", "Weather"),
        };
        let name_for = |avail: f32, size: f32| if c.measure(full, size, 600, Font::Text).0 <= avail { full } else { short };
        let reason = "Choose a location";
        // On the banner's sky the button is white with sky-blue text; elsewhere the blue pill.
        let pill = |out: &mut Painted, px: f32, py: f32, pw: f32, ph: f32| {
            let label = "Set location";
            if banner {
                c.rr(px, py, pw, ph, ph / 2.0, WHITE);
                let fs = style::CAPTION.size;
                c.text_cap(label, px + pw / 2.0, py + (ph - c.cap_h(fs)) / 2.0, fs, 600, hex(SKY_CLEAR.1), Align::Center, pw);
                out.hits.push(((px, py, pw, ph), WAct::Connect("weather")));
            } else {
                pill_button(c, out, px, py, pw, ph, label, WAct::Connect("weather"));
            }
        };
        match sh {
            Shape::Mini => {
                let kk = c.h / 54.0;
                let bs = style::BADGE_DOCK * kk;
                let (bx, by) = ((c.w - bs) / 2.0, (c.h - bs) / 2.0);
                plate(bx, by, bs);
                let (dx, dy, r) = (bx + bs - 1.0 * kk, by + 1.0 * kk, 3.5 * kk);
                c.circle(dx, dy, r + 1.5 * kk, c.halo());
                c.circle(dx, dy, r, if banner { WHITE } else { BLUE });
            }
            Shape::Dock => {
                let kk = c.h / 54.0;
                let m = style::M_DOCK * kk;
                let bs = style::BADGE_DOCK * kk;
                plate(m, (c.h - bs) / 2.0, bs);
                let x0 = m + bs + style::SP[4] * kk;
                let ts = style::TITLE.size * kk;
                c.text_base(name_for(c.w - m - x0, ts), x0, 30.0 * kk, ts, 600, t1, Align::Left, c.w - m - x0);
                c.text_base("Set location", x0, 44.0 * kk, style::FOOT.size * kk, 600, if banner { WHITE } else { BLUE }, Align::Left, c.w - m - x0);
            }
            Shape::Small => {
                let m = style::M_SMALL;
                plate(m, m, style::BADGE_SMALL);
                let inner = c.w - 2.0 * m;
                let pill_y = c.h - m - 26.0;
                c.text_base(name_for(inner, style::TITLE.size), m, pill_y - 22.0, style::TITLE.size, 600, t1, Align::Left, inner);
                c.text_base(reason, m, pill_y - 7.0, style::FOOT.size, 400, t2, Align::Left, inner);
                pill(out, m, pill_y, inner, 26.0);
            }
            Shape::Medium => {
                let m = style::M_CARD;
                plate(m, m, style::BADGE_SMALL);
                let x0 = m + style::BADGE_SMALL + style::SP[5];
                c.text_base(name_for(c.w - m - x0, 15.0), x0, 29.0, 15.0, 600, t1, Align::Left, c.w - m - x0);
                c.text_base(reason, x0, 46.0, style::CAPTION.size, 400, t2, Align::Left, c.w - m - x0);
                let (bw, bh) = (96.0, 28.0);
                pill(out, c.w - m - bw, c.h - m - bh, bw, bh);
            }
            Shape::Large => {
                let bs = 48.0;
                plate((c.w - bs) / 2.0, 36.0, bs);
                c.text_base(full, c.w / 2.0, 108.0, 15.0, 600, t1, Align::Center, c.w - 2.0 * style::M_CARD);
                c.text_base(reason, c.w / 2.0, 128.0, style::CAPTION.size, 400, t2, Align::Center, c.w - 2.0 * style::M_CARD);
                let (bw, bh) = (132.0, 30.0);
                pill(out, (c.w - bw) / 2.0, 150.0, bw, bh);
            }
        }
        return;
    }
    let ghost = if banner { alpha(WHITE, 0.5) } else { c.text3() };
    let track = if banner { alpha(WHITE, 0.22) } else { c.track() };
    let sep = if banner { alpha(WHITE, 0.2) } else { c.sep() };
    // A forecast column: a Micro label bar, an emoji disc and a temperature bar.
    let stub = |cx: f32, label_base: f32, disc_top: f32, ds: f32, temp_base: f32| {
        bar(cx - 9.0, label_base - 6.0, 18.0, 6.0);
        c.circle(cx, disc_top + ds / 2.0, ds / 2.0, track);
        bar(cx - 10.0, temp_base - 8.0, 20.0, 8.0);
    };
    match sh {
        Shape::Mini => {
            let kk = c.h / 54.0;
            c.glyph(G_SUN, c.w / 2.0, c.h / 2.0, 22.0 * kk, ghost);
        }
        Shape::Dock => {
            let kk = c.h / 54.0;
            let m = style::M_DOCK * kk;
            let bs = style::BADGE_DOCK * kk;
            c.glyph(G_SUN, m + bs / 2.0, c.h / 2.0, 22.0 * kk, ghost);
            let x0 = m + bs + style::SP[4] * kk;
            bar(x0, (c.h - 10.0 * kk) / 2.0, (60.0 * kk).min(c.w - m - x0), 10.0 * kk);
            if k == WeatherKind::Sun && c.w >= c.h * 3.4 {
                let slot_w = (c.w * 0.34).round();
                let slot_x = c.w - m - slot_w;
                sun_path(c, slot_x + slot_w / 2.0, 38.0 * kk, 22.0 * kk, (slot_x, slot_x + slot_w), None, (1.5, 1.5), 3.0 * kk, None);
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let inner = c.w - 2.0 * m;
            if k == WeatherKind::Sun {
                sun_path(c, c.w / 2.0, 58.0, 40.0, (m, c.w - m), None, (2.0, 2.5), 4.0, None);
                let ch = c.cap_h(style::HERO_S.size);
                bar(m, 86.0 - ch, 64.0, ch);
                bar(m, 106.0 - 8.0, 40.0, 8.0);
                return;
            }
            let mut cw = inner;
            if k == WeatherKind::Week {
                let bw = 24.0;
                bar(c.w - m - bw, m + c.cap_h(style::TITLE.size) - 8.0, bw, 8.0);
                cw -= bw + style::SP[3];
            }
            match k {
                WeatherKind::Hourly | WeatherKind::Week => {
                    c.text_cap(city, m, m, style::TITLE.size, style::TITLE.weight, t1, Align::Left, cw);
                    let colw = inner / 3.0;
                    for i in 0..3 {
                        let cx = m + colw * (i as f32 + 0.5);
                        if k == WeatherKind::Hourly {
                            stub(cx, 60.0, 66.0, 20.0, 106.0);
                        } else {
                            stub(cx, 52.0, 58.0, 20.0, 92.0);
                            bar(cx - 8.0, 106.0 - 6.0, 16.0, 6.0);
                        }
                    }
                }
                _ => {
                    let es = 24.0;
                    c.circle(c.w - m - es / 2.0, 12.0 + es / 2.0, es / 2.0, track);
                    c.text_cap(city, m, m, style::TITLE.size, style::TITLE.weight, t1, Align::Left, inner - es - style::SP[3]);
                    bar(m, 66.0 - 22.0, 60.0, 22.0);
                    bar(m, 90.0 - 8.0, 40.0, 8.0);
                    bar(m, 106.0 - 8.0, 40.0, 8.0);
                }
            }
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let r = c.w - m;
            let inner = r - m;
            let large = sh == Shape::Large;
            match k {
                WeatherKind::Sun => {
                    let ch = c.cap_h(style::HERO.size);
                    bar(m, m, 84.0, ch);
                    bar(m, 58.0 - 8.0, 64.0, 8.0);
                    bar(m, 76.0 - 8.0, 40.0, 8.0);
                    sun_path(c, 200.0, 72.0, 48.0, (148.0, r), None, (2.0, 2.5), 4.0, None);
                }
                WeatherKind::Hourly | WeatherKind::Week => {
                    let bw = 60.0;
                    bar(r - bw, style::HEADER_BASE - 8.0, bw, 8.0);
                    c.text_base(city, m, style::HEADER_BASE, style::TITLE.size, style::TITLE.weight, t1, Align::Left, inner - bw - style::SP[3]);
                    if k == WeatherKind::Week && large {
                        // The seven day rows: day, emoji, range track, high.
                        let pitch = (188.0 - style::CONTENT_TOP) / 7.0;
                        let h = style::METER_H_COMPACT;
                        for i in 0..7 {
                            let cy = style::CONTENT_TOP + pitch * (i as f32 + 0.5);
                            bar(m, cy - 4.0, 30.0, 8.0);
                            c.circle(88.0, cy, 7.0, track);
                            c.rr(142.0, cy - h / 2.0, 72.0, h, h / 2.0, track);
                            bar(r - 20.0, cy - 4.0, 20.0, 8.0);
                        }
                        return;
                    }
                    let n = if k == WeatherKind::Hourly { 6 } else { 5 };
                    let colw = inner / n as f32;
                    for i in 0..n {
                        stub(m + colw * (i as f32 + 0.5), 49.0, 52.5, 16.0, 80.0);
                    }
                }
                WeatherKind::Banner => {
                    let hw = 64.0;
                    bar(r - hw, m, hw, c.cap_h(style::HERO_XL.size));
                    let es = 28.0;
                    c.circle(r - es / 2.0, 80.0 - es / 2.0, es / 2.0, track);
                    let left_w = inner - hw - style::SP[6];
                    bar(m, m, 96.0, c.cap_h(style::VALUE.size));
                    c.text_base(city, m, 50.0, style::CAPTION.size, style::CAPTION.weight, t2, Align::Left, left_w);
                    bar(m, 80.0 - 8.0, 64.0, 8.0);
                }
                _ => {
                    let es = 40.0;
                    let detail = k == WeatherKind::Detail;
                    let title_w = if detail { 124.0 - m } else { inner - es - style::SP[3] };
                    c.text_base(city, m, 28.0, style::TITLE.size, style::TITLE.weight, t1, Align::Left, title_w);
                    bar(m, 61.0 - 22.0, 60.0, 22.0);
                    bar(m, 80.0 - 8.0, 40.0, 8.0);
                    if detail {
                        c.line(130.5, 16.0, 130.5, 80.0, 1.0, sep);
                        for i in 0..4 {
                            let base = 28.0 + i as f32 * (80.0 - 28.0) / 3.0;
                            bar(140.0, base - 8.0, 44.0, 8.0);
                            bar(r - 28.0, base - 8.0, 28.0, 8.0);
                        }
                    } else {
                        c.circle(r - es / 2.0, 14.0 + es / 2.0, es / 2.0, track);
                    }
                }
            }
            if large {
                c.line(m, style::SEP_Y_LARGE + 0.5, r, style::SEP_Y_LARGE + 0.5, 1.0, sep);
                let (dx, dy, dw, _) = detail_region(c);
                for (i, f) in [1.0, 0.75, 0.5].iter().enumerate() {
                    bar(dx, dy + 4.0 + i as f32 * style::ROW_L, dw * f, 8.0);
                }
            }
        }
    }
}

// ------------------------------------------------------------------ timers

// Filled (solid) Segoe Fluent transport glyphs: the outline ones read as hollow boxes next
// to album art and coloured buttons.
const G_PLAY_FILL: &str = "\u{F5B0}";
const G_PAUSE_FILL: &str = "\u{F8AE}";
const G_PREV_FILL: &str = "\u{F8AC}";
const G_NEXT_FILL: &str = "\u{F8AD}";
const G_MIC_OFF: &str = "\u{F781}";
const G_SPEAKERS: &str = "\u{E7F5}";
const G_HEADPHONES: &str = "\u{E7F6}";

/// A transport glyph centred on its ink. Outline transport glyphs are always drawn as
/// their filled forms. A play triangle's mass sits left of its box centre, so it is nudged
/// right to look centred in a circle.
fn transport_glyph(c: &Canvas, g: &str, cx: f32, cy: f32, size: f32, ink: Col) {
    let g = match g {
        G_PLAY => G_PLAY_FILL,
        G_PAUSE => G_PAUSE_FILL,
        G_PREV => G_PREV_FILL,
        G_NEXT => G_NEXT_FILL,
        _ => g,
    };
    let dx = if g == G_PLAY_FILL { size * 0.08 } else { 0.0 };
    c.glyph(g, cx + dx, cy, size, ink);
}

/// A round button: `fill`, a glyph at `GLYPH_IN` of its diameter, and a hit area of at
/// least `HIT_MIN` centred on it.
#[allow(clippy::too_many_arguments)]
fn round_button(c: &Canvas, out: &mut Painted, cx: f32, cy: f32, d: f32, fill: Col, g: &str, ink: Col, act: WAct) {
    c.circle(cx, cy, d / 2.0, fill);
    transport_glyph(c, g, cx, cy, d * style::GLYPH_IN, ink);
    let hd = d.max(style::HIT_MIN);
    out.hits.push(((cx - hd / 2.0, cy - hd / 2.0, hd, hd), act));
}

/// A clock time `offset` seconds from now: "3:42 PM".
fn clock_at(n: &super::Now, offset: f32) -> String {
    let t = (n.hour * 3600 + n.minute * 60 + n.second) as i64 + offset.round() as i64;
    let t = t.rem_euclid(86_400);
    let (h, m) = (t / 3600, t / 60 % 60);
    format!("{}:{m:02} {}", if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" })
}

/// "25 min", "1 h 30 min".
fn minutes_str(secs: u32) -> String {
    let m = (secs + 30) / 60;
    if m >= 60 && m % 60 == 0 {
        format!("{} h", m / 60)
    } else if m >= 60 {
        format!("{} h {} min", m / 60, m % 60)
    } else {
        format!("{m} min")
    }
}

/// Everything a timer face shows, worked out once for every size.
struct TimerView {
    /// Mode colour: Focus/Break red, Timer blue, Stopwatch orange.
    col: Col,
    /// The mode label ("Focus", "Break", "Timer", "Stopwatch", or "Done").
    label: String,
    time: String,
    /// Ring/meter fill: time remaining (countdowns) or the current minute (stopwatch).
    /// None for an idle stopwatch, which shows a dot at 12 o'clock instead.
    frac: Option<f32>,
    /// Dock caption: the mode when idle, "Ends 3:42 PM" / "Started 2:14 PM" while running.
    dock_caption: String,
    /// Small caption: "Ends 3:42 PM", "Paused" or "Ready".
    caption: String,
    /// Medium third line for the stopwatch ("Started 2:14 PM").
    started: String,
    /// Large: up to two (label, value) facts.
    facts: Vec<(String, String)>,
    /// What the primary button shows.
    glyph: &'static str,
    /// Light theme: the stopwatch's orange fill is too pale under a white glyph.
    fill: Col,
}

fn timer_view(c: &Canvas, k: TimerKind, x: &Ctx) -> TimerView {
    let n = &x.now;
    match k {
        TimerKind::Stopwatch => {
            let sw = &x.hub.stopwatch;
            let e = sw.elapsed().as_secs_f32();
            let running = sw.start.is_some();
            let used = e >= 0.5;
            let since = clock_at(n, -e);
            let state = if running { "Running" } else if used { "Paused" } else { "Ready" };
            let facts = if running {
                vec![("Started".to_string(), since.clone()), ("Status".to_string(), state.to_string())]
            } else if used {
                vec![("Elapsed".to_string(), hms(e)), ("Status".to_string(), state.to_string())]
            } else {
                // A fact with no value is drawn as a footnote hint.
                vec![("Status".to_string(), state.to_string()), ("Click play to start".to_string(), String::new())]
            };
            TimerView {
                col: ORANGE,
                label: "Stopwatch".into(),
                time: hms(e),
                frac: used.then(|| (e % 60.0) / 60.0),
                dock_caption: if running { format!("Started {since}") } else if used { "Paused".into() } else { "Stopwatch".into() },
                caption: if running { format!("Started {since}") } else { state.into() },
                started: if running { format!("Started {since}") } else { state.into() },
                facts,
                glyph: if running { G_PAUSE_FILL } else { G_PLAY_FILL },
                fill: if c.light { hex(0xF28A00) } else { ORANGE },
            }
        }
        _ => {
            let t = &x.hub.timers[&k];
            let left = t.left();
            let running = t.running();
            let paused = !running && !t.done && left < t.total as f32 - 0.5;
            let pomodoro = k == TimerKind::Pomodoro;
            let mode = match (pomodoro, t.phase) {
                (true, 0) => "Focus",
                (true, _) => "Break",
                _ => "Timer",
            };
            let ends = clock_at(n, left);
            let state = if t.done { "Done" } else if paused { "Paused" } else { "Ready" };
            let length = minutes_str(t.total);
            let facts = if pomodoro {
                let next = if t.phase == 0 { "Break · 5 min" } else { "Focus · 25 min" };
                vec![if running { ("Ends".to_string(), ends.clone()) } else { ("Length".to_string(), length) }, ("Next".to_string(), next.to_string())]
            } else if running {
                vec![("Ends".to_string(), ends.clone()), ("Length".to_string(), length)]
            } else {
                vec![("Length".to_string(), length), ("Status".to_string(), state.to_string())]
            };
            TimerView {
                col: if pomodoro { RED } else { BLUE },
                label: if t.done { "Done".into() } else { mode.into() },
                time: hms(left),
                frac: Some(if t.done { 1.0 } else { left / t.total.max(1) as f32 }),
                dock_caption: if running {
                    format!("Ends {ends}")
                } else if t.done || paused {
                    state.into()
                } else {
                    mode.into()
                },
                caption: if running { format!("Ends {ends}") } else { state.into() },
                started: String::new(),
                facts,
                glyph: if t.done {
                    G_RESET
                } else if running {
                    G_PAUSE_FILL
                } else {
                    G_PLAY_FILL
                },
                fill: if pomodoro { RED } else { BLUE },
            }
        }
    }
}

/// The timer ring: remaining time in the mode colour over the track; an idle stopwatch
/// shows the track and a dot at 12 o'clock.
fn timer_ring(c: &Canvas, v: &TimerView, cx: f32, cy: f32, d: f32, t: f32) {
    let r = d / 2.0 - t / 2.0;
    match v.frac {
        Some(f) => c.ring(cx, cy, r, t, f, v.col),
        None => {
            c.circle_stroke(cx, cy, r, c.track(), t);
            c.circle(cx, cy - r, t / 2.0, v.col);
        }
    }
}

/// The time as short as the inside of a Mini ring allows: "4:59" under ten minutes, whole
/// minutes from ten ("25m": a countdown rounds up, so 24:10 left still reads "25m"), then
/// hours ("1h05", or "2h" when that is too wide).
fn mini_time(c: &Canvas, v: &TimerView, secs: f32, countdown: bool, size: f32, max_w: f32) -> String {
    let s = secs.max(0.0);
    if s < 600.0 {
        return v.time.clone();
    }
    let mins = if countdown { (s / 60.0).ceil() as u32 } else { (s / 60.0).floor() as u32 };
    if mins < 60 {
        return format!("{mins}m");
    }
    let hm = format!("{}h{:02}", mins / 60, mins % 60);
    if mins % 60 != 0 && c.measure(&hm, size, 600, Font::Text).0 <= max_w {
        hm
    } else {
        format!("{}h", mins / 60)
    }
}

/// The mode mark inside a dock ring when play lives in the accessory slot.
const G_STOPWATCH: &str = "\u{E916}";

/// Reset (secondary) and play (primary) buttons, right-aligned at `right`, 8 apart.
#[allow(clippy::too_many_arguments)]
fn timer_buttons(c: &Canvas, out: &mut Painted, k: TimerKind, v: &TimerView, right: f32, cy: f32, primary: f32, secondary: f32, gap: f32) {
    let pcx = right - primary / 2.0;
    let rcx = pcx - primary / 2.0 - gap - secondary / 2.0;
    // Done: the primary button restarts, so a second reset button would only repeat it.
    if v.glyph != G_RESET {
        round_button(c, out, rcx, cy, secondary, c.plate(), G_RESET, c.text1(), WAct::TimerReset(k));
    }
    round_button(c, out, pcx, cy, primary, v.fill, v.glyph, WHITE, WAct::Timer(k));
}

fn timer(c: &Canvas, out: &mut Painted, k: TimerKind, x: &Ctx, sh: Shape) {
    let v = timer_view(c, k, x);
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Timer(k)));
    let secs = match k {
        TimerKind::Stopwatch => x.hub.stopwatch.elapsed().as_secs_f32(),
        _ => x.hub.timers[&k].left(),
    };
    match sh {
        Shape::Mini => {
            // A ring the size of an app icon, the time inside: identifiable by colour alone.
            let kk = c.h / 54.0;
            let d = 40.0 * kk;
            let t = style::METER_H_COMPACT * kk;
            let (cx, cy) = (c.w / 2.0, c.h / 2.0);
            timer_ring(c, &v, cx, cy, d, t);
            // At least 4 px of clear glass between the digits and the ring's inner edge.
            let inner = d - 2.0 * t - 8.0 * kk;
            let s = mini_time(c, &v, secs, k != TimerKind::Stopwatch, 11.0 * kk, inner);
            let fs = fit_down(c, &s, 12.0 * kk, 11.0 * kk, 600, inner);
            c.text_cap(&s, cx, cy - c.cap_h(fs) / 2.0, fs, 600, c.text1(), Align::Center, c.w * 2.0);
        }
        Shape::Dock => {
            let kk = c.h / 54.0;
            let m = style::M_DOCK * kk;
            let d = style::BADGE_DOCK * kk;
            let (cx, cy) = (m + d / 2.0, c.h / 2.0);
            timer_ring(c, &v, cx, cy, d, 3.5 * kk);
            // Dock L puts play in the accessory slot; Dock M plays from the ring.
            let wide = c.w >= c.h * 3.4;
            if wide {
                // Play is in the slot; the ring keeps a mode mark so it reads as an icon.
                c.glyph(G_STOPWATCH, cx, cy, 12.0 * kk, v.col);
            } else {
                transport_glyph(c, v.glyph, cx, cy, 12.0 * kk, v.col);
            }
            let x0 = 52.0 * kk;
            let slot_w = (c.w * 0.34).round();
            let right = if wide { c.w - m - slot_w - style::SP[3] * kk } else { c.w - m };
            let role = Type { size: style::VALUE.size * kk, min: style::VALUE.min * kk, ..style::VALUE };
            let (val, fs) = fit_value(c, &v.time, role, right - x0);
            c.text_base(&val, x0, 30.0 * kk, fs, role.weight, c.text1(), Align::Left, c.w * 2.0);
            let cs = fit_down(c, &v.dock_caption, style::FOOT.size * kk, style::FOOT.min * kk, style::FOOT.weight, right - x0);
            c.text_base(&v.dock_caption, x0, 44.0 * kk, cs, style::FOOT.weight, c.text2(), Align::Left, right - x0);
            if wide {
                timer_buttons(c, out, k, &v, c.w - m, cy, style::BTN_PRIMARY_DOCK * kk, style::BTN_SECONDARY_DOCK * kk, style::SP[3] * kk);
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            c.text_cap(&v.label, m, 16.0, style::MICRO.size, style::MICRO.weight, v.col, Align::Left, c.w - 2.0 * m);
            let role = Type { weight: style::time_weight(style::HERO_S.size), ..style::HERO_S };
            let (t, fs) = fit_value(c, &v.time, role, c.w - 2.0 * m);
            c.text_base(&t, m, 52.0, fs, role.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base(&v.caption, m, 70.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, c.w - 2.0 * m);
            let p = style::BTN_PRIMARY_SMALL;
            timer_buttons(c, out, k, &v, c.w - m, c.h - m - p / 2.0, p, style::BTN_SECONDARY_SMALL, style::SP[3]);
        }
        Shape::Medium => {
            let m = style::M_CARD;
            c.text_cap(&v.label, m, m, style::MICRO.size, style::MICRO.weight, v.col, Align::Left, 140.0);
            let role = Type { weight: style::time_weight(style::HERO.size), ..style::HERO };
            let (t, fs) = fit_value(c, &v.time, role, 160.0);
            c.text_base(&t, m, 58.0, fs, role.weight, c.text1(), Align::Left, c.w * 2.0);
            match (k, v.frac) {
                (TimerKind::Stopwatch, _) => {
                    c.text_base(&v.started, m, 76.0, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, 140.0);
                }
                (_, Some(f)) => c.bar(m, 70.0, 124.0, style::METER_H, f, v.col),
                _ => {}
            }
            timer_buttons(c, out, k, &v, c.w - m, c.h / 2.0, style::BTN_PRIMARY, style::BTN_SECONDARY, style::SP[3]);
        }
        Shape::Large => {
            // A ring with the time inside on the left, facts and controls on the right.
            let m = style::M_CARD;
            let d = 120.0;
            let t = (d * style::RING_T).max(3.0);
            let (cx, cy) = (m + d / 2.0, c.h / 2.0);
            timer_ring(c, &v, cx, cy, d, t);
            let role = Type { weight: style::time_weight(style::HERO.size), ..style::HERO };
            let (tm, fs) = fit_value(c, &v.time, role, d - 2.0 * t - style::SP[5]);
            let (lc, hc) = (c.cap_h(style::MICRO.size), c.cap_h(fs));
            let top = cy - (lc + style::SP[3] + hc) / 2.0;
            c.text_cap(&v.label, cx, top, style::MICRO.size, style::MICRO.weight, v.col, Align::Center, d);
            c.text_cap(&tm, cx, top + lc + style::SP[3], fs, role.weight, c.text1(), Align::Center, c.w * 2.0);
            let (fx, fw) = (152.0, c.w - m - 152.0);
            for (i, (label, value)) in v.facts.iter().take(2).enumerate() {
                let lb = 52.0 + i as f32 * 48.0;
                if value.is_empty() {
                    c.text_base(label, fx, lb, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, fw);
                    continue;
                }
                c.text_base(label, fx, lb, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, fw);
                let (val, vs) = fit_value(c, value, style::VALUE, fw);
                c.text_base(&val, fx, lb + 22.0, vs, style::VALUE.weight, c.text1(), Align::Left, c.w * 2.0);
            }
            timer_buttons(c, out, k, &v, c.w - m, c.h - m - style::BTN_PRIMARY / 2.0, style::BTN_PRIMARY, style::BTN_SECONDARY, style::SP[3]);
        }
    }
}

// ------------------------------------------------------------------ media

#[allow(clippy::too_many_arguments)]
fn art_src(x: &Ctx, wic: &IWICImagingFactory) -> Option<IWICBitmapSource> {
    x.hub.art_img.as_ref().map(|a| a.1.clone()).or_else(|| x.control.media.as_ref().and_then(|m| m.thumbnail_png.as_ref()).and_then(|b| load_image_bytes(wic, b)))
}

fn art(c: &Canvas, x: &Ctx, wic: &IWICImagingFactory, ax: f32, ay: f32, s: f32, r: f32) {
    art_note(c, x, wic, ax, ay, s, r, true);
}

/// The art; without a cover, a gradient tile with a music note (left off under the
/// paused overlay, where the two glyphs would collide).
#[allow(clippy::too_many_arguments)]
fn art_note(c: &Canvas, x: &Ctx, wic: &IWICImagingFactory, ax: f32, ay: f32, s: f32, r: f32, note: bool) {
    match art_src(x, wic) {
        Some(img) => {
            c.image(&img, ax, ay, s, s, r);
            // Pale covers need an edge on light glass.
            if c.light {
                c.rr_stroke(ax, ay, s, s, r, c.card_edge(), 1.0);
            }
        }
        None => {
            c.rr_grad(ax, ay, s, s, r, hex(0xFF5F6D), hex(0xC2185B));
            if note {
                c.glyph(G_MUSIC, ax + s / 2.0, ay + s / 2.0, s * 0.42, (1.0, 1.0, 1.0, 0.95));
            }
        }
    }
}

/// The transport row as bare glyphs (prev, play/pause, next) at the given centres. `dim`
/// draws it at 0.35 and inert, for when nothing is playing.
#[allow(clippy::too_many_arguments)]
fn controls(c: &Canvas, out: &mut Painted, xs: [f32; 3], cy: f32, sizes: [f32; 3], playing: bool, dim: bool) {
    let play = if playing { G_PAUSE_FILL } else { G_PLAY_FILL };
    let ink = if dim { alpha(c.text1(), 0.35) } else { c.text1() };
    for (i, (g, a)) in [(G_PREV_FILL, WAct::MediaPrev), (play, WAct::MediaPlay), (G_NEXT_FILL, WAct::MediaNext)].into_iter().enumerate() {
        transport_glyph(c, g, xs[i], cy, sizes[i], ink);
        if !dim {
            let hd = style::HIT_MIN;
            out.hits.push(((xs[i] - hd / 2.0, cy - hd / 2.0, hd, hd), a));
        }
    }
}

/// A plate button drawn at 0.35 with no hit of its own: the idle stand-in for a control.
fn dim_plate(c: &Canvas, cx: f32, cy: f32, d: f32, g: &str) {
    c.circle(cx, cy, d / 2.0, alpha(c.plate(), 0.35));
    transport_glyph(c, g, cx, cy, d * style::GLYPH_IN, alpha(c.text1(), 0.35));
}

/// The muted red. Light theme takes the deeper red so white text on it reaches 4.5:1.
fn mute_red(c: &Canvas) -> Col {
    if c.light {
        hex(0xD70015)
    } else {
        RED
    }
}

/// What the media faces show, worked out once for every size.
#[derive(Clone)]
struct Track {
    title: String,
    artist: String,
    album: String,
    /// The source app's display name ("Spotify"), or empty.
    source: String,
    playing: bool,
    /// Nothing is playing: the faces show a calm "Not playing" state.
    idle: bool,
}

impl Track {
    /// The supporting line under the artist: the album, else the source app, else the
    /// playback state, so a Medium always has its third layer.
    fn third(&self) -> String {
        if !self.album.is_empty() {
            self.album.clone()
        } else if !self.source.is_empty() {
            self.source.clone()
        } else {
            self.verb().into()
        }
    }
    fn verb(&self) -> &'static str {
        if self.playing {
            "Playing"
        } else {
            "Paused"
        }
    }
    /// "Playing in Spotify" / "Paused in Spotify"; without a source, "Paused" or nothing.
    fn where_line(&self) -> String {
        if self.source.is_empty() {
            if self.playing {
                String::new()
            } else {
                "Paused".into()
            }
        } else {
            format!("{} in {}", self.verb(), self.source)
        }
    }
    /// Like `where_line`, but never empty: "Playing" / "Paused" without a source.
    fn status_line(&self) -> String {
        if self.source.is_empty() {
            self.verb().into()
        } else {
            self.where_line()
        }
    }
}

/// The source app's display name from its AppUserModelID or exe name.
fn source_app(id: &str) -> String {
    let s = id.rsplit('!').next().unwrap_or(id);
    let s = s.rsplit(['\\', '/']).next().unwrap_or(s);
    let s = s.strip_suffix(".exe").or_else(|| s.strip_suffix(".EXE")).unwrap_or(s);
    let s = s.split('_').next().unwrap_or(s);
    let s = s.rsplit('.').next().unwrap_or(s).trim();
    // Some apps register a hash as their id; it means nothing to people.
    if s.is_empty() || (s.len() >= 12 && s.chars().all(|ch| ch.is_ascii_hexdigit())) {
        return String::new();
    }
    match s.to_ascii_lowercase().as_str() {
        "msedge" => "Edge".into(),
        "chrome" => "Chrome".into(),
        "firefox" => "Firefox".into(),
        "zunemusic" | "mediaplayer" => "Media Player".into(),
        "spotify" | "spotifymusic" => "Spotify".into(),
        "vlc" => "VLC".into(),
        _ => {
            let mut ch = s.chars();
            ch.next().map(|f| f.to_uppercase().collect::<String>() + ch.as_str()).unwrap_or_default()
        }
    }
}

/// The progress of the current track (elapsed, length) in seconds, when the source app
/// reports a timeline. `NowPlaying` doesn't carry one yet, so the Bar face falls back to a
/// level glyph; this is the one place to wire it up.
fn track_timeline(_m: &Track) -> Option<(f32, f32)> {
    None
}

/// "1:23".
fn mmss(secs: f32) -> String {
    let s = secs.max(0.0).round() as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The art, or (nothing playing) a plate-coloured tile with a music note.
#[allow(clippy::too_many_arguments)]
fn art_or_idle(c: &Canvas, x: &Ctx, wic: &IWICImagingFactory, t: &Track, ax: f32, ay: f32, s: f32, r: f32) {
    if t.idle {
        c.rr(ax, ay, s, s, r, c.plate());
        c.glyph(G_MUSIC, ax + s / 2.0, ay + s / 2.0, (s * style::GLYPH_IN).min(24.0), c.text3());
    } else {
        art(c, x, wic, ax, ay, s, r);
    }
}

/// A record: black vinyl with grooves, a coloured label (grey when nothing plays) and a
/// spindle. With album art, the art is the record's face.
#[allow(clippy::too_many_arguments)]
fn record(c: &Canvas, x: &Ctx, wic: &IWICImagingFactory, t: &Track, cx: f32, cy: f32, d: f32, cover: bool) {
    let r = d / 2.0;
    if cover && !t.idle && art_src(x, wic).is_some() {
        art(c, x, wic, cx - r, cy - r, d, r);
    } else {
        c.circle(cx, cy, r, (0.07, 0.07, 0.08, 1.0));
        let gt = if d >= 80.0 { 1.0 } else { 0.75 };
        for k in 1..5 {
            c.circle_stroke(cx, cy, r * (0.48 + k as f32 * 0.11), (1.0, 1.0, 1.0, 0.07), gt);
        }
        let l = d * 0.38;
        if t.idle {
            c.circle(cx, cy, l / 2.0, hex(0x8E8E93));
        } else {
            c.rr_grad(cx - l / 2.0, cy - l / 2.0, l, l, l / 2.0, hex(0xFF5F6D), hex(0xC2185B));
        }
    }
    c.circle_stroke(cx, cy, r - 0.5, (1.0, 1.0, 1.0, 0.18), 1.0);
    c.circle(cx, cy, (d * 0.04).max(1.5), (0.1, 0.1, 0.12, 1.0));
}

/// Paused, on a face without its own play button: a filled play glyph on a dark scrim.
fn paused_overlay(c: &Canvas, cx: f32, cy: f32, d: f32) {
    c.circle(cx, cy, d / 2.0, (0.0, 0.0, 0.0, 0.35));
    transport_glyph(c, G_PLAY_FILL, cx, cy, d * 0.5, WHITE);
}

/// A secondary (plate) play/pause button.
fn play_plate(c: &Canvas, out: &mut Painted, cx: f32, cy: f32, d: f32, playing: bool) {
    round_button(c, out, cx, cy, d, c.plate(), if playing { G_PAUSE_FILL } else { G_PLAY_FILL }, c.text1(), WAct::MediaPlay);
}

/// The primary play/pause button: a text1 disc with a card-coloured glyph.
fn play_primary(c: &Canvas, out: &mut Painted, cx: f32, cy: f32, d: f32, playing: bool, a: f32) {
    let g = if playing { G_PAUSE_FILL } else { G_PLAY_FILL };
    c.circle(cx, cy, d / 2.0, alpha(c.text1(), a));
    transport_glyph(c, g, cx, cy, d * style::GLYPH_IN, c.halo());
    if a >= 1.0 {
        let hd = d.max(style::HIT_MIN);
        out.hits.push(((cx - hd / 2.0, cy - hd / 2.0, hd, hd), WAct::MediaPlay));
    }
}

/// The Large transport row: prev and next plates (28) either side of the primary play (36),
/// centres `gap` apart. Dimmed and inert when nothing is playing.
fn transport_row(c: &Canvas, out: &mut Painted, cx: f32, cy: f32, gap: f32, t: &Track) {
    let a = if t.idle { 0.35 } else { 1.0 };
    let s = style::BTN_SECONDARY;
    for (dx, g, act) in [(-gap, G_PREV_FILL, WAct::MediaPrev), (gap, G_NEXT_FILL, WAct::MediaNext)] {
        c.circle(cx + dx, cy, s / 2.0, alpha(c.plate(), a));
        transport_glyph(c, g, cx + dx, cy, s * style::GLYPH_IN, alpha(c.text1(), a));
        if !t.idle {
            out.hits.push(((cx + dx - s / 2.0, cy - s / 2.0, s, s), act));
        }
    }
    play_primary(c, out, cx, cy, style::BTN_PRIMARY, t.playing, a);
}

/// A calm three-bar level mark (text2) with its bottom on `base`, while playing.
fn level_glyph(c: &Canvas, x0: f32, base: f32, playing: bool, kk: f32) {
    // Paused, three flat bars would read as an ellipsis: show nothing.
    if !playing {
        return;
    }
    let hs = [6.0, 10.0, 7.5];
    for (i, h) in hs.iter().enumerate() {
        c.rr(x0 + i as f32 * 4.0 * kk, base - h * kk, 2.5 * kk, h * kk, 1.25 * kk, c.text2());
    }
}
const LEVEL_W: f32 = 10.5;

/// A title that must not be cut: up to `lines` lines at the role's size, shrinking toward
/// its minimum before anything is ellipsized. Returns the lines and the size.
fn wrap_title(c: &Canvas, s: &str, role: Type, max_w: f32, lines: usize) -> (Vec<String>, f32) {
    let greedy = |size: f32| {
        let mut out: Vec<String> = Vec::new();
        let mut cur = String::new();
        for word in s.split_whitespace() {
            let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if cur.is_empty() || c.measure(&cand, size, role.weight, Font::Text).0 <= max_w {
                cur = cand;
            } else {
                out.push(std::mem::replace(&mut cur, word.to_string()));
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    };
    let mut size = role.size;
    loop {
        let ls = greedy(size);
        let fits = ls.len() <= lines && ls.iter().all(|l| c.measure(l, size, role.weight, Font::Text).0 <= max_w);
        if fits || size <= role.min {
            let mut ls = ls;
            if ls.len() > lines {
                let rest = ls.split_off(lines - 1).join(" ");
                ls.push(rest);
            }
            return (ls, size);
        }
        size = (size - 0.5).max(role.min);
    }
}

/// The largest size from `start` down to `min` at which `s` fits `max_w`, measured at each
/// step: below 12 px the text switches to the wider Small optical size, so a linear guess
/// overshoots.
fn fit_down(c: &Canvas, s: &str, start: f32, min: f32, weight: u32, max_w: f32) -> f32 {
    let mut size = fit(c, s, start, min, weight, Font::Text, max_w);
    while size > min && c.measure(s, size, weight, Font::Text).0 > max_w {
        size = (size - 0.25).max(min);
    }
    size
}

/// `s` cut at a word boundary to fit `max_w`, with an ellipsis; a single over-long word is
/// left to the layout's own trimming.
fn word_cut(c: &Canvas, s: &str, size: f32, weight: u32, max_w: f32) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    for n in (1..words.len()).rev() {
        let cand = format!("{}\u{2026}", words[..n].join(" "));
        if c.measure(&cand, size, weight, Font::Text).0 <= max_w {
            return cand;
        }
    }
    s.to_string()
}

/// Title and artist for the dock pills. The title fits from 13 down to 11 on one line, with
/// the artist under it. When that is still too wide (the 76 px column of Dock M), the title
/// takes both lines at 13 → 11 and the artist gives way: the art already carries the track.
/// A title too long even for two lines keeps its whole first line and is cut on the second
/// (at a word boundary when one fits), so it never shrinks to a stub like "The Long…".
fn dock_track_text(c: &Canvas, t: &Track, x0: f32, right: f32, kk: f32) {
    let w = right - x0;
    let (top, min) = (style::TITLE.size * kk, style::TITLE.min * kk);
    let fs = fit_down(c, &t.title, top, min, style::TITLE.weight, w);
    if c.measure(&t.title, fs, style::TITLE.weight, Font::Text).0 <= w {
        c.text_base(&t.title, x0, 25.0 * kk, fs, style::TITLE.weight, c.text1(), Align::Left, w);
        c.text_base(&t.artist, x0, 39.0 * kk, style::FOOT.size * kk, style::FOOT.weight, c.text2(), Align::Left, w);
        return;
    }
    let role = Type { size: top, min, ..style::TITLE };
    let (mut lines, fs) = wrap_title(c, &t.title, role, w, 2);
    if lines.len() == 2 && c.measure(&lines[1], fs, style::TITLE.weight, Font::Text).0 > w {
        lines[1] = word_cut(c, &lines[1], fs, style::TITLE.weight, w);
    }
    let pitch = (fs * 1.25).round();
    let n = lines.len() as f32;
    let b0 = (c.h + c.cap_h(fs) - pitch * (n - 1.0)) / 2.0;
    for (i, l) in lines.iter().enumerate() {
        // The layout trims a single over-long word itself, with an ellipsis.
        c.text_base(l, x0, b0 + i as f32 * pitch, fs, style::TITLE.weight, c.text1(), Align::Left, w);
    }
}

/// Title (fitted), artist and an optional third line from `base`, for card layouts.
#[allow(clippy::too_many_arguments)]
fn card_track_text(c: &Canvas, t: &Track, x0: f32, w: f32, title_base: f32, artist_base: f32, artist_size: f32, third: Option<(f32, String)>) {
    let fs = fit_down(c, &t.title, style::TITLE.size, style::TITLE.min, style::TITLE.weight, w);
    c.text_base(&t.title, x0, title_base, fs, style::TITLE.weight, c.text1(), Align::Left, w);
    c.text_base(&t.artist, x0, artist_base, artist_size, 400, c.text2(), Align::Left, w);
    if let Some((b, s)) = third {
        c.text_base(&s, x0, b, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, w);
    }
}

/// Large: title 17/600 (up to two lines), artist 13 text2 and album 12 text3 in a column
/// whose first capitals sit at `cap_top`.
#[allow(clippy::too_many_arguments)]
fn large_track_text(c: &Canvas, t: &Track, x0: f32, w: f32, cap_top: f32, third: &str, max_lines: usize) {
    let (lines, fs) = wrap_title(c, &t.title, style::VALUE, w, max_lines);
    let mut base = cap_top + c.cap_h(fs);
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            base += (fs * 1.2).round();
        }
        c.text_base(l, x0, base, fs, style::VALUE.weight, c.text1(), Align::Left, w);
    }
    base += style::SP[7];
    c.text_base(&t.artist, x0, base, style::BODY.size, 400, c.text2(), Align::Left, w);
    if !third.is_empty() {
        base += 17.0;
        c.text_base(third, x0, base, style::CAPTION.size, 400, c.text3(), Align::Left, w);
    }
}

fn media(c: &Canvas, out: &mut Painted, k: MediaKind, x: &Ctx, wic: &IWICImagingFactory, sh: Shape) {
    match k {
        MediaKind::Output => return output_face(c, out, x, sh),
        MediaKind::Mic => return mic_face(c, out, x, sh),
        _ => {}
    }
    let t = match x.control.media.as_ref() {
        Some(m) => Track { title: m.title.clone(), artist: m.artist.clone(), album: m.album.clone(), source: source_app(&m.app_id), playing: m.playing, idle: false },
        None => Track { title: "Not playing".into(), artist: String::new(), album: String::new(), source: String::new(), playing: false, idle: true },
    };
    // The Disc draws its own card on the Launchpad (the dock pill's comes from paint()).
    if k == MediaKind::Disc && !matches!(sh, Shape::Mini | Shape::Dock) {
        bg(c, false);
    }
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::MediaPlay));
    match sh {
        Shape::Mini => {
            let kk = c.h / 54.0;
            let s = 40.0 * kk;
            let (cx, cy) = (c.w / 2.0, c.h / 2.0);
            if k == MediaKind::Disc {
                record(c, x, wic, &t, cx, cy, s, true);
            } else if t.idle {
                c.rr(cx - s / 2.0, cy - s / 2.0, s, s, style::concentric(style::DOCK_R, style::M_MINI) * kk, c.plate());
                c.glyph(G_MUSIC, cx, cy, 18.0 * kk, c.text3());
            } else {
                art_note(c, x, wic, cx - s / 2.0, cy - s / 2.0, s, style::concentric(style::DOCK_R, style::M_MINI) * kk, t.playing);
            }
            if !t.idle && !t.playing {
                paused_overlay(c, cx, cy, 22.0 * kk);
            }
        }
        Shape::Dock => {
            let kk = c.h / 54.0;
            let m = style::M_DOCK * kk;
            let wide = c.w >= c.h * 3.4;
            let slot_w = (c.w * 0.34).round();
            let slot_x = c.w - m - slot_w;
            let s = 42.0 * kk;
            let a0 = 6.0 * kk;
            let pd = style::BTN_SECONDARY_DOCK * kk;
            if t.idle {
                // The Disc keeps its grey-label record, as on the Mini; the others a plate.
                let x0 = if k == MediaKind::Disc {
                    record(c, x, wic, &t, a0 + s / 2.0, c.h / 2.0, s, true);
                    56.0 * kk
                } else {
                    let p = style::BADGE_DOCK * kk;
                    c.rr(m, (c.h - p) / 2.0, p, p, p * style::BADGE_R, c.plate());
                    c.glyph(G_MUSIC, m + p / 2.0, c.h / 2.0, p * style::GLYPH_IN, c.text3());
                    52.0 * kk
                };
                // Dock L keeps its slot: the control it will hold, dimmed.
                let cy = c.h / 2.0;
                let mut right = c.w - m;
                if wide {
                    let pcx = if k == MediaKind::Full {
                        let ns = 14.0 * kk;
                        let ncx = c.w - m - ns / 2.0;
                        transport_glyph(c, G_NEXT_FILL, ncx, cy, ns, alpha(c.text1(), 0.35));
                        ncx - ns / 2.0 - style::SP[3] * kk - pd / 2.0
                    } else {
                        c.w - m - pd / 2.0
                    };
                    dim_plate(c, pcx, cy, pd, G_PLAY_FILL);
                    right = pcx - pd / 2.0 - style::SP[3] * kk;
                }
                c.text_base("Not playing", x0, 30.0 * kk, style::TITLE.size * kk, style::TITLE.weight, c.text1(), Align::Left, right - x0);
                let hint = "Play in any app";
                let fs = fit_down(c, hint, style::FOOT.size * kk, style::FOOT.min * kk, style::FOOT.weight, right - x0);
                c.text_base(hint, x0, 44.0 * kk, fs, style::FOOT.weight, c.text2(), Align::Left, right - x0);
                return;
            }
            if k == MediaKind::Disc {
                record(c, x, wic, &t, a0 + s / 2.0, c.h / 2.0, s, true);
            } else {
                art_note(c, x, wic, a0, a0, s, style::R_MIN * kk, t.playing || wide);
            }
            let x0 = 56.0 * kk;
            // The slot is only as wide as what it holds, so the title keeps its room.
            let used = match (k, track_timeline(&t)) {
                (MediaKind::Full, _) => pd + style::SP[3] * kk + 14.0 * kk,
                (MediaKind::Bar, Some(_)) => slot_w,
                (MediaKind::Bar, None) => pd + style::SP[3] * kk + LEVEL_W * kk,
                _ => pd,
            };
            let right = if wide { c.w - m - used - style::SP[3] * kk } else { c.w - 8.0 * kk };
            dock_track_text(c, &t, x0, right, kk);
            if !wide {
                if !t.playing {
                    paused_overlay(c, a0 + s / 2.0, c.h / 2.0, 22.0 * kk);
                }
                return;
            }
            let cy = c.h / 2.0;
            match k {
                MediaKind::Full => {
                    let ns = 14.0 * kk;
                    let ncx = c.w - m - ns / 2.0;
                    transport_glyph(c, G_NEXT_FILL, ncx, cy, ns, c.text1());
                    out.hits.push(((ncx - 14.0 * kk, cy - 14.0 * kk, 28.0 * kk, 28.0 * kk), WAct::MediaNext));
                    play_plate(c, out, ncx - ns / 2.0 - style::SP[3] * kk - pd / 2.0, cy, pd, t.playing);
                }
                MediaKind::Bar => match track_timeline(&t) {
                    Some((pos, len)) => {
                        let f = (pos / len.max(1.0)).clamp(0.0, 1.0);
                        c.text_base(&mmss(pos), c.w - m, 26.0 * kk, style::FOOT.size * kk, 500, c.text2(), Align::Right, slot_w);
                        c.bar(slot_x, 33.0 * kk, slot_w, style::METER_H_COMPACT * kk, f, c.text1());
                    }
                    None => {
                        let pcx = c.w - m - pd / 2.0;
                        play_plate(c, out, pcx, cy, pd, t.playing);
                        level_glyph(c, pcx - pd / 2.0 - style::SP[3] * kk - LEVEL_W * kk, cy + 5.0 * kk, t.playing, kk);
                    }
                },
                _ => play_plate(c, out, c.w - m - pd / 2.0, cy, pd, t.playing),
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let inner = c.w - 2.0 * m;
            let r = style::concentric(style::CARD_R, 8.0);
            match k {
                MediaKind::Disc => {
                    // The record top-left, the track bottom-anchored under it (the Small
                    // template), so the Small says at least what the Dock M says.
                    let d = 60.0;
                    let (cx, cy) = (m + d / 2.0, m + d / 2.0);
                    record(c, x, wic, &t, cx, cy, d, true);
                    if !t.idle && !t.playing {
                        paused_overlay(c, cx, cy, 26.0);
                    }
                    let artist = if t.idle { "Play in any app".to_string() } else { t.artist.clone() };
                    card_track_text(c, &Track { artist, ..t.clone() }, m, inner, 90.0, 106.0, style::CAPTION.size, None);
                }
                MediaKind::Full => {
                    art_or_idle(c, x, wic, &t, m, m, 40.0, r);
                    let t2 = if t.idle { Track { artist: "Play in any app".into(), ..t.clone() } } else { t.clone() };
                    card_track_text(c, &t2, m, inner, 70.0, 84.0, style::FOOT.size, None);
                    // Prev's left ink edge on the margin, next's right ink edge on the other.
                    // Idle, the row stays (dimmed) so the layout doesn't collapse.
                    controls(c, out, [m + 7.0, c.w / 2.0, c.w - m - 7.0], 98.0, [14.0, 18.0, 14.0], t.playing, t.idle);
                }
                MediaKind::Compact => {
                    art_or_idle(c, x, wic, &t, m, m, 52.0, r);
                    let p = style::BTN_SECONDARY;
                    if !t.idle {
                        play_plate(c, out, c.w - m - p / 2.0, m + p / 2.0, p, t.playing);
                    }
                    let artist = if t.idle { "Play in any app".to_string() } else { t.artist.clone() };
                    card_track_text(c, &Track { artist, ..t.clone() }, m, inner, 86.0, 106.0, style::CAPTION.size, None);
                }
                _ => {
                    // Bar: the player bar in miniature. Art with the live level beside it,
                    // the track, then the transport capsule on the bottom margin (dimmed
                    // when nothing plays), in the same language as the Bar Medium.
                    let s = 36.0;
                    art_or_idle(c, x, wic, &t, m, m, s, r);
                    let t2 = if t.idle { Track { artist: "Play in any app".into(), ..t.clone() } } else { t.clone() };
                    card_track_text(c, &t2, m, inner, 66.0, 80.0, style::FOOT.size, None);
                    if let Some((pos, len)) = track_timeline(&t) {
                        // With a timeline, the capsule's place holds the track's progress.
                        let p = style::BTN_SECONDARY;
                        play_plate(c, out, c.w - m - p / 2.0, m + s / 2.0, p, t.playing);
                        c.bar(m, 96.0, inner, style::METER_H_COMPACT, (pos / len.max(1.0)).clamp(0.0, 1.0), c.text1());
                        return;
                    }
                    let acy = m + s / 2.0;
                    if t.playing {
                        let kk = 1.2;
                        level_glyph(c, c.w - m - LEVEL_W * kk, acy + 6.0, true, kk);
                    } else if !t.idle {
                        c.text_base("Paused", c.w - m, acy + 4.0, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Right, inner - s);
                    }
                    let (by, bh) = (86.0, 20.0);
                    c.rr(m, by, inner, bh, bh / 2.0, alpha(c.plate(), if t.idle { 0.35 } else { 1.0 }));
                    let mid = c.w / 2.0;
                    controls(c, out, [mid - 32.0, mid, mid + 32.0], by + bh / 2.0, [10.0, 12.0, 10.0], t.playing, t.idle);
                }
            }
        }
        Shape::Medium => media_medium(c, out, k, x, wic, &t),
        Shape::Large => {
            let m = style::M_CARD;
            let r = style::concentric(style::CARD_R, 8.0);
            match k {
                MediaKind::Full => {
                    out.hits.clear();
                    let s = 96.0;
                    art_or_idle(c, x, wic, &t, m, m, s, r);
                    let tx = m + s + style::SP[5];
                    let t2 = if t.idle { Track { artist: "Play in any app".into(), ..t.clone() } } else { t.clone() };
                    large_track_text(c, &t2, tx, c.w - m - tx, 20.0, if t.idle { "" } else { &t.album }, 2);
                    // Where it plays, on the art's bottom edge: the column is anchored top
                    // and bottom, and the Large keeps the source the Medium shows.
                    if !t.idle {
                        c.text_base(&t.status_line(), tx, m + s, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, c.w - m - tx);
                    }
                    separator(c, 128.0);
                    transport_row(c, out, c.w / 2.0, 166.0, 56.0, &t);
                }
                MediaKind::Compact => {
                    // An art-led listening card: big cover, the track beside it, one button.
                    // 104 leaves a 120 px text column, wide enough for most two-word titles
                    // on one line.
                    let s = 104.0;
                    art_or_idle(c, x, wic, &t, m, m, s, r);
                    let tx = m + s + style::SP[5];
                    let t2 = if t.idle { Track { artist: "Play in any app".into(), ..t.clone() } } else { t.clone() };
                    large_track_text(c, &t2, tx, c.w - m - tx, 20.0, if t.idle { "" } else { &t.album }, 3);
                    let pd = style::BTN_PRIMARY;
                    let (pcx, pcy) = (c.w - m - pd / 2.0, c.h - m - pd / 2.0);
                    if !t.idle {
                        c.text_base(&t.status_line(), m, c.h - m, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, pcx - pd / 2.0 - style::SP[3] - m);
                    }
                    play_primary(c, out, pcx, pcy, pd, t.playing, if t.idle { 0.35 } else { 1.0 });
                }
                MediaKind::Disc => {
                    // Sleeve and record: the cover, with the record drawn out far enough that
                    // its whole label clears the sleeve (a part-hidden label read as a
                    // half-moon glitch at the sleeve edge). Its edge stays inside the margin.
                    let s = 120.0;
                    let d = 116.0;
                    let peek = 84.0;
                    if t.idle {
                        // No sleeve: the plate tile is translucent and the record would show
                        // through it. The bare record sits where the sleeve would.
                        record(c, x, wic, &t, m + s / 2.0, m + s / 2.0, s, false);
                    } else {
                        record(c, x, wic, &t, m + s + peek - d / 2.0, m + s / 2.0, d, false);
                        art(c, x, wic, m, m, s, r);
                    }
                    let pd = style::BTN_PRIMARY;
                    let (pcx, pcy) = (c.w - m - pd / 2.0, c.h - m - pd / 2.0);
                    let w = pcx - pd / 2.0 - style::SP[3] - m;
                    let (v, fs) = fit_value(c, &t.title, style::VALUE, w);
                    c.text_base(&v, m, 164.0, fs, style::VALUE.weight, c.text1(), Align::Left, w);
                    let artist = if t.idle { "Play something in any app" } else { &t.artist };
                    c.text_base(artist, m, 182.0, style::BODY.size, 400, c.text2(), Align::Left, w);
                    play_primary(c, out, pcx, pcy, pd, t.playing, if t.idle { 0.35 } else { 1.0 });
                }
                _ => {
                    // Bar: the Medium layout, then one more layer below the separator.
                    out.hits.clear();
                    media_medium(c, out, k, x, wic, &t);
                    separator(c, style::SEP_Y_LARGE);
                    if track_timeline(&t).is_none() {
                        // The Medium already holds the transport: the detail is the volume.
                        bar_volume(c, out, x);
                        return;
                    }
                    // With a timeline the Medium shows progress, so the transport comes here,
                    // over a footnote naming the album or where it plays.
                    let foot = if t.idle {
                        String::new()
                    } else if !t.album.is_empty() {
                        t.album.clone()
                    } else {
                        t.where_line()
                    };
                    let (_, y0, _, h) = detail_region(c);
                    let cy = if foot.is_empty() { y0 + h / 2.0 } else { 140.0 };
                    transport_row(c, out, c.w / 2.0, cy, 56.0, &t);
                    if !foot.is_empty() {
                        c.text_base(&foot, c.w / 2.0, 184.0, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Center, c.w - 2.0 * m);
                    }
                }
            }
        }
    }
}

/// The Medium media layouts (the Bar's is also the top of its Large).
fn media_medium(c: &Canvas, out: &mut Painted, k: MediaKind, x: &Ctx, wic: &IWICImagingFactory, t: &Track) {
    let m = style::M_CARD;
    let r = style::concentric(style::CARD_R, 8.0);
    let idle_t = || Track { artist: "Play something in any app".into(), ..t.clone() };
    match k {
        MediaKind::Full => {
            out.hits.clear();
            art_or_idle(c, x, wic, t, m, m, 64.0, r);
            let tx = 92.0;
            if t.idle {
                out.hits.push(((0.0, 0.0, c.w, c.h), WAct::MediaPlay));
                card_track_text(c, &idle_t(), tx, c.w - m - tx, 42.0, 60.0, style::CAPTION.size, None);
                return;
            }
            let mut right = c.w - m;
            if !t.source.is_empty() {
                let sw = c.measure(&t.source, style::FOOT.size, style::FOOT.weight, Font::Text).0.min(72.0);
                c.text_base(&t.source, c.w - m, style::HEADER_BASE, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Right, 72.0);
                right -= sw + style::SP[3];
            }
            let fs = fit_down(c, &t.title, style::TITLE.size, style::TITLE.min, style::TITLE.weight, right - tx);
            c.text_base(&t.title, tx, style::HEADER_BASE, fs, style::TITLE.weight, c.text1(), Align::Left, right - tx);
            c.text_base(&t.artist, tx, 47.0, style::CAPTION.size, 400, c.text2(), Align::Left, c.w - m - tx);
            controls(c, out, [104.0, 144.0, 184.0], 66.0, [16.0, 20.0, 16.0], t.playing, false);
        }
        MediaKind::Compact => {
            let s = 56.0;
            art_or_idle(c, x, wic, t, m, (c.h - s) / 2.0, s, r);
            let tx = m + s + style::SP[5];
            let pd = style::BTN_PRIMARY;
            let w = c.w - m - pd - style::SP[5] - tx;
            if t.idle {
                card_track_text(c, &idle_t(), tx, c.w - m - tx, 44.0, 60.0, style::CAPTION.size, None);
                return;
            }
            // Title, artist and a third line (album, app, or "Playing" / "Paused"), so the
            // Medium always adds a layer over the Small.
            card_track_text(c, t, tx, w, 36.0, 52.0, style::CAPTION.size, Some((68.0, t.third())));
            play_plate(c, out, c.w - m - pd / 2.0, c.h / 2.0, pd, t.playing);
        }
        MediaKind::Disc => {
            // The record at inset 12, so its curve keeps clear of the card's corner.
            let (d, i) = (72.0, 12.0);
            record(c, x, wic, t, i + d / 2.0, i + d / 2.0, d, true);
            let tx = i + d + style::SP[6];
            if t.idle {
                card_track_text(c, &idle_t(), tx, c.w - m - tx, 42.0, 60.0, style::CAPTION.size, None);
                return;
            }
            card_track_text(c, t, tx, c.w - m - tx, 30.0, 45.0, style::CAPTION.size, None);
            // Plates, like every other media Medium: play/pause, then next.
            let (p, cy) = (style::BTN_SECONDARY, 66.0);
            play_plate(c, out, tx + p / 2.0, cy, p, t.playing);
            let ncx = tx + p * 1.5 + style::SP[3];
            round_button(c, out, ncx, cy, p, c.plate(), G_NEXT_FILL, c.text1(), WAct::MediaNext);
        }
        _ => {
            let s = if track_timeline(t).is_some() { 40.0 } else { 32.0 };
            art_or_idle(c, x, wic, t, m, m, s, style::R_MIN + 2.0);
            let tx = m + s + style::SP[5];
            let pd = style::BTN_SECONDARY;
            let right = c.w - m - pd - style::SP[5];
            if t.idle {
                card_track_text(c, &idle_t(), tx, c.w - m - tx, style::HEADER_BASE, 47.0, style::CAPTION.size, None);
                if track_timeline(t).is_none() {
                    // The transport capsule stays, dimmed, so the bar keeps its shape.
                    let (by, bh) = (56.0, 24.0);
                    c.rr(m, by, c.w - 2.0 * m, bh, bh / 2.0, alpha(c.plate(), 0.35));
                    let mid = c.w / 2.0;
                    controls(c, out, [mid - 64.0, mid, mid + 64.0], by + bh / 2.0, [12.0, 14.0, 12.0], false, true);
                }
                return;
            }
            match track_timeline(t) {
                Some((pos, len)) => {
                    // A header row (art, title, artist, play), then the track's progress.
                    play_plate(c, out, c.w - m - pd / 2.0, 36.0, pd, t.playing);
                    card_track_text(c, t, tx, right - tx, style::HEADER_BASE, 47.0, style::CAPTION.size, None);
                    c.bar(m, 70.0, c.w - 2.0 * m, style::METER_H_COMPACT, (pos / len.max(1.0)).clamp(0.0, 1.0), c.text1());
                    c.text_base(&mmss(pos), m, 86.0, style::MICRO.size, style::MICRO.weight, c.text3(), Align::Left, 80.0);
                    c.text_base(&format!("\u{2212}{}", mmss(len - pos)), c.w - m, 86.0, style::MICRO.size, style::MICRO.weight, c.text3(), Align::Right, 80.0);
                }
                None => {
                    // A player bar: the track (with its live level) over a transport
                    // capsule. The app's name, when known, sits at the header's right.
                    let mut right = c.w - m;
                    let src = t.source.clone();
                    if !src.is_empty() {
                        let sw = c.measure(&src, style::FOOT.size, style::FOOT.weight, Font::Text).0.min(72.0);
                        c.text_base(&src, c.w - m, 30.0, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Right, 72.0);
                        right -= sw + style::SP[3];
                    }
                    let tw = right - tx - LEVEL_W - style::SP[2];
                    let fs = fit_down(c, &t.title, style::TITLE.size, style::TITLE.min, style::TITLE.weight, tw);
                    let w = c.text_base(&t.title, tx, 30.0, fs, style::TITLE.weight, c.text1(), Align::Left, tw);
                    level_glyph(c, tx + w + style::SP[2], 30.0, t.playing, 1.0);
                    c.text_base(&t.artist, tx, 45.0, style::CAPTION.size, 400, c.text2(), Align::Left, c.w - m - tx);
                    let (by, bh) = (56.0, 24.0);
                    c.rr(m, by, c.w - 2.0 * m, bh, bh / 2.0, c.plate());
                    let cy = by + bh / 2.0;
                    let mid = c.w / 2.0;
                    controls(c, out, [mid - 64.0, mid, mid + 64.0], cy, [12.0, 14.0, 12.0], t.playing, false);
                }
            }
        }
    }
}

/// Now Playing Bar, Large: the output volume as a slider in the detail region, under the
/// device name (or "Volume") and the level.
fn bar_volume(c: &Canvas, out: &mut Painted, x: &Ctx) {
    let (x0, y0, w, h) = detail_region(c);
    let vol = x.control.volume.unwrap_or(0.0).clamp(0.0, 1.0);
    let muted = x.control.muted == Some(true);
    let name = x.hub.output_name.as_deref().map(short_device).filter(|d| !d.is_empty()).unwrap_or_else(|| "Volume".into());
    let level = if muted { "Muted".to_string() } else { format!("{:.0}%", vol * 100.0) };
    // Label row and a 28 px capsule, centred as one block in the region.
    let bh = 28.0;
    let block = c.cap_h(style::FOOT.size) + style::SP[5] + bh;
    let top = y0 + (h - block) / 2.0;
    let base = top + c.cap_h(style::FOOT.size);
    let lw = c.measure(&level, style::FOOT.size, 500, Font::Text).0;
    c.text_base(&name, x0, base, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, w - lw - style::SP[3]);
    c.text_base(&level, x0 + w, base, style::FOOT.size, 500, if muted { RED } else { c.text2() }, Align::Right, w);
    let by = base + style::SP[5];
    c.rr(x0, by, w, bh, bh / 2.0, c.track());
    c.rr(x0, by, (w * if muted { 0.0 } else { vol }).max(bh), bh, bh / 2.0, if muted { c.text3() } else { BLUE });
    // The speaker glyph rides at the capsule's start, on the fill.
    let gcx = x0 + bh / 2.0;
    c.glyph(if muted { G_MUTE } else { G_VOL }, gcx, by + bh / 2.0, bh * style::GLYPH_IN, WHITE);
    out.hits.push(((x0, by - 4.0, w, bh + 8.0), WAct::Volume));
}

/// "Speakers (Realtek(R) Audio)" → "Speakers".
fn short_device(s: &str) -> String {
    s.split(" (").next().unwrap_or(s).trim().to_string()
}

fn output_face(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let vol = x.control.volume.unwrap_or(0.0).clamp(0.0, 1.0);
    let muted = x.control.muted == Some(true);
    let g = if muted { G_MUTE } else { G_VOL };
    let pct = format!("{:.0}%", vol * 100.0);
    let dev = x.hub.output_name.as_deref().map(short_device).filter(|d| !d.is_empty());
    let fill_col = if muted { c.text3() } else { BLUE };
    match sh {
        Shape::Mini => {
            out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Mute));
            let kk = c.h / 54.0;
            let d = 40.0 * kk;
            let t = style::METER_H_COMPACT * kk;
            c.ring(c.w / 2.0, c.h / 2.0, d / 2.0 - t / 2.0, t, if muted { 0.0 } else { vol }, BLUE);
            c.glyph(g, c.w / 2.0, c.h / 2.0, 16.0 * kk, c.text1());
        }
        Shape::Dock => {
            out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Mute));
            let kk = c.h / 54.0;
            let m = style::M_DOCK * kk;
            let p = style::BADGE_DOCK * kk;
            c.rr(m, (c.h - p) / 2.0, p, p, p * style::BADGE_R, if muted { mute_red(c) } else { c.plate() });
            c.glyph(g, m + p / 2.0, c.h / 2.0, p * style::GLYPH_IN, if muted { WHITE } else { c.text1() });
            let x0 = 52.0 * kk;
            let wide = c.w >= c.h * 3.4;
            let slot_w = (c.w * 0.34).round();
            let slot_x = c.w - m - slot_w;
            let right = if wide { slot_x - style::SP[3] * kk } else { c.w - m };
            let role = Type { size: style::VALUE.size * kk, min: style::VALUE.min * kk, ..style::VALUE };
            let (v, fs) = fit_value(c, &pct, role, right - x0);
            c.text_base(&v, x0, 30.0 * kk, fs, role.weight, c.text1(), Align::Left, c.w * 2.0);
            let (cap, ccol) = if muted { ("Muted", RED) } else { ("Volume", c.text2()) };
            c.text_base(cap, x0, 44.0 * kk, style::FOOT.size * kk, style::FOOT.weight, ccol, Align::Left, right - x0);
            if wide {
                let by = if dev.is_some() { 33.0 * kk } else { (c.h - style::METER_H_COMPACT * kk) / 2.0 };
                if let Some(d) = &dev {
                    c.text_base(d, c.w - m, 26.0 * kk, style::FOOT.size * kk, 500, c.text2(), Align::Right, slot_w);
                }
                c.bar(slot_x, by, slot_w, style::METER_H_COMPACT * kk, vol, fill_col);
                out.hits.push(((slot_x, by - 12.0 * kk, slot_w, 28.0 * kk), WAct::Volume));
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let p = style::BADGE_SMALL;
            c.rr(m, m, p, p, p * style::BADGE_R, if muted { mute_red(c) } else { c.plate() });
            c.glyph(g, m + p / 2.0, m + p / 2.0, p * style::GLYPH_IN, if muted { WHITE } else { c.text1() });
            out.hits.push(((m, m, p, p), WAct::Mute));
            let (v, fs) = fit_value(c, &pct, style::HERO_S, c.w - 2.0 * m);
            c.text_base(&v, m, 74.0, fs, style::HERO_S.weight, c.text1(), Align::Left, c.w * 2.0);
            let (cap, ccol) = if muted { ("Muted".to_string(), RED) } else { (dev.clone().unwrap_or_else(|| "Volume".into()), c.text2()) };
            c.text_base(&cap, m, 90.0, style::CAPTION.size, style::CAPTION.weight, ccol, Align::Left, c.w - 2.0 * m);
            let bw = c.w - 2.0 * m;
            c.bar(m, 100.0, bw, style::METER_H, vol, fill_col);
            out.hits.push(((m, 89.0, bw, 28.0), WAct::Volume));
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let p = style::BADGE_HEADER;
            c.rr(m, m, p, p, p * style::BADGE_R, if muted { mute_red(c) } else { c.plate() });
            c.glyph(g, m + p / 2.0, m + p / 2.0, p * style::GLYPH_IN, if muted { WHITE } else { c.text1() });
            out.hits.push(((m - 4.0, m - 4.0, p + 8.0, p + 8.0), WAct::Mute));
            let pw = c.measure(&pct, style::VALUE.size, style::VALUE.weight, Font::Text).0;
            c.text_base(&pct, c.w - m, style::HEADER_BASE, style::VALUE.size, style::VALUE.weight, c.text1(), Align::Right, c.w);
            let tx = m + p + style::SP[3];
            let name = dev.clone().unwrap_or_else(|| "Volume".into());
            c.text_base(&name, tx, style::HEADER_BASE, style::TITLE.size, style::TITLE.weight, c.text1(), Align::Left, c.w - m - pw - style::SP[3] - tx);
            // The slider: a 24 px capsule, filled to the volume.
            let (bx, by, bw, bh) = (m, 56.0, c.w - 2.0 * m, 24.0);
            c.rr(bx, by, bw, bh, bh / 2.0, c.track());
            c.rr(bx, by, (bw * vol).max(bh), bh, bh / 2.0, fill_col);
            out.hits.push(((bx, by - 4.0, bw, bh + 8.0), WAct::Volume));
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let (x0, y0, w, h) = detail_region(c);
                let tw = (w - style::SP[3]) / 2.0;
                // Mute toggle.
                let (tf, ink, sub) = if muted { (mute_red(c), WHITE, alpha(WHITE, 0.8)) } else { (c.plate(), c.text1(), c.text2()) };
                c.rr(x0, y0, tw, h, style::concentric(style::CARD_R, 8.0), tf);
                c.circle(x0 + 12.0 + 14.0, y0 + 12.0 + 14.0, 14.0, if muted { alpha(WHITE, 0.25) } else { c.plate() });
                c.glyph(g, x0 + 26.0, y0 + 26.0, 28.0 * style::GLYPH_IN, ink);
                c.text_base("Mute", x0 + 12.0, y0 + 54.0, style::TITLE.size, style::TITLE.weight, ink, Align::Left, tw - 24.0);
                c.text_base(if muted { "On" } else { "Off" }, x0 + 12.0, y0 + 69.0, style::FOOT.size, style::FOOT.weight, sub, Align::Left, tw - 24.0);
                out.hits.push(((x0, y0, tw, h), WAct::Mute));
                // Output device, opening Sound settings.
                let ox = x0 + tw + style::SP[3];
                let og = if name.to_lowercase().contains("headphone") || name.to_lowercase().contains("headset") || name.contains("AirPods") { G_HEADPHONES } else { G_SPEAKERS };
                c.rr(ox, y0, tw, h, style::concentric(style::CARD_R, 8.0), c.plate());
                c.circle(ox + 26.0, y0 + 26.0, 14.0, c.plate());
                c.glyph(og, ox + 26.0, y0 + 26.0, 28.0 * style::GLYPH_IN, c.text1());
                c.glyph(G_CHEV, ox + tw - 12.0 - 5.0, y0 + 26.0, 10.0, c.text3());
                c.text_base(&dev.clone().unwrap_or_else(|| "Output".into()), ox + 12.0, y0 + 54.0, style::TITLE.size, style::TITLE.weight, c.text1(), Align::Left, tw - 24.0);
                c.text_base("Sound settings", ox + 12.0, y0 + 69.0, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, tw - 24.0);
                out.hits.push(((ox, y0, tw, h), WAct::Open("ms-settings:sound".into())));
            }
        }
    }
}

fn mic_face(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Mic));
    let muted = x.hub.mic_muted == Some(true);
    let state = if muted { "Muted" } else { "On" };
    let (fill, g, ink) = if muted { (mute_red(c), G_MIC_OFF, WHITE) } else { (c.plate(), G_MIC, c.text1()) };
    let state_col = if muted { mute_red(c) } else { c.text2() };
    match sh {
        Shape::Mini => {
            let kk = c.h / 54.0;
            let p = 40.0 * kk;
            c.rr((c.w - p) / 2.0, (c.h - p) / 2.0, p, p, style::concentric(style::DOCK_R, style::M_MINI) * kk, fill);
            c.glyph(g, c.w / 2.0, c.h / 2.0, p * style::GLYPH_IN, ink);
        }
        Shape::Dock => {
            let kk = c.h / 54.0;
            let m = style::M_DOCK * kk;
            let p = style::BADGE_DOCK * kk;
            c.rr(m, (c.h - p) / 2.0, p, p, p * style::BADGE_R, fill);
            c.glyph(g, m + p / 2.0, c.h / 2.0, p * style::GLYPH_IN, ink);
            let x0 = 52.0 * kk;
            let wide = c.w >= c.h * 3.4;
            let label = if muted { "Unmute" } else { "Mute" };
            let fs = style::FOOT.size * kk;
            let pw = c.measure(label, fs, 600, Font::Text).0 + 2.0 * style::SP[5] * kk;
            let right = if wide { c.w - m - pw - style::SP[3] * kk } else { c.w - m };
            c.text_base(state, x0, 30.0 * kk, style::VALUE.size * kk, style::VALUE.weight, c.text1(), Align::Left, right - x0);
            c.text_base("Microphone", x0, 44.0 * kk, fs, style::FOOT.weight, c.text2(), Align::Left, right - x0);
            if wide {
                let ph = style::BTN_SECONDARY_DOCK * kk;
                let (px, py) = (c.w - m - pw, (c.h - ph) / 2.0);
                c.rr(px, py, pw, ph, ph / 2.0, c.plate());
                c.text_cap(label, px + pw / 2.0, py + (ph - c.cap_h(fs)) / 2.0, fs, 600, c.text1(), Align::Center, pw);
                out.hits.push(((px, py.min((c.h - 28.0 * kk) / 2.0), pw, ph.max(28.0 * kk)), WAct::Mic));
            }
        }
        Shape::Small => {
            let p = 44.0;
            c.rr((c.w - p) / 2.0, 22.0, p, p, p * style::BADGE_R, fill);
            c.glyph(g, c.w / 2.0, 22.0 + p / 2.0, p * style::GLYPH_IN, ink);
            c.text_base("Microphone", c.w / 2.0, 88.0, style::TITLE.size, style::TITLE.weight, c.text1(), Align::Center, c.w - 2.0 * style::M_SMALL);
            c.text_base(state, c.w / 2.0, 104.0, style::FOOT.size, style::FOOT.weight, state_col, Align::Center, c.w - 2.0 * style::M_SMALL);
        }
        Shape::Medium => {
            let m = style::M_CARD;
            let p = 44.0;
            c.rr(m, 26.0, p, p, p * style::BADGE_R, fill);
            c.glyph(g, m + p / 2.0, 26.0 + p / 2.0, p * style::GLYPH_IN, ink);
            let tx = 76.0;
            // Label, state and what that means for apps: three lines centred on the plate.
            c.text_base("Microphone", tx, 40.0, style::VALUE.size, style::VALUE.weight, c.text1(), Align::Left, c.w - m - tx);
            c.text_base(state, tx, 57.0, style::CAPTION.size, style::CAPTION.weight, state_col, Align::Left, c.w - m - tx);
            let note = if muted { "Apps can't hear you" } else { "Apps can use it" };
            c.text_base(note, tx, 73.0, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, c.w - m - tx);
        }
        Shape::Large => {
            // A state tile, then two facts.
            let m = style::M_CARD;
            let i = 6.0;
            let (tw, th) = (c.w - 2.0 * i, 112.0);
            c.rr(i, i, tw, th, style::concentric(style::CARD_R, i), c.plate());
            let d = 56.0;
            let (cx, cy) = (i + style::SP[6] + d / 2.0, i + th / 2.0);
            c.circle(cx, cy, d / 2.0, if muted { mute_red(c) } else { c.plate() });
            c.glyph(g, cx, cy, d * style::GLYPH_IN, ink);
            let tx = cx + d / 2.0 + style::SP[6];
            c.text_base("Microphone", tx, cy - 4.0, style::VALUE.size, style::VALUE.weight, c.text1(), Align::Left, c.w - i - style::SP[6] - tx);
            c.text_base(state, tx, cy + 16.0, style::BODY.size, 400, state_col, Align::Left, c.w - i - style::SP[6] - tx);
            // Real facts: what muting does, and what a click will do now. (The device
            // name would go here once the hub reports the default capture endpoint.)
            let facts = if muted { ["Apps can't hear you", "Click to turn it back on"] } else { ["Apps can use your microphone", "Click to mute it in every app"] };
            for (k, fact) in facts.iter().enumerate() {
                c.text_base(fact, m, 146.0 + k as f32 * style::ROW_L, style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, c.w - 2.0 * m);
            }
        }
    }
}

// ------------------------------------------------------------------ system
// The System stats family: resource-monitor, cpu, memory, network, battery, disk,
// storage, language, uptime and brightness. Every face is built from the same parts: an
// identity mark (a ring for a metric, a glyph plate for the rest), one key value and a
// caption, then one more layer per size. Dock L adds its accessory slot, Small its slot,
// Medium the Glance block (or a header and chart), and Large a detail region under it.

const G_DOWNLOAD: &str = "\u{E896}";
const G_BATTERY: &str = "\u{E83F}";
const G_BOLT: &str = "\u{E945}";

/// Width of the Glance accessory box (x 148 to 252 on a Medium card, §3 A).
const SYS_ACC_W: f32 = 104.0;
/// History kept for charts: a minute at one sample a second.
const SYS_HIST: usize = 60;
/// Day letters for the uptime strip (Sunday first, as `Now::weekday`).
const SYS_DAY_LETTERS: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];
const SYS_DAY_SHORT: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// "2.4 MB/s", "310 KB/s".
fn rate(bps: f64) -> String {
    if bps >= 1_000_000.0 {
        format!("{:.1} MB/s", bps / 1_000_000.0)
    } else {
        format!("{:.0} KB/s", bps / 1000.0)
    }
}

/// The Mini form of a rate: "2.4M", "310K".
fn rate_short(bps: f64) -> String {
    rate(bps).replace(" KB/s", "K").replace(" MB/s", "M")
}

/// A load percentage (§2.2): one decimal under 10, whole numbers otherwise.
fn sys_pct(v: f32) -> String {
    if v > 0.05 && v < 9.95 {
        format!("{v:.1}%")
    } else {
        format!("{v:.0}%")
    }
}

/// Battery colour: green, yellow at 20% or less, red at 10% or less; green while charging.
fn sys_battery_col(pct: f32, charging: bool) -> Col {
    if charging || pct > 20.0 {
        GREEN
    } else if pct > 10.0 {
        YELLOW
    } else {
        RED
    }
}

/// Disk colour: orange, red once 90% or more is used.
fn sys_disk_col(used: f32) -> Col {
    if used >= 0.9 {
        RED
    } else {
        ORANGE
    }
}

/// GB in a size as the storage connector writes it: "240 GB", "1.8 TB", "640 MB".
fn sys_gb(s: &str) -> Option<f64> {
    let mut it = s.split_whitespace();
    let n: f64 = it.next()?.parse().ok()?;
    Some(match it.next()? {
        "TB" => n * 1024.0,
        "GB" => n,
        "MB" => n / 1024.0,
        "KB" => n / (1024.0 * 1024.0),
        _ => return None,
    })
}

/// (free, total) GB from a storage row: "240 GB free of 931 GB".
fn sys_free_total(v: &str) -> Option<(f64, f64)> {
    let (a, b) = v.split_once(" free of ")?;
    Some((sys_gb(a)?, sys_gb(b)?))
}

/// One formatter for every drive size, so the hero, captions and rows agree:
/// "640 MB", "9.5 GB", "241 GB", "1.8 TB".
fn sys_fmt_gb(gb: f64) -> String {
    if gb < 1.0 {
        format!("{:.0} MB", gb * 1024.0)
    } else if gb < 10.0 {
        format!("{gb:.1} GB")
    } else if gb < 1023.5 {
        format!("{gb:.0} GB")
    } else {
        format!("{:.1} TB", gb / 1024.0).replace(".0 ", " ")
    }
}

/// The Mini form (5 glyphs): "241G", "1.2T", "9.5G", "640M".
fn sys_fmt_gb_short(gb: f64) -> String {
    sys_fmt_gb(gb).replace(" MB", "M").replace(" GB", "G").replace(" TB", "T")
}

/// Made-up history for previews (the gallery and the widget sheet) before any real samples
/// exist: a calm, drifting walk that ends at the current value.
fn sys_demo_hist(seed: u32, now: f64, vol: f64, lo: f64, hi: f64) -> Vec<f64> {
    let mut x = 0x9E37_79B9u32 ^ seed.wrapping_mul(2_654_435_761);
    let (mut v, mut vel) = (now, 0.0);
    let mut out = Vec::with_capacity(SYS_HIST);
    for _ in 0..SYS_HIST {
        out.push(v);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let r = (x % 10_000) as f64 / 10_000.0 - 0.5;
        // Momentum keeps it smooth; a pull back to `now` keeps it near today's level.
        vel = vel * 0.75 + r * vol * 0.6 + (now - v) * 0.04;
        v = (v + vel).clamp(lo, hi);
    }
    out.reverse();
    out
}

/// What identifies a system face.
#[derive(Clone, Copy)]
enum Mark {
    /// A progress ring: fraction and colour.
    Ring(f32, Col),
    /// Resource monitor: CPU (outer ring, blue) and memory (inner ring, purple).
    Dual(f32, f32),
    /// A Fluent glyph on a plate, in its colour.
    Plate(&'static str, Col),
}

fn sys_ring_t(d: f32) -> f32 {
    (d * style::RING_T).max(3.0)
}

/// Draw a mark in the `d`-sized square at (x, y).
fn sys_mark(c: &Canvas, mark: Mark, x: f32, y: f32, d: f32) {
    let (cx, cy) = (x + d / 2.0, y + d / 2.0);
    match mark {
        Mark::Ring(f, col) => {
            let t = sys_ring_t(d);
            c.ring(cx, cy, (d - t) / 2.0, t, f, col);
        }
        Mark::Dual(a, b) => {
            let t = sys_ring_t(d);
            c.ring(cx, cy, (d - t) / 2.0, t, a, BLUE);
            // The inner ring sits 2 px inside the outer one (at the Mini's 38 px; scaled).
            let d2 = d - 2.0 * (t + style::SP[0] * d / style::BADGE_MINI_VIS);
            c.ring(cx, cy, (d2 - t) / 2.0, t, b, PURPLE);
        }
        Mark::Plate(g, col) => {
            c.rr(x, y, d, d, d * style::BADGE_R, c.plate());
            c.glyph(g, cx, cy, d * 0.57, col);
        }
    }
}

/// A line that starts with an arrow in colour, the rest in `tcol` ("↑ 310 KB/s").
/// Returns its width.
#[allow(clippy::too_many_arguments)]
fn sys_arrow_text(c: &Canvas, arrow: &str, acol: Col, text: &str, x: f32, base: f32, size: f32, weight: u32, tcol: Col, align: Align, max_w: f32) -> f32 {
    let aw = c.measure(arrow, size, weight, Font::Text).0;
    let gap = style::SP[1] * size / 13.0;
    let tw = c.measure(text, size, weight, Font::Text).0.min(max_w - aw - gap);
    let left = match align {
        Align::Left => x,
        Align::Center => x - (aw + gap + tw) / 2.0,
        Align::Right => x - aw - gap - tw,
    };
    c.text_base(arrow, left, base, size, weight, acol, Align::Left, aw + 4.0);
    c.text_base(text, left + aw + gap, base, size, weight, tcol, Align::Left, max_w - aw - gap);
    aw + gap + tw
}

/// A secondary pill (plate fill, text1 label).
fn sys_pill(c: &Canvas, x: f32, y: f32, w: f32, h: f32, label: &str) {
    c.rr(x, y, w, h, h / 2.0, c.plate());
    let fs = style::CAPTION.size;
    c.text_cap(label, x + w / 2.0, y + (h - c.cap_h(fs)) / 2.0, fs, 600, c.text1(), Align::Center, w);
}

/// Keycaps side by side ("Win", "Space"): plate chips with a hairline edge. `x` is the
/// left edge, centre or right edge of the row per `align`. Each key is at least its
/// minimum width.
#[allow(clippy::too_many_arguments)]
fn sys_keycaps(c: &Canvas, keys: &[(&str, f32)], x: f32, y: f32, h: f32, fs: f32, weight: u32, col: Col, r: f32, gap: f32, align: Align) {
    let pad = h * 0.4;
    let ws: Vec<f32> = keys.iter().map(|(k, min)| (c.measure(k, fs, weight, Font::Text).0 + 2.0 * pad).max(*min)).collect();
    let total = ws.iter().sum::<f32>() + gap * (ws.len().saturating_sub(1)) as f32;
    let mut left = match align {
        Align::Left => x,
        Align::Center => x - total / 2.0,
        Align::Right => x - total,
    };
    for ((k, _), w) in keys.iter().zip(ws) {
        c.rr(left, y, w, h, r, c.plate());
        c.rr_stroke(left, y, w, h, r, c.sep(), 1.0);
        c.text_cap(k, left + w / 2.0, y + (h - c.cap_h(fs)) / 2.0, fs, weight, col, Align::Center, w);
        left += w + gap;
    }
}

// ---- Mini

/// Visual-led Mini: a 38 px mark, with a number (or a glyph) inside a ring.
fn sys_mini_ring(c: &Canvas, mark: Mark, text: &str, glyph: Option<&str>) {
    let k = c.h / 54.0;
    let d = style::BADGE_MINI_VIS * k;
    sys_mark(c, mark, (c.w - d) / 2.0, (c.h - d) / 2.0, d);
    let (cx, cy) = (c.w / 2.0, c.h / 2.0);
    if let Some(g) = glyph {
        c.glyph(g, cx, cy, 12.0 * k, c.text1());
    } else if !text.is_empty() {
        let inner = d - 2.0 * sys_ring_t(d) - style::SP[0] * k;
        let fs = fit(c, text, style::FOOT.size * k, style::MICRO.size * k, 600, Font::Text, inner);
        c.text_cap(text, cx, cy - c.cap_h(fs) / 2.0, fs, 600, c.text1(), Align::Center, d);
    }
}

/// Value-led Mini: a 20 px glyph over one compact value (§3 Mini).
fn sys_mini_value(c: &Canvas, glyph: &str, gcol: Col, value: &str) {
    let k = c.h / 54.0;
    let g = style::BADGE_MINI * k;
    c.glyph(glyph, c.w / 2.0, 8.0 * k + g / 2.0, g, gcol);
    let role = Type { size: style::MINI_VALUE.size * k, min: style::MINI_VALUE.min * k, ..style::MINI_VALUE };
    let (v, fs) = fit_value(c, value, role, c.w - 2.0 * style::M_MINI * k);
    c.text_base(&v, c.w / 2.0, 45.0 * k, fs, role.weight, c.text1(), Align::Center, c.w * 2.0);
}

// ---- Dock M / Dock L

/// What a Dock L pill shows in its accessory slot (x = w − 12 − 34%, y 14 to 40).
enum SysSlot {
    None,
    Spark(Vec<f64>, Col),
    /// Download and upload on one scale.
    Net(Vec<f64>, Vec<f64>),
    /// A 4 px meter, with a footnote over it when the label isn't empty.
    Meter(String, f32, Col),
    /// A second value: footnote (text2) over meta (text3).
    Two(String, String),
    /// The Win + Space keycaps.
    Keys,
}

fn sys_dock_l(c: &Canvas) -> bool {
    c.w >= c.h * 3.4
}

/// Dock M: mark 30 at (12, 12), value 17/600 at baseline 30, caption 11 text2 at 44 (with a
/// meter: 23 / 36 and a 4 px bar at 41). Dock L adds `slot` at the right.
#[allow(clippy::too_many_arguments)]
fn sys_dock(c: &Canvas, mark: Mark, value: &str, cap: &str, arrow: Option<(&str, Col)>, meter: Option<(f32, Col)>, slot: &SysSlot) {
    let k = c.h / 54.0;
    let m = style::M_DOCK * k;
    let bs = style::BADGE_DOCK * k;
    sys_mark(c, mark, m, (c.h - bs) / 2.0, bs);
    let x0 = m + bs + style::SP[4] * k;
    let wide = sys_dock_l(c) && !matches!(slot, SysSlot::None);
    let slot_w = (c.w * 0.34).round();
    let slot_x = c.w - m - slot_w;
    let right = if wide { slot_x - style::SP[4] * k } else { c.w - m };
    let (vb, cb) = if meter.is_some() { (24.0 * k, 36.0 * k) } else { (30.0 * k, 44.0 * k) };
    let role = Type { size: style::VALUE.size * k, min: style::VALUE.min * k, ..style::VALUE };
    let (v, fs) = fit_value(c, value, role, right - x0);
    c.text_base(&v, x0, vb, fs, role.weight, c.text1(), Align::Left, c.w * 2.0);
    let cs = style::FOOT.size * k;
    match arrow {
        Some((a, acol)) => {
            sys_arrow_text(c, a, acol, cap, x0, cb, cs, 400, c.text2(), Align::Left, right - x0);
        }
        None => {
            c.text_base(cap, x0, cb, cs, 400, c.text2(), Align::Left, right - x0);
        }
    }
    if let Some((f, col)) = meter {
        c.bar(x0, 42.0 * k, right - x0, style::METER_H_COMPACT * k, f, col);
    }
    if !wide {
        return;
    }
    let (y0, y1, sr) = (14.0 * k, 40.0 * k, c.w - m);
    match slot {
        SysSlot::None => {}
        SysSlot::Spark(v, col) => c.area(slot_x, y0, slot_w, y1 - y0, v, *col, true),
        SysSlot::Net(d, u) => {
            sys_net_chart(c, slot_x, y0, slot_w, y1 - y0, d, u, style::STROKE_SPARK, false);
        }
        SysSlot::Meter(label, f, col) => {
            let mh = style::METER_H_COMPACT * k;
            if label.is_empty() {
                c.bar(slot_x, (y0 + y1 - mh) / 2.0, slot_w, mh, *f, *col);
            } else {
                c.text_base(label, sr, 24.0 * k, cs, 500, c.text2(), Align::Right, slot_w);
                c.bar(slot_x, 30.0 * k, slot_w, mh, *f, *col);
            }
        }
        SysSlot::Two(a, b) => {
            c.text_base(a, sr, 24.0 * k, cs, 500, c.text2(), Align::Right, slot_w);
            c.text_base(b, sr, 38.0 * k, cs, 400, c.text3(), Align::Right, slot_w);
        }
        SysSlot::Keys => {
            let h = 18.0 * k;
            sys_keycaps(c, &[("Win", 0.0), ("Space", 0.0)], sr, (y0 + y1 - h) / 2.0, h, style::MICRO.size * k, style::MICRO.weight, c.text2(), 4.0 * k, style::SP[1] * k, Align::Right);
        }
    }
}

// ---- Small

/// What a Small card shows in its top-right slot (x 52 to 114, y 16 to 42).
enum SmallSlot {
    None,
    Spark(Vec<f64>, Col),
    /// A 4 px meter at y 27, with an optional Micro label above it.
    Meter(f32, Col, String),
    /// A footnote, right-aligned at baseline 26.
    Foot(String, Col),
    /// A glyph and an optional word (the charging chip).
    Chip(&'static str, String),
}

/// Small, bottom-anchored (§3): mark 28 at (14, 14), the slot, Hero S at baseline 86 (with
/// an optional unit in 17/600 text2), caption 12 text2 at baseline 106.
#[allow(clippy::too_many_arguments)]
fn sys_small(c: &Canvas, mark: Mark, slot: &SmallSlot, hero: &str, unit: Option<&str>, cap: &str, arrow: Option<(&str, Col)>) {
    let m = style::M_SMALL;
    let bs = style::BADGE_SMALL;
    sys_mark(c, mark, m, m, bs);
    let (sx0, sx1) = (m + bs + style::SP[4], c.w - m);
    let sw = sx1 - sx0;
    match slot {
        SmallSlot::None => {}
        SmallSlot::Spark(v, col) => c.area(sx0, 16.0, sw, 26.0, v, *col, true),
        SmallSlot::Meter(f, col, label) => {
            if !label.is_empty() {
                c.text_base(label, sx1, 22.0, style::MICRO.size, style::MICRO.weight, c.text3(), Align::Right, sw);
            }
            c.bar(sx0, 27.0, sw, style::METER_H_COMPACT, *f, *col);
        }
        SmallSlot::Foot(t, col) => {
            let fs = fit(c, t, style::FOOT.size, style::MICRO.size, 500, Font::Text, sw);
            c.text_base(t, sx1, 26.0, fs, 500, *col, Align::Right, sw + 8.0);
        }
        SmallSlot::Chip(g, t) => {
            let fs = style::FOOT.size;
            let tw = if t.is_empty() { 0.0 } else { c.text_base(t, sx1, 26.0, fs, 600, c.text2(), Align::Right, sw) + style::SP[1] };
            c.glyph(g, sx1 - tw - fs / 2.0, 26.0 - c.cap_h(fs) / 2.0, fs, c.text2());
        }
    }
    let us = style::VALUE.size;
    let uw = unit.map(|u| c.measure(u, us, 600, Font::Text).0 + 3.0).unwrap_or(0.0);
    let (v, fs) = fit_value(c, hero, style::HERO_S, c.w - 2.0 * m - uw);
    let hw = c.text_base(&v, m, 86.0, fs, style::HERO_S.weight, c.text1(), Align::Left, c.w * 2.0);
    if let Some(u) = unit {
        c.text_base(u, m + hw + 3.0, 86.0, us, 600, c.text2(), Align::Left, c.w);
    }
    let (cs, cw) = (style::CAPTION.size, c.w - 2.0 * m);
    match arrow {
        Some((a, acol)) => {
            sys_arrow_text(c, a, acol, cap, m, 106.0, cs, 400, c.text2(), Align::Left, cw);
        }
        None => {
            c.text_base(cap, m, 106.0, cs, 400, c.text2(), Align::Left, cw);
        }
    }
}

// ---- Medium (Glance) and Large

/// What the Glance accessory box (x 148 to 252, y 48 to 80) holds.
enum SysAcc {
    None,
    Spark(Vec<f64>, Col),
    /// A 6 px meter at y 70.
    Meter(f32, Col),
}

struct Glance<'a> {
    mark: Mark,
    hero: &'a str,
    /// A small glyph after the hero (the charging bolt).
    hero_glyph: Option<&'static str>,
    cap: &'a str,
    /// Third line (11) at baseline 76, in its colour.
    third: Option<(&'a str, Col)>,
    /// A 6 px meter at y 70 from the left margin to this x.
    meter: Option<(f32, Col, f32)>,
    acc: &'a SysAcc,
}

/// Glance (§3 A): hero 32/600 at cap-top 16, caption at baseline 58, a third line or meter,
/// the mark 24 at the top right. Large reuses it unchanged.
fn sys_glance(c: &Canvas, g: &Glance) {
    let m = style::M_CARD;
    let bs = style::BADGE_GLANCE;
    sys_mark(c, g.mark, c.w - m - bs, m, bs);
    let (v, fs) = fit_value(c, g.hero, style::HERO, c.w - 2.0 * m - bs - style::SP[3]);
    let hw = c.text_cap(&v, m, m, fs, style::HERO.weight, c.text1(), Align::Left, c.w * 2.0);
    if let Some(gl) = g.hero_glyph {
        let gs = style::CAPTION.size;
        c.glyph(gl, m + hw + 3.0 + gs / 2.0, m + c.cap_h(fs) / 2.0, gs, c.text2());
    }
    let ax = c.w - m - SYS_ACC_W;
    let tw = if matches!(g.acc, SysAcc::None) { c.w - 2.0 * m } else { ax - m - style::SP[3] };
    c.text_base(g.cap, m, 58.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, tw);
    if let Some((t, col)) = g.third {
        c.text_base(t, m, 76.0, style::FOOT.size, style::FOOT.weight, col, Align::Left, tw);
    }
    if let Some((f, col, x1)) = g.meter {
        c.bar(m, 70.0, x1 - m, style::METER_H, f, col);
    }
    match g.acc {
        SysAcc::None => {}
        SysAcc::Spark(v, col) => c.area(ax, 48.0, SYS_ACC_W, 32.0, v, *col, true),
        SysAcc::Meter(f, col) => c.bar(ax, 70.0, SYS_ACC_W, style::METER_H, *f, *col),
    }
}

/// Header row (§3 B): glyph plate 20 at (16, 16) and the title, leaving `trailing_w` at the right.
fn sys_header(c: &Canvas, glyph: &'static str, gcol: Col, title: &str, trailing_w: f32) {
    let m = style::M_CARD;
    let bs = style::BADGE_HEADER;
    sys_mark(c, Mark::Plate(glyph, gcol), m, m, bs);
    let tx = m + bs + style::SP[3];
    c.text_base(title, tx, style::HEADER_BASE, style::TITLE.size, style::TITLE.weight, c.text1(), Align::Left, c.w - m - trailing_w - style::SP[3] - tx);
}

/// What a Large card shows under the separator.
enum SysDetail {
    /// A history chart (y 108 to 174) with "1 min ago" / "Now" under it, and the current
    /// level (0..1) for the first seconds before there's any history.
    Chart(Vec<f64>, Col, f32),
    /// Three stats: (value, label).
    Stats([(String, String); 3]),
}

fn sys_detail(c: &Canvas, d: &SysDetail) {
    match d {
        SysDetail::Chart(v, col, level) => {
            if sys_history(c, v, *col, style::DETAIL_TOP, c.h - style::M_CARD - 14.0, *level) {
                sys_time_labels(c);
            } else {
                sys_collecting(c);
            }
        }
        SysDetail::Stats(s) => {
            let items = [(s[0].0.as_str(), s[0].1.as_str()), (s[1].0.as_str(), s[1].1.as_str()), (s[2].0.as_str(), s[2].1.as_str())];
            sys_stat_footer(c, items);
        }
    }
}

/// Three-stat footer (§3 Large): values 17/600 at baseline 150 over 11 text2 labels at
/// 166, in three equal columns. All three values share one size, so a long word in one
/// column shrinks the row evenly instead of standing out.
fn sys_stat_footer(c: &Canvas, items: [(&str, &str); 3]) {
    let m = style::M_CARD;
    let cw = (c.w - 2.0 * m) / 3.0;
    let r = style::VALUE;
    let fs = items.iter().map(|(v, _)| fit(c, v, r.size, r.min, r.weight, Font::Text, cw - style::SP[3])).fold(r.size, f32::min);
    let base = 150.0;
    for (i, (v, label)) in items.iter().enumerate() {
        let x = m + i as f32 * cw;
        c.text_base(v, x, base, fs, r.weight, c.text1(), Align::Left, c.w);
        c.text_base(label, x, base + style::SP[6], style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, cw - style::SP[3]);
    }
}

/// A full-width history chart from y0 to y1. Until there are two samples it draws the chart
/// starting: a track line at the current `level` (0..1) ending in the accent dot. Returns
/// whether a real chart was drawn.
fn sys_history(c: &Canvas, vals: &[f64], col: Col, y0: f32, y1: f32, level: f32) -> bool {
    let (x, w) = (style::M_CARD, c.w - 2.0 * style::M_CARD);
    if vals.len() < 2 {
        let pad = style::CHART_DOT + 1.5;
        let y = y1 - pad - level.clamp(0.0, 1.0) * (y1 - y0 - 2.0 * pad);
        let dx = x + w - pad;
        c.line(x + 1.0, y, dx, y, style::STROKE, c.track());
        c.circle(dx, y, style::CHART_DOT + 1.5, c.halo());
        c.circle(dx, y, style::CHART_DOT, col);
        return false;
    }
    c.chart(x, y0, w, y1 - y0, vals, col, ChartOpts { fill: true, dot: true, stroke: style::STROKE, smooth: vals.len() >= 8 });
    true
}

/// Under a history chart that hasn't started yet: what's happening, instead of time labels.
fn sys_collecting(c: &Canvas) {
    let r = style::MICRO;
    c.text_base("Charting the last minute", style::M_CARD, c.h - style::M_CARD, r.size, r.weight, c.text3(), Align::Left, c.w - 2.0 * style::M_CARD);
}

/// "1 min ago" and "Now" under a Large history chart, on the bottom margin.
fn sys_time_labels(c: &Canvas) {
    let base = c.h - style::M_CARD;
    let r = style::MICRO;
    c.text_base("1 min ago", style::M_CARD, base, r.size, r.weight, c.text3(), Align::Left, c.w / 2.0);
    c.text_base("Now", c.w - style::M_CARD, base, r.size, r.weight, c.text3(), Align::Right, c.w / 2.0);
}

/// Download (blue area) and upload (green line) on one scale from zero, so their heights
/// compare and the download fill reaches the zero line. The zero line sits a little above
/// the box bottom (§2.5 floor), so a quiet upload line never lies on the edge; the
/// baseline hairline shows only while there's nothing to draw.
#[allow(clippy::too_many_arguments)]
fn sys_net_chart(c: &Canvas, x: f32, y: f32, w: f32, h: f32, down: &[f64], up: &[f64], stroke: f32, dot: bool) -> bool {
    let peak = down.iter().chain(up.iter()).copied().filter(|v| v.is_finite()).fold(0.0f64, f64::max);
    if peak <= 0.0 || (down.len() < 2 && up.len() < 2) {
        if dot {
            // Before any history: the chart starting, a track line ending in the download dot.
            let r = style::CHART_DOT;
            let (cy, dx) = (y + h - r - 1.5, x + w - r - 1.5);
            c.line(x + 1.0, cy, dx, cy, stroke, c.track());
            c.circle(dx, cy, r + 1.5, c.halo());
            c.circle(dx, cy, r, BLUE);
        } else {
            c.line(x, y + h - 0.5, x + w, y + h - 0.5, 1.0, c.sep());
        }
        return false;
    }
    let top_v = peak * (1.0 + style::CHART_PAD_HI);
    let floor = (h * style::CHART_PAD_LO as f32).max(style::SP[0]);
    for (vals, col, fill, st) in [(down, BLUE, true, stroke), (up, GREEN, false, style::STROKE_SPARK)] {
        if vals.len() < 2 {
            continue;
        }
        c.chart_in(x, y, w, h - floor, vals, col, ChartOpts { fill, dot, stroke: st, smooth: vals.len() >= 8 }, Some((0.0, top_v)));
    }
    true
}

// ---- Placeholder states

/// Loading: the face's own layout with its mark and track bars (§7), never the connector
/// placeholder. `text_icon`: the Mini is a word ("ENG"); `meter`: the face has a meter.
fn sys_skeleton(c: &Canvas, sh: Shape, mark: Mark, text_icon: bool, meter: bool) {
    let ring = matches!(mark, Mark::Ring(..));
    match sh {
        Shape::Mini => {
            let k = c.h / 54.0;
            if text_icon {
                skeleton(c, (c.w - 30.0 * k) / 2.0, (c.h - 12.0 * k) / 2.0, 30.0 * k, 12.0 * k);
            } else if ring {
                let d = style::BADGE_MINI_VIS * k;
                sys_mark(c, mark, (c.w - d) / 2.0, (c.h - d) / 2.0, d);
            } else {
                if let Mark::Plate(g, col) = mark {
                    let gs = style::BADGE_MINI * k;
                    c.glyph(g, c.w / 2.0, 8.0 * k + gs / 2.0, gs, col);
                }
                skeleton(c, (c.w - 28.0 * k) / 2.0, 35.0 * k, 28.0 * k, 10.0 * k);
            }
        }
        Shape::Dock => {
            let k = c.h / 54.0;
            let m = style::M_DOCK * k;
            let bs = style::BADGE_DOCK * k;
            sys_mark(c, mark, m, (c.h - bs) / 2.0, bs);
            let x0 = m + bs + style::SP[4] * k;
            let (vb, cb) = if meter { (24.0 * k, 36.0 * k) } else { (30.0 * k, 44.0 * k) };
            skeleton(c, x0, vb - 10.0 * k, (60.0 * k).min(c.w - m - x0), 10.0 * k);
            skeleton(c, x0, cb - 8.0 * k, 40.0 * k, 8.0 * k);
            if meter {
                c.bar(x0, 42.0 * k, c.w - m - x0, style::METER_H_COMPACT * k, 0.0, ORANGE);
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            sys_mark(c, mark, m, m, style::BADGE_SMALL);
            if meter {
                let sx0 = m + style::BADGE_SMALL + style::SP[4];
                c.bar(sx0, 27.0, c.w - m - sx0, style::METER_H_COMPACT, 0.0, ORANGE);
            }
            let ch = c.cap_h(style::HERO_S.size);
            skeleton(c, m, 86.0 - ch, (c.w - 2.0 * m) * 0.6, ch);
            skeleton(c, m, 106.0 - 8.0, 40.0, 8.0);
        }
        _ => {
            let m = style::M_CARD;
            let bs = style::BADGE_GLANCE;
            sys_mark(c, mark, c.w - m - bs, m, bs);
            skeleton(c, m, m, (c.w - 2.0 * m - bs - style::SP[3]) * 0.6, c.cap_h(style::HERO.size));
            skeleton(c, m, 58.0 - 8.0, 40.0, 8.0);
            if meter {
                c.bar(m, 70.0, c.w - 2.0 * m, style::METER_H, 0.0, ORANGE);
            }
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let (x, y, w, _) = detail_region(c);
                for (i, f) in [1.0, 0.75, 0.5].iter().enumerate() {
                    skeleton(c, x, y + 4.0 + i as f32 * style::ROW_L, w * f, 8.0);
                }
            }
        }
    }
}

/// A face with nothing to show on this PC (no battery, a display without DDC/CI) or a
/// failed read: the §7 layout with no Connect action.
struct Unsup<'a> {
    glyph: &'static str,
    /// The widget's name ("Brightness").
    name: &'a str,
    /// The short state for Dock and Small ("Not supported").
    short: &'a str,
    /// Small's second line ("On this display").
    sub: &'a str,
    /// The full reason for Medium and Large ("Not supported on this display").
    reason: &'a str,
    /// A failure (the reason gets the warning glyph) rather than a fact about this PC.
    err: bool,
    /// A secondary pill naming where a click goes ("Display settings"); empty for none.
    pill: &'a str,
}

fn sys_unsupported(c: &Canvas, out: &mut Painted, sh: Shape, u: &Unsup, open: &str) {
    open_hit(out, c, open);
    match sh {
        Shape::Mini => {
            let k = c.h / 54.0;
            c.glyph(u.glyph, c.w / 2.0, c.h / 2.0, 22.0 * k, c.text3());
        }
        Shape::Dock => {
            let k = c.h / 54.0;
            let m = style::M_DOCK * k;
            let bs = style::BADGE_DOCK * k;
            sys_mark(c, Mark::Plate(u.glyph, c.text2()), m, (c.h - bs) / 2.0, bs);
            let x0 = m + bs + style::SP[4] * k;
            let tw = c.w - m - x0;
            c.text_base(u.name, x0, 30.0 * k, style::TITLE.size * k, 600, c.text1(), Align::Left, tw);
            reason_line(c, u.short, x0, 44.0 * k, style::FOOT.size * k, u.err, Align::Left, tw);
        }
        Shape::Small => {
            let m = style::M_SMALL;
            sys_mark(c, Mark::Plate(u.glyph, c.text2()), m, m, style::BADGE_SMALL);
            let tw = c.w - 2.0 * m;
            c.text_base(u.short, m, 86.0, style::TITLE.size, 600, c.text1(), Align::Left, tw);
            reason_line(c, u.sub, m, 106.0, style::CAPTION.size, u.err, Align::Left, tw);
        }
        Shape::Medium => {
            let m = style::M_CARD;
            let bs = style::BADGE_SMALL;
            sys_mark(c, Mark::Plate(u.glyph, c.text2()), m, m, bs);
            let x0 = m + bs + style::SP[5];
            c.text_base(u.name, x0, 28.0, 15.0, 600, c.text1(), Align::Left, c.w - m - x0);
            reason_line(c, u.reason, x0, 44.0, style::CAPTION.size, u.err, Align::Left, c.w - m - x0);
            if !u.pill.is_empty() {
                let (bw, bh) = (120.0, style::BTN_SECONDARY);
                sys_pill(c, c.w - m - bw, c.h - m - bh, bw, bh, u.pill);
            }
        }
        Shape::Large => {
            let bs = 48.0;
            let x = (c.w - bs) / 2.0;
            c.rr(x, 36.0, bs, bs, bs * style::BADGE_R, c.plate());
            c.glyph(u.glyph, c.w / 2.0, 36.0 + bs / 2.0, 24.0, c.text2());
            let tw = c.w - 2.0 * style::M_CARD;
            c.text_base(u.name, c.w / 2.0, 108.0, 15.0, 600, c.text1(), Align::Center, tw);
            reason_line(c, u.reason, c.w / 2.0, 128.0, style::CAPTION.size, u.err, Align::Center, tw);
            if !u.pill.is_empty() {
                let (bw, bh) = (132.0, 30.0);
                sys_pill(c, (c.w - bw) / 2.0, 150.0, bw, bh, u.pill);
            }
        }
    }
}

// ---- Ring faces (cpu, memory, battery, disk)

struct RingFace {
    frac: f32,
    col: Col,
    /// The key value ("23%").
    value: String,
    /// Inside the Mini ring.
    mini: String,
    /// Replaces the Mini number (the charging bolt).
    mini_glyph: Option<&'static str>,
    dock_cap: String,
    dock_slot: SysSlot,
    small_slot: SmallSlot,
    caption: String,
    hero_glyph: Option<&'static str>,
    third: Option<(String, Col)>,
    meter: Option<(f32, Col, f32)>,
    acc: SysAcc,
    detail: SysDetail,
}

fn sys_ring(c: &Canvas, sh: Shape, r: &RingFace) {
    let mark = Mark::Ring(r.frac, r.col);
    match sh {
        Shape::Mini => sys_mini_ring(c, mark, &r.mini, r.mini_glyph),
        Shape::Dock => sys_dock(c, mark, &r.value, &r.dock_cap, None, None, &r.dock_slot),
        Shape::Small => sys_small(c, mark, &r.small_slot, &r.value, None, &r.caption, None),
        _ => {
            let third = r.third.as_ref().map(|(t, col)| (t.as_str(), *col));
            sys_glance(c, &Glance { mark, hero: &r.value, hero_glyph: r.hero_glyph, cap: &r.caption, third, meter: r.meter, acc: &r.acc });
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                sys_detail(c, &r.detail);
            }
        }
    }
}

// ---- Faces

fn sys(c: &Canvas, out: &mut Painted, k: SysKind, x: &Ctx, sh: Shape) {
    let st = x.stats;
    let cpu = st.map(|s| s.cpu_percent).unwrap_or(0.0);
    let (mu, mt) = st.map(|s| (s.mem_used_gb, s.mem_total_gb)).unwrap_or((0.0, 0.0));
    let memf = if mt > 0.0 { mu / mt } else { 0.0 };
    if matches!(k, SysKind::Resource | SysKind::Cpu | SysKind::Memory | SysKind::Network) {
        out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("taskmgr".into())));
    }
    let mut cpu_hist: Vec<f64> = x.hub.cpu_hist.iter().rev().take(SYS_HIST).rev().map(|v| *v as f64).collect();
    if cpu_hist.len() < 2 && x.sample {
        cpu_hist = sys_demo_hist(7, cpu as f64, 9.0, 2.0, 100.0);
    }
    match k {
        SysKind::Resource => sys_resource(c, sh, cpu, mu, mt, &cpu_hist),
        SysKind::Cpu => {
            let peak = cpu_hist.iter().copied().fold(cpu as f64, f64::max) as f32;
            let pct = sys_pct(cpu);
            sys_ring(
                c,
                sh,
                &RingFace {
                    frac: cpu / 100.0,
                    col: BLUE,
                    value: pct.clone(),
                    mini: pct,
                    mini_glyph: None,
                    dock_cap: "CPU".into(),
                    dock_slot: SysSlot::Spark(cpu_hist.clone(), BLUE),
                    small_slot: SmallSlot::Spark(cpu_hist.clone(), BLUE),
                    caption: "CPU load".into(),
                    hero_glyph: None,
                    third: (cpu_hist.len() >= 2).then(|| (format!("Peak {} · last minute", sys_pct(peak)), c.text3())),
                    meter: None,
                    acc: SysAcc::Spark(cpu_hist.clone(), BLUE),
                    detail: SysDetail::Chart(cpu_hist, BLUE, cpu / 100.0),
                },
            );
        }
        SysKind::Memory => {
            // No memory history is kept yet, so Large shows used / free / total.
            let pct = sys_pct(memf * 100.0);
            let gb = format!("{mu:.1} of {mt:.0} GB");
            sys_ring(
                c,
                sh,
                &RingFace {
                    frac: memf,
                    col: PURPLE,
                    value: pct.clone(),
                    mini: pct,
                    mini_glyph: None,
                    dock_cap: "Memory".into(),
                    dock_slot: SysSlot::Meter(gb.clone(), memf, PURPLE),
                    small_slot: SmallSlot::Foot(format!("{mu:.1} / {mt:.0} GB"), c.text2()),
                    caption: "Memory used".into(),
                    hero_glyph: None,
                    third: Some((gb, c.text3())),
                    meter: None,
                    acc: SysAcc::Meter(memf, PURPLE),
                    detail: SysDetail::Stats([
                        (format!("{mu:.1} GB"), "Used".into()),
                        (format!("{:.1} GB", (mt - mu).max(0.0)), "Free".into()),
                        (format!("{mt:.0} GB"), "Total".into()),
                    ]),
                },
            );
        }
        SysKind::Network => sys_network(c, x, sh),
        SysKind::Brightness => sys_brightness(c, out, x, sh),
        _ => sys_local(c, out, k, x, sh),
    }
}

/// Resource monitor: CPU (always blue) and memory (always purple), each shown once.
fn sys_resource(c: &Canvas, sh: Shape, cpu: f32, mu: f32, mt: f32, cpu_hist: &[f64]) {
    let memf = if mt > 0.0 { mu / mt } else { 0.0 };
    let mark = Mark::Dual(cpu / 100.0, memf);
    let rows = [("CPU", sys_pct(cpu), cpu / 100.0, BLUE), ("MEM", sys_pct(memf * 100.0), memf, PURPLE)];
    match sh {
        Shape::Mini => sys_mini_ring(c, mark, "", None),
        Shape::Dock => {
            let k = c.h / 54.0;
            let m = style::M_DOCK * k;
            let bs = style::BADGE_DOCK * k;
            sys_mark(c, mark, m, (c.h - bs) / 2.0, bs);
            let x0 = m + bs + style::SP[4] * k;
            let wide = sys_dock_l(c);
            let slot_w = (c.w * 0.34).round();
            let slot_x = c.w - m - slot_w;
            let right = if wide { slot_x - style::SP[4] * k } else { c.w - m };
            let (ls, vs) = (style::FOOT.size * k, style::TITLE.size * k);
            let lw = rows.iter().map(|r| c.measure(r.0, ls, 400, Font::Text).0).fold(0.0, f32::max) + style::SP[1] * k;
            for (i, (l, v, _, _)) in rows.iter().enumerate() {
                let base = (26.0 + 16.0 * i as f32) * k;
                c.text_base(l, x0, base, ls, 400, c.text2(), Align::Left, lw);
                c.text_base(v, x0 + lw, base, vs, 600, c.text1(), Align::Left, right - x0 - lw);
            }
            if wide {
                c.area(slot_x, 14.0 * k, slot_w, 26.0 * k, cpu_hist, BLUE, true);
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let bs = style::BADGE_SMALL;
            sys_mark(c, mark, m, m, bs);
            let sx0 = m + bs + style::SP[4];
            c.area(sx0, 16.0, c.w - m - sx0, 26.0, cpu_hist, BLUE, true);
            for (i, (l, v, _, col)) in rows.iter().enumerate() {
                let x0 = if i == 0 { m } else { c.w / 2.0 + style::SP[2] };
                let cw = c.w / 2.0 - m - style::SP[2];
                let (vv, fs) = fit_value(c, v, style::VALUE, cw);
                c.text_base(&vv, x0, 88.0, fs, style::VALUE.weight, c.text1(), Align::Left, c.w);
                c.text_base(l, x0, 104.0, style::MICRO.size, style::MICRO.weight, *col, Align::Left, cw);
            }
        }
        _ => {
            let m = style::M_CARD;
            let gap = style::SP[7];
            let cw = (c.w - 2.0 * m - gap) / 2.0;
            for (i, (l, v, f, col)) in rows.iter().enumerate() {
                let x0 = m + i as f32 * (cw + gap);
                c.text_base(l, x0, 26.0, style::MICRO.size, style::MICRO.weight, *col, Align::Left, cw);
                let (vv, fs) = fit_value(c, v, style::HERO, cw);
                c.text_base(&vv, x0, 60.0, fs, style::HERO.weight, c.text1(), Align::Left, c.w);
                c.bar(x0, 70.0, cw, style::METER_H, *f, *col);
            }
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let base = c.h - style::M_CARD - 2.0;
                sys_history(c, cpu_hist, BLUE, style::DETAIL_TOP, base - 18.0, cpu / 100.0);
                let peak = cpu_hist.iter().copied().fold(cpu as f64, f64::max) as f32;
                let pw = c.text_base(&format!("Peak {}", sys_pct(peak)), c.w - m, base, style::FOOT.size, 500, c.text2(), Align::Right, c.w / 2.0);
                c.text_base(&format!("{mu:.1} of {mt:.0} GB memory in use"), m, base, style::FOOT.size, 400, c.text2(), Align::Left, c.w - 2.0 * m - pw - style::SP[3]);
            }
        }
    }
}

fn sys_network(c: &Canvas, x: &Ctx, sh: Shape) {
    let (d, u) = x.stats.map(|s| (s.net_down_bps, s.net_up_bps)).unwrap_or((0.0, 0.0));
    let hist: Vec<(f64, f64)> = x.hub.net_hist.iter().rev().take(SYS_HIST).rev().copied().collect();
    let (down, up): (Vec<f64>, Vec<f64>) = if hist.len() < 2 && x.sample {
        (sys_demo_hist(11, d, d * 0.35, d * 0.1, d * 4.0), sys_demo_hist(13, u, u * 0.4, u * 0.1, u * 4.0))
    } else {
        hist.into_iter().unzip()
    };
    let mark = Mark::Plate(G_DOWNLOAD, BLUE);
    match sh {
        Shape::Mini => sys_mini_value(c, G_DOWNLOAD, BLUE, &rate_short(d)),
        Shape::Dock => {
            let slot = if sys_dock_l(c) { SysSlot::Net(down, up) } else { SysSlot::None };
            sys_dock(c, mark, &rate(d), &rate(u), Some(("↑", GREEN)), None, &slot);
        }
        Shape::Small => {
            let r = rate(d);
            let (num, unit) = r.split_once(' ').unwrap_or((&r, ""));
            sys_small(c, mark, &SmallSlot::Spark(down.clone(), BLUE), num, Some(unit), &format!("{} up", rate(u)), Some(("↑", GREEN)));
        }
        _ => {
            let m = style::M_CARD;
            let (ts, ws) = (style::TITLE.size, style::TITLE.weight);
            let dt = rate(d);
            let aw = c.measure("↓", ts, ws, Font::Text).0 + style::SP[1];
            let tw = aw + c.measure(&dt, ts, ws, Font::Text).0;
            sys_header(c, G_DOWNLOAD, BLUE, "Network", tw);
            sys_arrow_text(c, "↓", BLUE, &dt, c.w - m, style::HEADER_BASE, ts, ws, c.text1(), Align::Right, c.w / 2.0);
            sys_arrow_text(c, "↑", GREEN, &rate(u), c.w - m, 45.0, style::FOOT.size, 400, c.text2(), Align::Right, c.w / 2.0);
            let (top, large) = (52.0, sh == Shape::Large);
            let bottom = if large { c.h - m - 14.0 } else { c.h - m };
            let drawn = sys_net_chart(c, m, top, c.w - 2.0 * m, bottom - top, &down, &up, if large { style::STROKE } else { style::STROKE_SPARK }, true);
            if large {
                if drawn {
                    sys_time_labels(c);
                } else {
                    sys_collecting(c);
                }
            }
        }
    }
}

fn sys_brightness(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let open = "ms-settings:display";
    // Previews show a sample level, like connector widgets show sample data.
    let Some(v) = x.hub.brightness.or(if x.sample { Some(0.64) } else { None }) else {
        let u = Unsup { glyph: G_SUN, name: "Brightness", short: "Not supported", sub: "On this display", reason: "Not supported on this display", err: false, pill: "Display settings" };
        sys_unsupported(c, out, sh, &u, open);
        return;
    };
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open(open.into())));
    let pct = format!("{:.0}%", v * 100.0);
    let mark = Mark::Plate(G_SUN, c.text2());
    // One neutral level colour at every size (the slider, meters and the chosen preset):
    // text1 in dark (a white level, like a lit screen); text2 in light, so the slider is a
    // calm graphite rather than a solid black slab.
    let fill = if c.light { c.text2() } else { c.text1() };
    // The value is read from the main display (DDC/CI); the plate already says brightness.
    let cap = "Main display";
    match sh {
        Shape::Mini => sys_mini_value(c, G_SUN, c.text2(), &pct),
        Shape::Dock => sys_dock(c, mark, &pct, cap, None, None, &SysSlot::Meter(String::new(), v, fill)),
        Shape::Small => sys_small(c, mark, &SmallSlot::Meter(v, fill, String::new()), &pct, None, cap, None),
        _ => {
            let m = style::M_CARD;
            let r = style::VALUE;
            let tw = c.measure(&pct, r.size, r.weight, Font::Text).0;
            sys_header(c, G_SUN, c.text2(), "Brightness", tw);
            c.text_base(&pct, c.w - m, style::HEADER_BASE, r.size, r.weight, c.text1(), Align::Right, c.w / 2.0);
            // The slider: 28 tall, track = plate, fill = text1 with the sun in the card colour.
            let (bx, bw, bh, by) = (m, c.w - 2.0 * m, style::BTN_SECONDARY, 52.0);
            c.rr(bx, by, bw, bh, bh / 2.0, c.plate());
            c.rr(bx, by, (bw * v.clamp(0.0, 1.0)).max(bh), bh, bh / 2.0, fill);
            c.glyph(G_SUN, bx + bh / 2.0, by + bh / 2.0, style::CAPTION.size, c.halo());
            out.hits.push(((bx, by - style::SP[1], bw, bh + style::SP[3]), WAct::Brightness));
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                // Presets: each pill sets its level; the chosen one is filled.
                let gap = style::SP[3];
                let pw = (c.w - 2.0 * m - 2.0 * gap) / 3.0;
                for (i, p) in [0.3f32, 0.6, 1.0].iter().enumerate() {
                    let px = m + i as f32 * (pw + gap);
                    let on = (v - p).abs() < 0.025;
                    c.rr(px, 120.0, pw, bh, bh / 2.0, if on { fill } else { c.plate() });
                    let fs = style::CAPTION.size;
                    c.text_cap(&format!("{:.0}%", p * 100.0), px + pw / 2.0, 120.0 + (bh - c.cap_h(fs)) / 2.0, fs, 600, if on { c.halo() } else { c.text1() }, Align::Center, pw);
                    out.hits.push(((px, 120.0, pw, bh), WAct::SetBrightness(*p)));
                }
                c.text_base("Main display · DDC/CI", m, c.h - m - 4.0, style::FOOT.size, 400, c.text3(), Align::Left, c.w - 2.0 * m);
            }
        }
    }
}

/// When Windows started: days before today, minute of that day, and its weekday.
fn sys_boot(now: &super::Now, hours: f64) -> (i64, i64, usize) {
    let up = (hours * 60.0).round() as i64;
    let t = (now.hour * 60 + now.minute) as i64 - up;
    let days_back = if t >= 0 { 0 } else { (-t + 1439) / 1440 };
    (days_back, t.rem_euclid(1440), (now.weekday as i64 - days_back).rem_euclid(7) as usize)
}

/// "9:14 AM".
fn sys_time(min_of_day: i64) -> String {
    let (h, m) = (min_of_day / 60, min_of_day % 60);
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{h12}:{m:02} {}", if h < 12 { "AM" } else { "PM" })
}

/// Battery, disk, storage, language and uptime: local connectors, drawn in this family's
/// own loading and unsupported states (never the connector placeholder).
fn sys_local(c: &Canvas, out: &mut Painted, k: SysKind, x: &Ctx, sh: Shape) {
    let conn = match k {
        SysKind::Battery => "battery",
        SysKind::Language => "language",
        SysKind::Uptime => "uptime",
        _ => "storage",
    };
    let (name, mark, glyph, open) = match k {
        SysKind::Battery => ("Battery", Mark::Ring(0.0, GREEN), G_BATTERY, ""),
        SysKind::Disk => ("Disk", Mark::Ring(0.0, ORANGE), G_DRIVE, "ms-settings:storagesense"),
        SysKind::Storage => ("Storage", Mark::Plate(G_DRIVE, c.text2()), G_DRIVE, "ms-settings:storagesense"),
        SysKind::Language => ("Language", Mark::Plate(G_KEYBOARD, c.text2()), G_KEYBOARD, ""),
        _ => ("Uptime", Mark::Plate(G_CLOCK, c.text2()), G_CLOCK, ""),
    };
    let f = match got(x, conn) {
        Got::Feed(f) => f,
        Got::Loading => {
            sys_skeleton(c, sh, mark, k == SysKind::Language, k == SysKind::Storage);
            return;
        }
        // The battery connector's "setup" is a fact about this PC: there is no battery.
        Got::Setup(m) if k == SysKind::Battery => {
            let u = Unsup { glyph, name, short: "No battery", sub: "Runs on AC power", reason: &m, err: false, pill: "Power settings" };
            sys_unsupported(c, out, sh, &u, "ms-settings:powersleep");
            return;
        }
        Got::Setup(m) | Got::Error(m) => {
            let u = Unsup { glyph, name, short: "Unavailable", sub: "Will try again", reason: &m, err: true, pill: "" };
            sys_unsupported(c, out, sh, &u, open);
            return;
        }
    };
    if !open.is_empty() {
        out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open(open.into())));
    }
    match k {
        SysKind::Battery => {
            let charging = f.extra.get("charging").is_some_and(|v| v == "true");
            let plugged = f.extra.get("plugged_in").is_some_and(|v| v == "true");
            let frac = f.frac.unwrap_or(0.0);
            let pct = (frac * 100.0).round();
            let col = sys_battery_col(pct, charging);
            let left = f.extra.get("time_left").filter(|s| !s.is_empty()).map(|t| format!("{t} left"));
            let status = if charging {
                "Charging"
            } else if plugged {
                "Plugged in"
            } else {
                "On battery"
            };
            let cap = left.clone().unwrap_or_else(|| status.to_string());
            let wide = sh == Shape::Dock && sys_dock_l(c);
            sys_ring(
                c,
                sh,
                &RingFace {
                    frac,
                    col,
                    value: format!("{pct:.0}%"),
                    mini: format!("{pct:.0}"),
                    mini_glyph: charging.then_some(G_BOLT),
                    // Dock L puts the time left in its slot, so its caption is the power state.
                    dock_cap: if wide { status.to_string() } else { cap.clone() },
                    dock_slot: SysSlot::Meter(left.clone().unwrap_or_default(), frac, col),
                    small_slot: if charging {
                        SmallSlot::Chip(G_BOLT, String::new())
                    } else if plugged {
                        SmallSlot::Chip(G_PLUG, String::new())
                    } else {
                        SmallSlot::None
                    },
                    caption: cap,
                    hero_glyph: charging.then_some(G_BOLT),
                    third: None,
                    meter: Some((frac, col, c.w - 2.0 * style::M_CARD - SYS_ACC_W)),
                    acc: SysAcc::None,
                    detail: SysDetail::Stats([
                        (f.extra.get("time_left").filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| "—".into()), "Time left".into()),
                        (if plugged || charging { "Plugged in" } else { "Battery" }.into(), "Power source".into()),
                        (
                            if charging {
                                "Charging"
                            } else if plugged {
                                "Not charging"
                            } else {
                                "Discharging"
                            }
                            .into(),
                            "Status".into(),
                        ),
                    ]),
                },
            );
        }
        SysKind::Disk | SysKind::Storage => {
            let letter = f.caption.trim_end_matches(':').chars().last().filter(|ch| ch.is_ascii_alphabetic()).unwrap_or('C');
            let head = format!("{letter}:");
            let drives: Vec<(String, f64, f64, f32)> = f.rows.iter().filter_map(|r| sys_free_total(&r.value).map(|(fr, tot)| (r.label.clone(), fr, tot, r.frac.unwrap_or(0.0)))).collect();
            let used = f.frac.unwrap_or(0.0);
            let free = f.value.unwrap_or(0.0);
            let total = drives.iter().find(|d| d.0.starts_with(&head)).map(|d| d.2).filter(|t| *t > 0.0).unwrap_or(if used < 1.0 { free / (1.0 - used as f64) } else { free });
            let (free_s, total_s, used_s) = (sys_fmt_gb(free), sys_fmt_gb(total), sys_fmt_gb((total - free).max(0.0)));
            let col = sys_disk_col(used);
            let used_pct = format!("{:.0}%", used * 100.0);
            let stats = SysDetail::Stats([(used_s, "Used".into()), (free_s.clone(), "Free".into()), (total_s.clone(), "Total".into())]);
            if k == SysKind::Disk {
                let cap = format!("Used on {head}");
                sys_ring(
                    c,
                    sh,
                    &RingFace {
                        frac: used,
                        col,
                        value: used_pct.clone(),
                        mini: used_pct,
                        mini_glyph: None,
                        dock_cap: cap.clone(),
                        dock_slot: SysSlot::Meter(format!("{free_s} free"), used, col),
                        small_slot: SmallSlot::Foot(format!("{free_s} free"), c.text2()),
                        caption: cap,
                        hero_glyph: None,
                        third: Some((format!("{free_s} free of {total_s}"), c.text3())),
                        meter: None,
                        acc: SysAcc::Meter(used, col),
                        detail: stats,
                    },
                );
                return;
            }
            let mark = Mark::Plate(G_DRIVE, c.text2());
            let cap = format!("Free on {head}");
            match sh {
                Shape::Mini => sys_mini_value(c, G_DRIVE, c.text2(), &sys_fmt_gb_short(free)),
                Shape::Dock if sys_dock_l(c) => sys_dock(c, mark, &free_s, &cap, None, None, &SysSlot::Meter(format!("{used_pct} used"), used, col)),
                Shape::Dock => sys_dock(c, mark, &free_s, &cap, None, Some((used, col)), &SysSlot::None),
                Shape::Small => sys_small(c, mark, &SmallSlot::Meter(used, col, used_pct), &free_s, None, &cap, None),
                _ => {
                    sys_glance(c, &Glance { mark, hero: &free_s, hero_glyph: None, cap: &cap, third: None, meter: Some((used, col, c.w - style::M_CARD)), acc: &SysAcc::None });
                    if sh == Shape::Large {
                        separator(c, style::SEP_Y_LARGE);
                        if drives.len() > 1 {
                            sys_drive_rows(c, &drives, &head, free);
                        } else {
                            sys_detail(c, &stats);
                        }
                    }
                }
            }
        }
        SysKind::Language => {
            out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Language));
            let code = f.text.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "—".into());
            let full = f.caption.clone();
            let lang = full.split(" (").next().unwrap_or(&full).to_string();
            let mark = Mark::Plate(G_KEYBOARD, c.text2());
            match sh {
                Shape::Mini => {
                    // The code is the icon.
                    let kk = c.h / 54.0;
                    let fs = fit(c, &code, 16.0 * kk, style::MINI_VALUE.min * kk, 600, Font::Text, c.w - 2.0 * style::M_MINI * kk);
                    c.text_cap(&code, c.w / 2.0, (c.h - c.cap_h(fs)) / 2.0, fs, 600, c.text1(), Align::Center, c.w);
                }
                Shape::Dock => sys_dock(c, mark, &code, &lang, None, None, &SysSlot::Keys),
                Shape::Small => sys_small(c, mark, &SmallSlot::None, &code, None, &full, None),
                _ => {
                    // Large shows the shortcut as keycaps, so its third line says which
                    // window the language belongs to instead of repeating the hint.
                    let third = if sh == Shape::Large { "For the window in front" } else { "Win + Space to switch" };
                    sys_glance(c, &Glance { mark, hero: &code, hero_glyph: None, cap: &full, third: Some((third, c.text3())), meter: None, acc: &SysAcc::None });
                    if sh == Shape::Large {
                        // Only the current language is known: a keycap composition for the shortcut.
                        separator(c, style::SEP_Y_LARGE);
                        sys_keycaps(c, &[("Win", 56.0), ("Space", 112.0)], c.w / 2.0, 120.0, 32.0, style::TITLE.size, style::TITLE.weight, c.text1(), 8.0, style::SP[3], Align::Center);
                        c.text_base("Switch input language", c.w / 2.0, 178.0, style::FOOT.size, 400, c.text2(), Align::Center, c.w - 2.0 * style::M_CARD);
                    }
                }
            }
        }
        _ => {
            let t = f.text.clone().unwrap_or_else(|| headline(&f));
            let hours = f.value.unwrap_or(0.0).max(0.0);
            let (days_back, boot_min, boot_day) = sys_boot(&x.now, hours);
            let at = sys_time(boot_min);
            let week = days_back >= 7;
            let since = match days_back {
                0 => format!("Since {at}"),
                1..=6 => format!("Since {} {at}", SYS_DAY_SHORT[boot_day]),
                n => format!("Started {n} days ago"),
            };
            let third = if week {
                ("Restart recommended".to_string(), ORANGE)
            } else if hours < 1.0 {
                (format!("Up {:.0} minutes", hours * 60.0), c.text3())
            } else {
                (format!("Up {:.0} hours", hours.floor()), c.text3())
            };
            let mark = Mark::Plate(G_CLOCK, c.text2());
            match sh {
                Shape::Mini => sys_mini_value(c, G_CLOCK, c.text2(), &t),
                Shape::Dock => {
                    // The plate already says uptime: the caption says since when, and Dock L's
                    // slot adds the start time.
                    let (cap, a, b) = match days_back {
                        0 => ("Since today".to_string(), at.clone(), "Started".to_string()),
                        1..=6 => (format!("Since {}", SYS_DAY_SHORT[boot_day]), at.clone(), "Started".to_string()),
                        n => ("Restart soon".to_string(), at.clone(), format!("{n} days ago")),
                    };
                    sys_dock(c, mark, &t, &cap, None, None, &SysSlot::Two(a, b));
                }
                Shape::Small => {
                    // The caption must fit whole: drop "Since" before cutting the time.
                    let inner = c.w - 2.0 * style::M_SMALL;
                    let cap = if c.measure(&since, style::CAPTION.size, 400, Font::Text).0 <= inner { since.clone() } else { since.trim_start_matches("Since ").to_string() };
                    let slot = if hours >= 1.0 && !week { SmallSlot::Foot(format!("{:.0} h", hours.floor()), c.text3()) } else { SmallSlot::None };
                    sys_small(c, mark, &slot, &t, None, &cap, None);
                }
                _ => {
                    sys_glance(c, &Glance { mark, hero: &t, hero_glyph: None, cap: &since, third: Some((&third.0, third.1)), meter: None, acc: &SysAcc::None });
                    if sh == Shape::Large {
                        separator(c, style::SEP_Y_LARGE);
                        sys_uptime_strip(c, &x.now, days_back, boot_min, boot_day);
                    }
                }
            }
        }
    }
}

/// Storage Large with more than one drive: one row per drive at the 24 pitch (label, a
/// 60 px meter, free space), then a summary when there's room.
fn sys_drive_rows(c: &Canvas, drives: &[(String, f64, f64, f32)], head: &str, head_free: f64) {
    let m = style::M_CARD;
    let pitch = style::ROW_L;
    let ls = style::BODY.size;
    let top = style::DETAIL_TOP - style::SP[1];
    let base = |i: usize| top + i as f32 * pitch + (pitch + c.cap_h(ls)) / 2.0;
    // Up to 3 rows fit; with 2 there's room for the summary, with more than 3 it replaces the third.
    let n = drives.len();
    let shown = if n == 3 { 3 } else { n.min(2) };
    let free_of = |d: &(String, f64, f64, f32)| if d.0.starts_with(head) { head_free } else { d.1 };
    let trail: Vec<String> = drives.iter().take(shown).map(|d| format!("{} free", sys_fmt_gb(free_of(d)))).collect();
    let tw = trail.iter().map(|t| c.measure(t, style::FOOT.size, 500, Font::Text).0).fold(0.0, f32::max);
    let mw = 60.0;
    let mx = c.w - m - tw - style::SP[3] - mw;
    for (i, (d, t)) in drives.iter().zip(&trail).enumerate() {
        let b = base(i);
        let label = if d.0.trim_end().ends_with(':') { format!("{} Local Disk", d.0.trim_end()) } else { d.0.clone() };
        c.text_base(&label, m, b, ls, style::BODY.weight, c.text1(), Align::Left, mx - style::SP[3] - m);
        let mh = style::METER_H_COMPACT;
        c.bar(mx, b - c.cap_h(ls) / 2.0 - mh / 2.0, mw, mh, d.3, sys_disk_col(d.3));
        c.text_base(t, c.w - m, b, style::FOOT.size, 500, c.text2(), Align::Right, tw + 4.0);
    }
    if shown < n || n == 2 {
        let total_free: f64 = drives.iter().map(free_of).sum();
        let s = format!("{n} drives · {} free", sys_fmt_gb(total_free));
        c.text_base(&s, m, base(shown), style::FOOT.size, 400, c.text3(), Align::Left, c.w - 2.0 * m);
    }
}

/// Uptime Large: the last seven days (today at the right), filled from boot to now, and when it started.
fn sys_uptime_strip(c: &Canvas, now: &super::Now, days_back: i64, boot_min: i64, boot_day: usize) {
    let m = style::M_CARD;
    let gap = style::SP[1];
    let cw = (c.w - 2.0 * m - 6.0 * gap) / 7.0;
    let (y, h, r) = (128.0, 28.0, 6.0);
    let now_min = (now.hour * 60 + now.minute) as f32;
    for i in 0..7usize {
        let back = 6 - i as i64;
        let x0 = m + i as f32 * (cw + gap);
        let wd = (now.weekday as i64 - back).rem_euclid(7) as usize;
        let today = back == 0;
        c.text_base(SYS_DAY_LETTERS[wd], x0 + cw / 2.0, 120.0, style::MICRO.size, style::MICRO.weight, if today { c.text1() } else { c.text3() }, Align::Center, cw);
        c.rr(x0, y, cw, h, r, c.track());
        if back > days_back {
            continue;
        }
        // The part of this day the PC was up: from boot (on the boot day) to now (today).
        // A partial day is the cell itself, clipped to that span, so its corners stay the
        // cell's and the cut edge is straight (a fill, not a separate pill).
        let from = if back == days_back { boot_min as f32 / 1440.0 } else { 0.0 };
        let to = if today { (now_min / 1440.0).max(from + 0.03) } else { 1.0 };
        c.clip(x0 + cw * from, y, cw * (to - from).clamp(0.0, 1.0), h, || c.rr(x0, y, cw, h, r, alpha(BLUE, 0.9)));
        if today {
            // Drawn after the fill, on the cell's own outline, so stroke and fill stay concentric.
            c.rr_stroke(x0, y, cw, h, r, c.text3(), 1.0);
        }
    }
    let when = match days_back {
        0 => format!("Started today at {}", sys_time(boot_min)),
        1 => format!("Started yesterday at {}", sys_time(boot_min)),
        2..=6 => format!("Started {} at {}", super::DAYS[boot_day], sys_time(boot_min)),
        n => format!("Started {n} days ago"),
    };
    c.text_base(&when, m, c.h - m - 4.0, style::FOOT.size, 400, c.text2(), Align::Left, c.w - 2.0 * m);
}

// ------------------------------------------------------------------ tools
// One layout for every single-action tool (docs/widget-design.md §3, §5): a control mark
// (a circle for on/off toggles, a rounded plate for launchers), its name and state.
//   Mini   mark 40 centred
//   Dock   mark 30 at (12, 12), name 13/600 and state 11 at x 52; Dock L adds a switch,
//          the Windows shortcut or a value in the accessory slot
//   Small  mark 44 centred at y 22, name baseline 88, state baseline 104
//   Medium mark 44 at (16, 26), name 17/600 baseline 45, state 12 baseline 63
//   Large  Medium unchanged, a separator at 96, then up to three fact rows at a 24 pitch

const G_OPEN: &str = "\u{E8A7}";
const G_FOLDER: &str = "\u{E8B7}";
const G_ADD: &str = "\u{E710}";
const G_FOCUS: &str = "\u{EC43}";
const G_PALETTE: &str = "\u{E790}";
const G_POWER: &str = "\u{E7E8}";
const G_EYE: &str = "\u{E7B3}";
const G_CUT: &str = "\u{E8C6}";
const G_REGION: &str = "\u{F407}";
const G_WINDOW: &str = "\u{E737}";
const G_FULL: &str = "\u{E740}";
const WHATSAPP: Col = hex(0x25D366);

/// Mark sizes per shape (dock sizes are drawn at 54 and scale with the icon size).
const MARK_MINI: f32 = 40.0;
const MARK_CARD: f32 = 44.0;
/// Small: the mark's top, then the name and state baselines.
const SMALL_MARK_Y: f32 = 22.0;
const SMALL_NAME_BASE: f32 = 88.0;
const SMALL_STATE_BASE: f32 = 104.0;
/// Medium: the mark's top-left, the text column, name and state baselines.
const MED_MARK_Y: f32 = 26.0;
const MED_TEXT_X: f32 = 72.0;
const MED_NAME_BASE: f32 = 45.0;
const MED_STATE_BASE: f32 = 63.0;
/// Keycaps and switches.
const KEY_H: f32 = 20.0;
const KEY_R: f32 = 6.0;
const SWITCH_W: f32 = 36.0;
const SWITCH_H: f32 = 20.0;

/// The dock's scale (its layouts are specified at 54 tall).
fn dock_k(c: &Canvas) -> f32 {
    c.h / 54.0
}

/// A wide dock pill (Dock L), which has room for the accessory slot.
fn dock_wide(c: &Canvas) -> bool {
    c.w >= c.h * 3.4
}

/// The mark's box (x, y, size) for a shape.
fn mark_box(c: &Canvas, sh: Shape) -> (f32, f32, f32) {
    match sh {
        Shape::Mini => {
            let s = MARK_MINI * dock_k(c);
            ((c.w - s) / 2.0, (c.h - s) / 2.0, s)
        }
        Shape::Dock => {
            let k = dock_k(c);
            let s = style::BADGE_DOCK * k;
            (style::M_DOCK * k, (c.h - s) / 2.0, s)
        }
        Shape::Small => ((c.w - MARK_CARD) / 2.0, SMALL_MARK_Y, MARK_CARD),
        Shape::Medium | Shape::Large => (style::M_CARD, MED_MARK_Y, MARK_CARD),
    }
}

/// A control mark: a circle (toggles) or a rounded plate (launchers). Filled with `fill`
/// and a white glyph when given, else the plate token with a text1 glyph.
fn mark(c: &Canvas, g: &str, x: f32, y: f32, s: f32, round: bool, fill: Option<Col>) {
    let (bg, ink) = match fill {
        Some(f) => (f, WHITE),
        None => (c.plate(), c.text1()),
    };
    if round {
        c.circle(x + s / 2.0, y + s / 2.0, s / 2.0, bg);
        c.glyph(g, x + s / 2.0, y + s / 2.0, s * style::GLYPH_IN, ink);
    } else {
        plate(c, g, x, y, s, bg, ink);
    }
}

/// A red attention dot on a mark's top-right corner, ringed in the card colour.
fn mark_dot(c: &Canvas, x: f32, y: f32, s: f32, d: f32, col: Col) {
    let (cx, cy) = (x + s - 2.0, y + 2.0);
    c.circle(cx, cy, d / 2.0 + 1.5, c.halo());
    c.circle(cx, cy, d / 2.0, col);
}

/// A keycap ("Win + D") right-aligned at `right`, centred on `cy`. Returns its width.
fn keycap(c: &Canvas, keys: &str, right: f32, cy: f32, k: f32) -> f32 {
    let fs = style::MICRO.size * k;
    let w = c.measure(keys, fs, style::MICRO.weight, Font::Text).0 + 2.0 * style::SP[3] * k;
    let h = KEY_H * k;
    c.rr(right - w, cy - h / 2.0, w, h, KEY_R * k, c.plate());
    c.text_base(keys, right - w / 2.0, cy + c.cap_h(fs) / 2.0, fs, style::MICRO.weight, c.text2(), Align::Center, w);
    w
}

/// An on/off switch right-aligned at `right`, centred on `cy`. Returns its width.
fn switch(c: &Canvas, on: bool, right: f32, cy: f32, k: f32) -> f32 {
    let (w, h) = (SWITCH_W * k, SWITCH_H * k);
    let x = right - w;
    c.rr(x, cy - h / 2.0, w, h, h / 2.0, if on { c.blue() } else { c.track() });
    let r = h / 2.0 - style::SP[0] * k;
    let kx = if on { right - h / 2.0 } else { x + h / 2.0 };
    c.circle(kx, cy, r, WHITE);
    if c.light {
        c.circle_stroke(kx, cy, r, c.card_edge(), 1.0);
    }
    w
}

/// A fully round capsule with a white label (Dock L slot): returns its width.
fn capsule(c: &Canvas, s: &str, right: f32, cy: f32, fill: Col, k: f32) -> f32 {
    let fs = style::FOOT.size * k;
    let w = c.measure(s, fs, 600, Font::Text).0 + 2.0 * style::SP[3] * k;
    let h = KEY_H * k;
    c.rr(right - w, cy - h / 2.0, w, h, h / 2.0, fill);
    c.text_base(s, right - w / 2.0, cy + c.cap_h(fs) / 2.0, fs, 600, WHITE, Align::Center, w);
    w
}

/// What a tool's Dock L slot (and Medium's right edge) holds.
enum ToolSlot {
    None,
    /// On/off switch (toggles).
    Switch,
    /// The Windows shortcut.
    Key(&'static str),
    /// A value 17/600.
    Value(String),
    /// A white label on a coloured capsule.
    Capsule(&'static str, Col),
    /// A quiet note in text3.
    Note(String),
}

/// The end of a Large fact row.
enum Trail {
    Text(String),
    Key(&'static str),
    /// A link: chevron, and the row opens it.
    Go(WAct),
    None,
}

/// A Large fact row: glyph, label and trailing value. A row with no glyph is a footnote.
struct TRow {
    g: Option<&'static str>,
    label: String,
    trail: Trail,
}

fn row(g: &'static str, label: &str, trail: Trail) -> TRow {
    TRow { g: Some(g), label: label.to_string(), trail }
}

fn note_row(label: &str) -> TRow {
    TRow { g: None, label: label.to_string(), trail: Trail::None }
}

/// A single-action tool, described once and drawn at every size.
struct ToolFace {
    g: &'static str,
    /// Name on Small and up.
    name: &'static str,
    /// Name on the dock (never ellipsized).
    short: &'static str,
    /// Dock second line (empty: one line).
    state: String,
    /// Small and Medium captions.
    small: String,
    medium: String,
    /// Toggles are round; launchers are plates.
    round: bool,
    /// The mark's fill: blue for an on toggle, the identity colour, or None for the plate.
    fill: Option<Col>,
    slot: ToolSlot,
    /// Show the slot on Medium too (switches only).
    slot_medium: bool,
    rows: Vec<TRow>,
    act: WAct,
}

impl ToolFace {
    fn new(g: &'static str, name: &'static str, act: WAct) -> Self {
        ToolFace { g, name, short: name, state: String::new(), small: String::new(), medium: String::new(), round: false, fill: None, slot: ToolSlot::None, slot_medium: false, rows: Vec::new(), act }
    }
}

/// Draw a tool at any shape. Returns the mark's box so callers can badge it.
fn tool_face(c: &Canvas, out: &mut Painted, f: &ToolFace, sh: Shape) -> (f32, f32, f32) {
    out.hits.push(((0.0, 0.0, c.w, c.h), f.act.clone()));
    let (mx, my, ms) = mark_box(c, sh);
    mark(c, f.g, mx, my, ms, f.round, f.fill);
    match sh {
        Shape::Mini => {}
        Shape::Dock => {
            let k = dock_k(c);
            let m = style::M_DOCK * k;
            let x0 = mx + ms + style::SP[4] * k;
            let mut right = c.w - m;
            if dock_wide(c) {
                if let Some(w) = tool_slot(c, &f.slot, c.w - m, 27.0 * k, k, f.fill.is_some()) {
                    right = c.w - m - w - style::SP[5] * k;
                }
            }
            let ls = fit(c, f.short, style::TITLE.size * k, style::TITLE.min * k, 600, Font::Text, right - x0);
            if f.state.is_empty() {
                c.text_base(f.short, x0, 32.0 * k, ls, 600, c.text1(), Align::Left, c.w);
            } else {
                c.text_base(f.short, x0, 26.0 * k, ls, 600, c.text1(), Align::Left, c.w);
                c.text_base(&f.state, x0, 40.0 * k, style::FOOT.size * k, 400, c.text2(), Align::Left, right - x0);
            }
        }
        Shape::Small => {
            let inner = c.w - 2.0 * style::M_SMALL;
            let ls = fit(c, f.name, style::TITLE.size, style::TITLE.min, 600, Font::Text, inner);
            c.text_base(f.name, c.w / 2.0, SMALL_NAME_BASE, ls, 600, c.text1(), Align::Center, c.w);
            c.text_base(&f.small, c.w / 2.0, SMALL_STATE_BASE, style::FOOT.size, 400, c.text2(), Align::Center, inner);
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let mut right = c.w - m;
            if f.slot_medium {
                if let Some(w) = tool_slot(c, &f.slot, c.w - m, my + ms / 2.0, 1.0, f.fill.is_some()) {
                    right -= w + style::SP[5];
                }
            }
            let ls = fit(c, f.name, style::VALUE.size, style::VALUE.min, 600, Font::Text, right - MED_TEXT_X);
            c.text_base(f.name, MED_TEXT_X, MED_NAME_BASE, ls, 600, c.text1(), Align::Left, c.w);
            c.text_base(&f.medium, MED_TEXT_X, MED_STATE_BASE, style::CAPTION.size, 400, c.text2(), Align::Left, right - MED_TEXT_X);
            if sh == Shape::Large {
                tool_rows(c, out, &f.rows);
            }
        }
    }
    (mx, my, ms)
}

/// Draw a slot right-aligned at `right`, centred on `cy`. Returns its width (None if empty).
fn tool_slot(c: &Canvas, s: &ToolSlot, right: f32, cy: f32, k: f32, on: bool) -> Option<f32> {
    match s {
        ToolSlot::None => None,
        ToolSlot::Switch => Some(switch(c, on, right, cy, k)),
        ToolSlot::Key(keys) => Some(keycap(c, keys, right, cy, k)),
        ToolSlot::Capsule(t, col) => Some(capsule(c, t, right, cy, *col, k)),
        ToolSlot::Value(v) => {
            let fs = style::VALUE.size * k;
            Some(c.text_base(v, right, cy + c.cap_h(fs) / 2.0, fs, style::VALUE.weight, c.text1(), Align::Right, c.w))
        }
        ToolSlot::Note(t) => {
            let fs = style::FOOT.size * k;
            Some(c.text_base(t, right, cy + c.cap_h(fs) / 2.0, fs, 400, c.text3(), Align::Right, c.w))
        }
    }
}

/// The Large detail region: a separator at 96, then up to three rows at a 24 pitch.
fn tool_rows(c: &Canvas, out: &mut Painted, rows: &[TRow]) {
    if rows.is_empty() {
        return;
    }
    separator(c, style::SEP_Y_LARGE);
    let m = style::M_CARD;
    let gw = 16.0;
    for (i, r) in rows.iter().take(3).enumerate() {
        let top = style::DETAIL_TOP + i as f32 * style::ROW_L;
        let cy = top + style::ROW_L / 2.0;
        let base = cy + c.cap_h(style::BODY.size) / 2.0;
        let mut right = c.w - m;
        match &r.trail {
            Trail::Text(t) => {
                let w = c.text_base(t, right, cy + c.cap_h(style::FOOT.size) / 2.0, style::FOOT.size, 500, c.text2(), Align::Right, c.w);
                right -= w + style::SP[3];
            }
            Trail::Key(keys) => {
                let w = keycap(c, keys, right, cy, 1.0);
                right -= w + style::SP[3];
            }
            Trail::Go(act) => {
                c.glyph(G_CHEV, right - 4.0, cy, 10.0, c.text3());
                right -= 8.0 + style::SP[3];
                out.hits.push(((m, top, c.w - 2.0 * m, style::ROW_L), act.clone()));
            }
            Trail::None => {}
        }
        match r.g {
            Some(g) => {
                c.glyph(g, m + gw / 2.0, cy, 14.0, c.text2());
                let x = m + gw + style::SP[3];
                c.text_base(&r.label, x, base, style::BODY.size, style::BODY.weight, c.text1(), Align::Left, right - x);
            }
            None => {
                c.text_base(&r.label, m, cy + c.cap_h(style::FOOT.size) / 2.0, style::FOOT.size, 400, c.text2(), Align::Left, right - m);
            }
        }
    }
}

/// The icon plate used by tiles: radius 0.25 × size, glyph 0.45 × size.
fn plate(c: &Canvas, g: &str, x: f32, y: f32, p: f32, fill: Col, ink: Col) {
    c.rr(x, y, p, p, p * style::BADGE_R, fill);
    c.glyph(g, x + p / 2.0, y + p / 2.0, p * style::GLYPH_IN, ink);
}

/// The Recycle Bin's item count and size, queried off the UI thread at most every 15 s.
fn bin_stats(sample: bool) -> Option<(i64, i64)> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    if sample {
        return Some((38, 214 * 1024 * 1024));
    }
    // (last query, result, a query running)
    static BIN: Mutex<(Option<Instant>, Option<(i64, i64)>, bool)> = Mutex::new((None, None, false));
    let mut g = BIN.lock().ok()?;
    let stale = g.0.map(|t| t.elapsed() > Duration::from_secs(15)).unwrap_or(true);
    if stale && !g.2 {
        g.2 = true;
        std::thread::spawn(|| {
            let mut info = windows::Win32::UI::Shell::SHQUERYRBINFO { cbSize: std::mem::size_of::<windows::Win32::UI::Shell::SHQUERYRBINFO>() as u32, ..Default::default() };
            let ok = unsafe { windows::Win32::UI::Shell::SHQueryRecycleBinW(windows::core::PCWSTR::null(), &mut info) }.is_ok();
            if let Ok(mut g) = BIN.lock() {
                let (n, size) = (info.i64NumItems, info.i64Size);
                *g = (Some(Instant::now()), ok.then_some((n, size)), false);
            }
        });
    }
    g.1
}

/// Bytes as "820 KB", "214 MB", "1.2 GB".
fn bytes_str(b: i64) -> String {
    let b = b.max(0) as f64;
    let (v, unit) = if b >= 1e9 {
        (b / 1073741824.0, "GB")
    } else if b >= 1e6 {
        (b / 1048576.0, "MB")
    } else {
        (b / 1024.0, "KB")
    };
    if v < 10.0 && unit == "GB" {
        format!("{v:.1} {unit}")
    } else {
        format!("{} {unit}", v.round().max(if b > 0.0 { 1.0 } else { 0.0 }) as i64)
    }
}

/// "1 item" / "38 items".
fn items_str(n: i64) -> String {
    if n == 1 {
        "1 item".into()
    } else {
        format!("{n} items")
    }
}

/// An app's display name: no ".exe", title-cased when it was a bare process name.
fn app_display(raw: &str) -> String {
    let t = raw.trim();
    let stem = if t.to_ascii_lowercase().ends_with(".exe") { &t[..t.len() - 4] } else { t };
    // Drop a release channel or edition in brackets: "T3 Code (Nightly)" becomes "T3 Code".
    let stem = match stem.find(" (") {
        Some(i) if stem.ends_with(')') && i > 0 => &stem[..i],
        _ => stem,
    };
    match stem.to_ascii_lowercase().as_str() {
        "explorer" => return "File Explorer".into(),
        "msedge" => return "Microsoft Edge".into(),
        "code" => return "VS Code".into(),
        "windowsterminal" => return "Terminal".into(),
        _ => {}
    }
    if stem.chars().any(|ch| ch.is_uppercase()) {
        return stem.to_string();
    }
    stem.split(' ')
        .map(|w| {
            let mut ch = w.chars();
            match ch.next() {
                Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Today's apps by time, display names merged, longest first.
fn screen_apps(x: &Ctx) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = Vec::new();
    for (name, secs) in &x.hub.screen.apps {
        let n = app_display(name);
        match v.iter_mut().find(|(k, _)| *k == n) {
            Some(e) => e.1 += secs,
            None => v.push((n, *secs)),
        }
    }
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

/// Category colours for the top apps; everything else is "other" (neutral).
const APP_COLS: [Col; 3] = [BLUE, PURPLE, ORANGE];

/// A stacked share bar: the top apps in their colours, the rest neutral, over a track.
fn share_bar(c: &Canvas, apps: &[(String, u64)], total: u64, x: f32, y: f32, w: f32, h: f32) {
    c.rr(x, y, w, h, h / 2.0, c.track());
    if total == 0 {
        return;
    }
    let mut segs: Vec<(f32, Col)> = Vec::new();
    let mut acc = 0u64;
    for (i, (_, s)) in apps.iter().take(APP_COLS.len()).enumerate() {
        acc += s;
        segs.push((acc as f32 / total as f32, APP_COLS[i]));
    }
    if acc < total {
        segs.push((1.0, c.ink(0.32)));
    }
    // Longest first, each shorter one on top: every segment keeps a round end.
    for (end, col) in segs.iter().rev() {
        c.rr(x, y, (w * end).max(h), h, h / 2.0, *col);
    }
}

/// Hour-by-hour bars on a baseline: no bar for empty hours, the current hour solid, the
/// rest lighter. One hour of use (`full`) fills 86% of the height.
#[allow(clippy::too_many_arguments)]
fn hour_bars(c: &Canvas, vals: &[f64], now: usize, full: f64, x: f32, y: f32, w: f32, h: f32, bw: f32) {
    let n = vals.len().max(1);
    let slot = w / n as f32;
    let hi = vals.iter().cloned().fold(full, f64::max);
    let past = alpha(BLUE, if c.light { 0.5 } else { 0.6 });
    let r = (bw / 2.0).min(1.5);
    for (i, v) in vals.iter().enumerate() {
        if *v <= 0.0 {
            continue;
        }
        let ht = ((v / hi) as f32 * h * 0.86).max(2.0);
        let bx = x + slot * i as f32 + (slot - bw) / 2.0;
        c.rr(bx, y + h - ht, bw, ht, r, if i == now { BLUE } else { past });
    }
    c.line(x, y + h + 0.5, x + w, y + h + 0.5, 1.0, c.sep());
}

/// Hour ticks under hour bars: Micro text3, each left-aligned to its hour's bar.
fn hour_ticks(c: &Canvas, marks: &[(usize, &str)], x: f32, w: f32, bw: f32, base: f32) {
    let slot = w / 24.0;
    for (h, l) in marks {
        let bx = x + slot * *h as f32 + (slot - bw) / 2.0;
        c.text_base(l, bx, base, style::MICRO.size, style::MICRO.weight, c.text3(), Align::Left, 40.0);
    }
}

/// The 24 hourly values (seconds), today.
fn hours_of(x: &Ctx) -> Vec<f64> {
    (0..24).map(|h| *x.hub.screen.hours.get(h).unwrap_or(&0) as f64).collect()
}

/// A time like "8 AM".
fn hour_str(h: usize) -> String {
    let h = h as u32;
    format!("{} {}", h12(h), ampm(h))
}

/// Compact duration for a Mini: "46m", "6.8h", "12h".
fn dur_mini(secs: u64) -> String {
    let h = secs as f64 / 3600.0;
    if secs < 3600 {
        format!("{}m", secs / 60)
    } else if h < 10.0 {
        let s = format!("{h:.1}");
        format!("{}h", s.strip_suffix(".0").unwrap_or(&s))
    } else {
        format!("{}h", h.round() as i64)
    }
}

fn tool(c: &Canvas, out: &mut Painted, k: ToolKind, x: &Ctx, sh: Shape) {
    let open = |s: &str| WAct::Open(s.to_string());
    match k {
        ToolKind::Tray => tray(c, out, x, sh),
        ToolKind::Connectivity => connectivity(c, out, x, sh),
        ToolKind::ControlTiles => control_tiles(c, out, x, sh),
        ToolKind::Toggles => toggles(c, out, x, sh),
        ToolKind::WinSettings => win_settings(c, out, sh),
        ToolKind::WinUpdate => win_update(c, out, x, sh),
        ToolKind::DarkMode => {
            let on = x.control.dark;
            let mut f = ToolFace::new(G_MOON, "Dark mode", WAct::Dark);
            let st = if on { "On" } else { "Off" };
            f.state = st.into();
            f.small = st.into();
            f.medium = st.into();
            f.round = true;
            f.fill = on.then(|| c.blue());
            f.slot = ToolSlot::Switch;
            f.slot_medium = true;
            f.rows = vec![
                row(G_MOON, "Apps and Windows", Trail::Text(if on { "Dark" } else { "Light" }.into())),
                row(G_PALETTE, "Colours", Trail::Go(open("ms-settings:colors"))),
                note_row("Switches apps and Windows together"),
            ];
            tool_face(c, out, &f, sh);
        }
        ToolKind::Focus => {
            let mut f = ToolFace::new(G_FOCUS, "Focus", open("ms-settings:quiethours"));
            f.small = "Do not disturb".into();
            f.medium = "Do not disturb settings".into();
            f.slot = ToolSlot::Key("Win + N");
            f.rows = vec![
                row(G_DND, "Do not disturb", Trail::Go(open("ms-settings:quiethours"))),
                row(G_SETTINGS, "Notification settings", Trail::Go(open("ms-settings:notifications"))),
                row(G_KEYBOARD, "Notification centre", Trail::Key("Win + N")),
            ];
            tool_face(c, out, &f, sh);
        }
        ToolKind::Notifications => {
            let mut f = ToolFace::new(G_BELL, "Notifications", open("ms-actioncenter:"));
            f.small = "Open centre".into();
            f.medium = "Open notification centre".into();
            f.slot = ToolSlot::Key("Win + N");
            f.rows = vec![
                row(G_KEYBOARD, "Shortcut", Trail::Key("Win + N")),
                row(G_DND, "Do not disturb", Trail::Go(open("ms-settings:quiethours"))),
                row(G_SETTINGS, "Notification settings", Trail::Go(open("ms-settings:notifications"))),
            ];
            tool_face(c, out, &f, sh);
        }
        ToolKind::ShowDesktop => {
            let on = x.hub.desktop_shown;
            let mut f = ToolFace::new(G_DESKTOP, "Show desktop", WAct::ShowDesktop);
            f.short = "Desktop";
            f.state = if on { "Shown".into() } else { String::new() };
            f.small = if on { "Restore windows" } else { "Hide windows" }.into();
            f.medium = if on { "Bring windows back" } else { "Hide all windows" }.into();
            f.round = true;
            f.fill = on.then(|| c.blue());
            f.slot = ToolSlot::Key("Win + D");
            f.rows = vec![
                row(G_KEYBOARD, "Shortcut", Trail::Key("Win + D")),
                row(G_EYE, "Peek at the desktop", Trail::Key("Win + ,")),
                note_row("Click again to bring your windows back"),
            ];
            tool_face(c, out, &f, sh);
        }
        ToolKind::KeepAwake => {
            let on = x.hub.keep_awake;
            let mut f = ToolFace::new(G_COFFEE, "Keep awake", WAct::KeepAwake);
            f.state = if on { "On" } else { "Off" }.into();
            f.small = if on { "PC stays awake" } else { "Off" }.into();
            f.medium = f.small.clone();
            f.round = true;
            f.fill = on.then(|| c.blue());
            f.slot = ToolSlot::Switch;
            f.slot_medium = true;
            f.rows = vec![
                row(G_SCREEN, "Screen", Trail::Text(if on { "Stays on" } else { "Can sleep" }.into())),
                row(G_POWER, "System sleep", Trail::Text(if on { "Paused" } else { "Normal" }.into())),
                row(G_SETTINGS, "Power settings", Trail::Go(open("ms-settings:powersleep"))),
            ];
            tool_face(c, out, &f, sh);
        }
        ToolKind::Search => search(c, out, x, sh),
        ToolKind::Run => {
            let mut f = ToolFace::new(G_OPEN, "Run", open("shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}"));
            f.small = "Open a command".into();
            f.medium = "Open a program, folder or file".into();
            f.slot = ToolSlot::Key("Win + R");
            f.rows = vec![
                row(G_KEYBOARD, "Shortcut", Trail::Key("Win + R")),
                row(G_RUN, "Run as admin", Trail::Key("Ctrl + Shift + Enter")),
                note_row("Type a program, folder or web address"),
            ];
            tool_face(c, out, &f, sh);
        }
        ToolKind::Trash => {
            let stats = bin_stats(x.sample);
            let mut f = ToolFace::new(G_TRASH, "Recycle Bin", open("shell:RecycleBinFolder"));
            match stats {
                Some((0, _)) => {
                    f.state = "Empty".into();
                    f.small = "Empty".into();
                    f.medium = "Empty".into();
                    f.slot = ToolSlot::Note("Nothing deleted".into());
                    f.rows = vec![row(G_TRASH, "Items", Trail::Text("None".into())), note_row("Deleted files wait here until you empty it")];
                }
                Some((n, b)) => {
                    f.state = items_str(n);
                    f.small = items_str(n);
                    f.medium = format!("{} \u{00B7} {}", items_str(n), bytes_str(b));
                    f.slot = ToolSlot::Value(bytes_str(b));
                    f.rows = vec![
                        row(G_TRASH, "Items", Trail::Text(n.to_string())),
                        row(G_DRIVE, "Size on disk", Trail::Text(bytes_str(b))),
                        note_row("Click to open; empty it from File Explorer"),
                    ];
                }
                None => {
                    f.small = "Click to open".into();
                    f.medium = "Click to open".into();
                    f.slot = ToolSlot::Note("Open".into());
                    f.rows = vec![row(G_TRASH, "Open Recycle Bin", Trail::Go(open("shell:RecycleBinFolder"))), note_row("Deleted files wait here until you empty it")];
                }
            }
            let (mx, my, ms) = tool_face(c, out, &f, sh);
            if sh == Shape::Mini && stats.map(|s| s.0 > 0).unwrap_or(false) {
                mark_dot(c, mx, my, ms, 6.0 * dock_k(c), c.text2());
            }
        }
        ToolKind::WhatsApp => {
            let (state, rows): (&str, Vec<TRow>) = match x.whatsapp {
                Some(true) => ("New messages", vec![row(G_POWER, "Status", Trail::Text("Running".into())), row(G_CHAT, "Messages", Trail::Text("New".into())), note_row("Click to open your chats")]),
                Some(false) => ("No new messages", vec![row(G_POWER, "Status", Trail::Text("Running".into())), row(G_CHAT, "Messages", Trail::Text("All read".into())), note_row("Click to open your chats")]),
                None => ("Not running", vec![row(G_POWER, "Status", Trail::Text("Not running".into())), note_row("Click to start WhatsApp")]),
            };
            let mut f = ToolFace::new(G_CHAT, "WhatsApp", WAct::WhatsApp);
            // The dock has room for a shorter state only.
            f.state = if x.whatsapp == Some(false) { "All read".into() } else { state.into() };
            f.small = state.into();
            f.medium = state.into();
            f.fill = Some(WHATSAPP);
            f.slot = match x.whatsapp {
                Some(true) => ToolSlot::Capsule("New", WHATSAPP),
                Some(false) => ToolSlot::Note("Running".into()),
                None => ToolSlot::Note("Click to open".into()),
            };
            f.rows = rows;
            let (mx, my, ms) = tool_face(c, out, &f, sh);
            if x.whatsapp == Some(true) {
                let d = if matches!(sh, Shape::Mini | Shape::Dock) { 7.0 * dock_k(c) } else { 8.0 };
                mark_dot(c, mx, my, ms, d, RED);
            }
        }
        ToolKind::Screenshot => screenshot(c, out, sh),
        ToolKind::Apps => apps_grid(c, out, x, sh),
        ToolKind::Notes => notes(c, out, x, sh),
        ToolKind::ScreenTime => screen_time(c, x, sh),
        ToolKind::Timeline => timeline(c, x, sh),
    }
}

/// Search: a plate on the dock and Small; a search field on Medium; Large adds pinned apps.
fn search(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    if !matches!(sh, Shape::Medium | Shape::Large) {
        let mut f = ToolFace::new(G_SEARCH, "Search", WAct::Search);
        f.small = "Apps and files".into();
        f.slot = ToolSlot::Key("Win + S");
        tool_face(c, out, &f, sh);
        return;
    }
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Search));
    let m = style::M_CARD;
    let (py, ph) = (28.0, 40.0);
    c.rr(m, py, c.w - 2.0 * m, ph, ph / 2.0, c.plate());
    let cy = py + ph / 2.0;
    c.glyph(G_SEARCH, 38.0, cy, 14.0, c.text2());
    let kw = keycap(c, "Win + S", c.w - m - style::SP[4], cy, 1.0);
    let tx = 58.0;
    let tmax = c.w - m - style::SP[4] - kw - style::SP[3] - tx;
    // The longest prompt that fits whole.
    let prompts = ["Search apps, files and settings", "Search apps and files", "Search"];
    let p = prompts.iter().find(|p| c.measure(p, style::BODY.size, 400, Font::Text).0 <= tmax).unwrap_or(&"Search");
    c.text_base(p, tx, cy + c.cap_h(style::BODY.size) / 2.0, style::BODY.size, 400, c.text2(), Align::Left, tmax);
    if sh != Shape::Large {
        return;
    }
    separator(c, style::SEP_Y_LARGE);
    c.text_base("PINNED", m, 116.0, style::MICRO.size, style::MICRO.weight, c.text3(), Align::Left, c.w);
    let cw = (c.w - 2.0 * m) / 4.0;
    let s = MARK_CARD;
    let top = 124.0;
    if x.pinned.is_empty() {
        c.text_base("Pin apps in Launchpad to see them here", m, top + 26.0, style::CAPTION.size, 400, c.text2(), Align::Left, c.w - 2.0 * m);
        return;
    }
    for (i, id) in x.pinned.iter().take(4).enumerate() {
        let cx = m + cw * (i as f32 + 0.5);
        out.icons.push((cx - s / 2.0, top, s, id.clone()));
        out.hits.push(((cx - cw / 2.0, top - 4.0, cw, c.h - style::M_CARD - top + 8.0), WAct::App(id.clone())));
        let name = x.names.get(id).cloned().unwrap_or_default();
        c.text_base(&name, cx, 184.0, style::FOOT.size, 400, c.text2(), Align::Center, cw - 4.0);
    }
}

/// Screenshot: Region (the primary action, blue), Window and Full screen.
fn screenshot(c: &Canvas, out: &mut Painted, sh: Shape) {
    let items: [(&str, &str, &str); 4] = [
        (G_REGION, "Region", "ms-screenclip:?clippingMode=Rectangle"),
        (G_WINDOW, "Window", "ms-screenclip:?clippingMode=Window"),
        (G_FULL, "Full screen", "ms-screenclip:?clippingMode=Fullscreen"),
        (G_FOLDER, "Screenshots", "shell:Screenshots"),
    ];
    let fill = |i: usize| (i == 0).then(|| c.blue());
    match sh {
        Shape::Mini => {
            let (mx, my, ms) = mark_box(c, sh);
            mark(c, items[0].0, mx, my, ms, false, fill(0));
            out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open(items[0].2.into())));
        }
        Shape::Dock => {
            let k = dock_k(c);
            let m = style::M_DOCK * k;
            let s = style::BADGE_DOCK * k;
            let n = if dock_wide(c) { 4 } else { 3 };
            let gap = (c.w - 2.0 * m - n as f32 * s) / (n - 1) as f32;
            for (i, (g, _, uri)) in items.iter().take(n).enumerate() {
                let x0 = m + i as f32 * (s + gap);
                mark(c, g, x0, (c.h - s) / 2.0, s, false, fill(i));
                out.hits.push(((x0 - gap / 2.0, 0.0, s + gap, c.h), WAct::Open((*uri).into())));
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let (rh, ps) = (28.0, 24.0);
            for (i, (g, l, uri)) in items.iter().take(3).enumerate() {
                let y = m + i as f32 * (rh + style::SP[1]);
                let cy = y + rh / 2.0;
                mark(c, g, m, cy - ps / 2.0, ps, false, fill(i));
                let tx = m + ps + style::SP[3];
                c.text_base(l, tx, cy + c.cap_h(style::BODY.size) / 2.0, style::BODY.size, 500, c.text1(), Align::Left, c.w - m - tx);
                out.hits.push(((0.0, y - style::SP[0], c.w, rh + style::SP[1]), WAct::Open((*uri).into())));
            }
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let cw = (c.w - 2.0 * m) / 3.0;
            let s = MARK_CARD;
            for (i, (g, l, uri)) in items.iter().take(3).enumerate() {
                let cx = m + cw * (i as f32 + 0.5);
                mark(c, g, cx - s / 2.0, m, s, false, fill(i));
                c.text_base(l, cx, 76.0, style::FOOT.size, 500, c.text2(), Align::Center, cw);
                out.hits.push(((cx - cw / 2.0, 0.0, cw, style::SEP_Y_LARGE), WAct::Open((*uri).into())));
            }
            if sh == Shape::Large {
                tool_rows(
                    c,
                    out,
                    &[
                        row(G_FOLDER, "Screenshots folder", Trail::Go(WAct::Open("shell:Screenshots".into()))),
                        row(G_CUT, "Snipping Tool", Trail::Go(WAct::Open("Microsoft.ScreenSketch_8wekyb3d8bbwe!App".into()))),
                        row(G_KEYBOARD, "Snip from anywhere", Trail::Key("Win + Shift + S")),
                    ],
                );
            }
        }
    }
}

/// A placeholder app tile; the first one carries a "+" that opens Launchpad to pin apps.
fn app_slot(c: &Canvas, out: &mut Painted, x: f32, y: f32, s: f32, plus: bool) {
    c.rr(x, y, s, s, s * 10.0 / MARK_CARD, c.plate());
    if plus {
        c.glyph(G_ADD, x + s / 2.0, y + s / 2.0, s * 16.0 / MARK_CARD, c.text3());
        out.hits.push(((x, y, s, s), WAct::Search));
    }
}

/// Apps: the Launchpad pins on a 44 / 12 grid; empty slots are placeholders.
fn apps_grid(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let pins = x.pinned;
    let gap = 12.0;
    // (cols, rows, s, left, top) of the grid; Medium is one row of four 59-wide columns.
    let (cols, rows, s, left, top) = match sh {
        Shape::Mini => {
            let s = MARK_MINI * dock_k(c);
            (1, 1, s, (c.w - s) / 2.0, (c.h - s) / 2.0)
        }
        Shape::Dock => {
            let k = dock_k(c);
            let s = MARK_MINI * k;
            let g = style::SP[3] * k;
            let n = (((c.w - 2.0 * style::M_MINI * k + g) / (s + g)).floor() as usize).max(1);
            // The pins and one "+" at most: the dock never shows a run of empty tiles.
            let n = n.min(pins.len() + 1);
            let w = n as f32 * s + (n - 1) as f32 * g;
            let left = if pins.is_empty() { style::M_MINI * k } else { (c.w - w) / 2.0 };
            for i in 0..n {
                let ix = left + i as f32 * (s + g);
                match pins.get(i) {
                    Some(id) => {
                        out.icons.push((ix, (c.h - s) / 2.0, s, id.clone()));
                        out.hits.push(((ix - g / 2.0, 0.0, s + g, c.h), WAct::App(id.clone())));
                    }
                    None => app_slot(c, out, ix, (c.h - s) / 2.0, s, i == pins.len()),
                }
            }
            if pins.is_empty() {
                let x0 = left + s + style::SP[4] * k;
                c.text_base("Pin apps", x0, 26.0 * k, style::TITLE.size * k, 600, c.text1(), Align::Left, c.w - x0 - style::M_DOCK * k);
                c.text_base("In Launchpad", x0, 40.0 * k, style::FOOT.size * k, 400, c.text2(), Align::Left, c.w - x0 - style::M_DOCK * k);
            }
            return;
        }
        Shape::Small => {
            if pins.is_empty() {
                (2, 1, MARK_CARD, style::M_SMALL, SMALL_MARK_Y)
            } else {
                (2, 2, MARK_CARD, style::M_SMALL, 10.0)
            }
        }
        Shape::Medium => {
            if pins.is_empty() {
                (2, 1, MARK_CARD, style::M_CARD, MED_MARK_Y)
            } else {
                // Four columns, each tile centred in its 59-wide column, with names.
                let m = style::M_CARD;
                let cw = (c.w - 2.0 * m) / 4.0;
                for i in 0..4 {
                    let cx = m + cw * (i as f32 + 0.5);
                    let ix = cx - MARK_CARD / 2.0;
                    match pins.get(i) {
                        Some(id) => {
                            out.icons.push((ix, m, MARK_CARD, id.clone()));
                            out.hits.push(((cx - cw / 2.0, 0.0, cw, c.h), WAct::App(id.clone())));
                            let name = x.names.get(id).cloned().unwrap_or_default();
                            c.text_base(&name, cx, 76.0, style::FOOT.size, 400, c.text2(), Align::Center, cw - 4.0);
                        }
                        None => app_slot(c, out, ix, m, MARK_CARD, i == pins.len()),
                    }
                }
                return;
            }
        }
        Shape::Large => {
            let rows = if pins.is_empty() { 2 } else { 3 };
            let w = 4.0 * MARK_CARD + 3.0 * gap;
            let h = rows as f32 * MARK_CARD + (rows - 1) as f32 * gap;
            let top = if pins.is_empty() { 24.0 } else { (c.h - h) / 2.0 };
            (4, rows, MARK_CARD, (c.w - w) / 2.0, top)
        }
    };
    for i in 0..cols * rows {
        let (ix, iy) = (left + (i % cols) as f32 * (s + gap), top + (i / cols) as f32 * (s + gap));
        match pins.get(i) {
            Some(id) => {
                out.icons.push((ix, iy, s, id.clone()));
                out.hits.push(((ix - gap / 2.0, iy - gap / 2.0, s + gap, s + gap), WAct::App(id.clone())));
            }
            None => app_slot(c, out, ix, iy, s, i == pins.len()),
        }
    }
    if !pins.is_empty() {
        return;
    }
    // No pins: say how to add them, under (or beside) the placeholders.
    match sh {
        Shape::Small => {
            c.text_base("Pin apps", c.w / 2.0, SMALL_NAME_BASE, style::TITLE.size, 600, c.text1(), Align::Center, c.w);
            c.text_base("In Launchpad", c.w / 2.0, SMALL_STATE_BASE, style::FOOT.size, 400, c.text2(), Align::Center, c.w);
        }
        Shape::Medium => {
            let tx = left + 2.0 * s + gap + style::SP[6];
            c.text_base("Pin apps", tx, MED_NAME_BASE, style::VALUE.size, 600, c.text1(), Align::Left, c.w - style::M_CARD - tx);
            c.text_base("In Launchpad", tx, MED_STATE_BASE, style::CAPTION.size, 400, c.text2(), Align::Left, c.w - style::M_CARD - tx);
        }
        Shape::Large => {
            c.text_base("No apps pinned", c.w / 2.0, 154.0, style::TITLE.size, 600, c.text1(), Align::Center, c.w);
            c.text_base("Pin apps in Launchpad to see them here", c.w / 2.0, 172.0, style::CAPTION.size, 400, c.text2(), Align::Center, c.w - 2.0 * style::M_CARD);
        }
        _ => {}
    }
}

/// The Notes tile: yellow, with a dark glyph, the same in both themes.
fn note_tile(c: &Canvas, x: f32, y: f32, s: f32) {
    c.rr(x, y, s, s, s * style::BADGE_R, c.yellow());
    c.glyph(G_NOTE, x + s / 2.0, y + s / 2.0, s * 0.5, alpha(hex(0x1C1C1E), 0.85));
}

/// Notes: the first line of your notes file and what follows it.
fn notes(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Notes));
    let lines: Vec<&str> = x.hub.notes.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let empty = lines.is_empty();
    match sh {
        Shape::Mini => {
            let (mx, my, ms) = mark_box(c, sh);
            note_tile(c, mx, my, ms);
        }
        Shape::Dock => {
            let k = dock_k(c);
            let (mx, my, ms) = mark_box(c, sh);
            note_tile(c, mx, my, ms);
            let m = style::M_DOCK * k;
            let x0 = mx + ms + style::SP[4] * k;
            let mut right = c.w - m;
            if dock_wide(c) {
                let slot = if empty { ToolSlot::Note("New note".into()) } else { ToolSlot::Note(if lines.len() == 1 { "1 line".into() } else { format!("{} lines", lines.len()) }) };
                if let Some(w) = tool_slot(c, &slot, c.w - m, 27.0 * k, k, false) {
                    right -= w + style::SP[5] * k;
                }
            }
            let (first, second) = if empty { ("Notes", "Click to write") } else { (lines[0], lines.get(1).copied().unwrap_or("Notes")) };
            c.text_base(first, x0, 26.0 * k, style::TITLE.size * k, 600, c.text1(), Align::Left, right - x0);
            c.text_base(second, x0, 40.0 * k, style::FOOT.size * k, 400, c.text2(), Align::Left, right - x0);
        }
        Shape::Small => {
            let m = style::M_SMALL;
            note_tile(c, m, m, style::BADGE_SMALL);
            let inner = c.w - 2.0 * m;
            if empty {
                c.text_base("No notes", m, SMALL_NAME_BASE, style::TITLE.size, 600, c.text1(), Align::Left, inner);
                c.text_base("Click to write", m, SMALL_STATE_BASE, style::CAPTION.size, 400, c.text2(), Align::Left, inner);
                return;
            }
            c.text_cap(lines[0], m, 52.0, style::TITLE.size, 600, c.text1(), Align::Left, inner);
            let first = 52.0 + c.cap_h(style::TITLE.size) + style::ROW_S - 2.0;
            if lines.len() == 1 {
                c.text_base("Click to add more", m, first, style::CAPTION.size, 400, c.text3(), Align::Left, inner);
            }
            for (i, l) in lines.iter().skip(1).enumerate() {
                let base = first + i as f32 * 16.0;
                if base > c.h - m {
                    break;
                }
                c.text_base(l, m, base, style::CAPTION.size, 400, c.text2(), Align::Left, inner);
            }
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            if empty && sh == Shape::Large {
                let s = 48.0;
                note_tile(c, (c.w - s) / 2.0, 36.0, s);
                c.text_base("No notes yet", c.w / 2.0, 108.0, 15.0, 600, c.text1(), Align::Center, c.w);
                c.text_base("Click to write a note", c.w / 2.0, 128.0, style::CAPTION.size, 400, c.text2(), Align::Center, c.w - 2.0 * m);
                let (bw, bh) = (132.0, 30.0);
                pill_button(c, out, (c.w - bw) / 2.0, 150.0, bw, bh, "New note", WAct::Notes);
                return;
            }
            if empty {
                note_tile(c, m, m, style::BADGE_GLANCE);
                c.text_cap("No notes", m, 50.0, 15.0, 600, c.text1(), Align::Left, c.w - 2.0 * m);
                c.text_base("Click to write a note", m, 76.0, style::CAPTION.size, 400, c.text2(), Align::Left, c.w - 2.0 * m);
                return;
            }
            let bs = style::BADGE_HEADER;
            note_tile(c, m, m, bs);
            let tx = m + bs + style::SP[3];
            c.text_base(lines[0], tx, style::HEADER_BASE, style::TITLE.size, 600, c.text1(), Align::Left, c.w - m - tx);
            let (pitch, last) = if sh == Shape::Large { (18.0, c.h - m) } else { (18.0, 80.0) };
            let fits = (((last - 56.0) / pitch).floor() as usize) + 1;
            let body: Vec<&&str> = lines.iter().skip(1).collect();
            if body.is_empty() {
                c.text_base("Click to add more", m, 56.0, style::BODY.size, 400, c.text3(), Align::Left, c.w - 2.0 * m);
            }
            for (i, l) in body.iter().take(fits).enumerate() {
                // More lines than fit: the last one fades instead of stopping short.
                let col = if i + 1 == fits && body.len() > fits { c.text3() } else { c.text2() };
                c.text_base(l, m, 56.0 + i as f32 * pitch, style::BODY.size, 400, col, Align::Left, c.w - 2.0 * m);
            }
        }
    }
}

/// Screen Time: today's total and the apps it went to (Large adds when).
fn screen_time(c: &Canvas, x: &Ctx, sh: Shape) {
    let apps = screen_apps(x);
    let total: u64 = apps.iter().map(|a| a.1).sum();
    let none = total == 0;
    let value = if none { "0m".to_string() } else { dur(total) };
    match sh {
        Shape::Mini => {
            let k = dock_k(c);
            c.glyph(G_SCREEN, c.w / 2.0, (8.0 + 9.0) * k, 18.0 * k, BLUE);
            let role = Type { size: style::MINI_VALUE.size * k, ..style::MINI_VALUE };
            let (v, fs) = fit_value(c, &dur_mini(total), role, c.w - 2.0 * style::M_MINI * k);
            c.text_base(&v, c.w / 2.0, 45.0 * k, fs, role.weight, c.text1(), Align::Center, c.w * 2.0);
        }
        Shape::Dock => {
            let k = dock_k(c);
            let m = style::M_DOCK * k;
            let (mx, my, ms) = mark_box(c, sh);
            plate(c, G_SCREEN, mx, my, ms, c.plate(), BLUE);
            let x0 = mx + ms + style::SP[4] * k;
            let mut right = c.w - m;
            if dock_wide(c) {
                let sw = (c.w * 0.34).round();
                let sx = c.w - m - sw;
                if let Some((name, secs)) = apps.first() {
                    let pct = format!("{:.0}%", *secs as f64 * 100.0 / total.max(1) as f64);
                    let pw = c.text_base(&pct, c.w - m, 23.0 * k, style::FOOT.size * k, 500, c.text3(), Align::Right, sw);
                    c.text_base(name, sx, 23.0 * k, style::FOOT.size * k, 500, c.text2(), Align::Left, sw - pw - style::SP[1] * k);
                }
                share_bar(c, &apps, total, sx, 29.0 * k, sw, style::METER_H_COMPACT * k);
                right = sx - style::SP[5] * k;
            }
            let role = Type { size: style::VALUE.size * k, ..style::VALUE };
            let (v, fs) = fit_value(c, &value, role, right - x0);
            c.text_base(&v, x0, 30.0 * k, fs, role.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base("Today", x0, 44.0 * k, style::FOOT.size * k, 400, c.text2(), Align::Left, right - x0);
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let inner = c.w - 2.0 * m;
            let (v, fs) = fit_value(c, &value, style::HERO_S, inner);
            c.text_cap(&v, m, m, fs, style::HERO_S.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base(if none { "Starting now" } else { "Today" }, m, 54.0, style::CAPTION.size, 400, c.text2(), Align::Left, inner);
            share_bar(c, &apps, total, m, 64.0, inner, style::METER_H);
            for (i, (name, secs)) in apps.iter().take(2).enumerate() {
                let base = 88.0 + i as f32 * style::ROW_S;
                let cy = base - c.cap_h(style::FOOT.size) / 2.0;
                c.circle(m + 3.0, cy, 3.0, APP_COLS[i]);
                let tw = c.text_base(&dur(*secs), c.w - m, base, style::FOOT.size, 500, c.text2(), Align::Right, inner);
                let nx = m + 10.0;
                c.text_base(name, nx, base, style::FOOT.size, 500, c.text1(), Align::Left, c.w - m - tw - style::SP[2] - nx);
            }
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let lw = 132.0 - m;
            let (v, fs) = fit_value(c, &value, style::HERO, lw);
            c.text_cap(&v, m, m, fs, style::HERO.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base(if none { "Counting from now" } else { "Screen time today" }, m, 58.0, style::CAPTION.size, 400, c.text2(), Align::Left, lw);
            share_bar(c, &apps, total, m, 70.0, lw, style::METER_H);
            let (rx, rw) = (148.0, c.w - m - 148.0);
            let first = m + c.cap_h(style::FOOT.size);
            if apps.is_empty() {
                c.text_base("No apps yet", rx, first, style::FOOT.size, 400, c.text3(), Align::Left, rw);
            }
            for (i, (name, secs)) in apps.iter().take(3).enumerate() {
                let base = first + i as f32 * style::ROW_L;
                let tw = c.text_base(&dur(*secs), c.w - m, base, style::FOOT.size, 500, c.text2(), Align::Right, rw);
                c.text_base(name, rx, base, style::FOOT.size, 500, c.text1(), Align::Left, rw - tw - style::SP[2]);
                c.bar(rx, base + 5.0, rw, 3.0, *secs as f32 / total.max(1) as f32, APP_COLS[i]);
            }
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let vals = hours_of(x);
                let w = c.w - 2.0 * m;
                let bw = (w / 24.0 * 0.6).round();
                hour_bars(c, &vals, x.now.hour as usize, 3600.0, m, style::DETAIL_TOP, w, 64.0, bw);
                hour_ticks(c, &[(0, "12a"), (6, "6a"), (12, "12p"), (18, "6p")], m, w, bw, 186.0);
            }
        }
    }
}

/// Day Timeline: when you used the PC today, hour by hour.
fn timeline(c: &Canvas, x: &Ctx, sh: Shape) {
    let vals = hours_of(x);
    let total = x.hub.screen.total();
    let now = x.now.hour as usize;
    let value = if total == 0 { "0m".to_string() } else { dur(total) };
    let ticks4: [(usize, &str); 4] = [(0, "12a"), (6, "6a"), (12, "12p"), (18, "6p")];
    match sh {
        Shape::Mini => {
            // Twelve two-hour buckets, drawn as the icon.
            let k = dock_k(c);
            let b: Vec<f64> = (0..12).map(|i| vals[2 * i] + vals[2 * i + 1]).collect();
            let (w, h) = (40.0 * k, 24.0 * k);
            if total == 0 {
                c.glyph(G_CLOCK, c.w / 2.0, c.h / 2.0, 22.0 * k, BLUE);
                return;
            }
            hour_bars(c, &b, now / 2, 7200.0, (c.w - w) / 2.0, (c.h - h) / 2.0, w, h, 2.5 * k);
        }
        Shape::Dock => {
            let k = dock_k(c);
            let m = style::M_DOCK * k;
            let (mx, my, ms) = mark_box(c, sh);
            plate(c, G_CLOCK, mx, my, ms, c.plate(), BLUE);
            let x0 = mx + ms + style::SP[4] * k;
            let mut right = c.w - m;
            if dock_wide(c) {
                let sw = (c.w * 0.34).round();
                let sx = c.w - m - sw;
                hour_bars(c, &vals, now, 3600.0, sx, 14.0 * k, sw, 26.0 * k, (sw / 24.0 * 0.7).max(1.0));
                right = sx - style::SP[5] * k;
            }
            let role = Type { size: style::VALUE.size * k, ..style::VALUE };
            let (v, fs) = fit_value(c, &value, role, right - x0);
            c.text_base(&v, x0, 30.0 * k, fs, role.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base("Today", x0, 44.0 * k, style::FOOT.size * k, 400, c.text2(), Align::Left, right - x0);
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let inner = c.w - 2.0 * m;
            let (v, fs) = fit_value(c, &value, style::HERO_S, inner);
            c.text_cap(&v, m, m, fs, style::HERO_S.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base("Today", m, 54.0, style::CAPTION.size, 400, c.text2(), Align::Left, inner);
            let bw = 3.0;
            hour_bars(c, &vals, now, 3600.0, m, 64.0, inner, 28.0, bw);
            hour_ticks(c, &[(0, "12a"), (12, "12p")], m, inner, bw, 106.0);
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let badge = super::canvas::gly(G_CLOCK, 0x0A84FF, 0xFFFFFF);
            header(c, &badge, "Today", Some((value.as_str(), c.text1())));
            let w = c.w - 2.0 * m;
            let bw = (w / 24.0 * 0.6).round();
            hour_bars(c, &vals, now, 3600.0, m, style::CONTENT_TOP, w, 22.0, bw);
            hour_ticks(c, &ticks4, m, w, bw, 79.0);
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let used: Vec<usize> = (0..24).filter(|h| vals[*h] > 0.0).collect();
                match (used.first(), used.is_empty()) {
                    (Some(first), false) => {
                        let busiest = (0..24).max_by(|a, b| vals[*a].partial_cmp(&vals[*b]).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0);
                        let (a, b, n) = (hour_str(*first), hour_str(busiest), format!("{}h", used.len()));
                        stat_footer(c, [(&a, "First use"), (&b, "Busiest hour"), (&n, "Active hours")], 132.0);
                    }
                    _ => {
                        c.text_base("Counting from now", c.w / 2.0, 152.0, style::CAPTION.size, 400, c.text2(), Align::Center, c.w - 2.0 * m);
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Control Center

const G_CHECK: &str = "\u{E73E}";
const G_APPS: &str = "\u{E71D}";

/// Is a dock pill the wide one (Dock L), which adds an accessory slot? Same rule as `compact_sum`.
fn cc_wide(c: &Canvas) -> bool {
    c.w >= c.h * 3.4
}

/// The Dock L accessory slot: (x, width), scaled to the pill.
fn cc_slot(c: &Canvas) -> (f32, f32) {
    let k = c.h / 54.0;
    let w = (c.w * 0.34).round();
    (c.w - style::M_DOCK * k - w, w)
}

/// On/off colours (§2.3): on = blue fill + white ink, off = plate + text1 ink.
fn cc_fill(c: &Canvas, on: bool) -> (Col, Col) {
    if on {
        (BLUE, WHITE)
    } else {
        (c.plate(), c.text1())
    }
}

/// A round radio button: blue when on, plate when off, plate with a faded glyph when the
/// radio isn't there (`None`). `d` is its diameter.
fn round_toggle(c: &Canvas, cx: f32, cy: f32, d: f32, g: &str, on: Option<bool>) {
    let (fill, ink) = cc_fill(c, on == Some(true));
    let ink = if on.is_none() { alpha(c.text1(), 0.5) } else { ink };
    c.circle(cx, cy, d / 2.0, fill);
    c.glyph(g, cx, cy, d * style::GLYPH_IN, ink);
}

/// A rounded-square glyph plate (radius 0.25 × size, glyph 0.45 × size).
fn cc_plate(c: &Canvas, g: &str, x: f32, y: f32, s: f32, fill: Col, ink: Col) {
    c.rr(x, y, s, s, s * style::BADGE_R, fill);
    c.glyph(g, x + s / 2.0, y + s / 2.0, s * style::GLYPH_IN, ink);
}

/// A red count badge (diameter `d`) with a card-coloured ring, centred on (cx, cy) but
/// kept 3 px inside the card. No number draws a plain attention dot.
fn count_badge(c: &Canvas, cx: f32, cy: f32, d: f32, n: Option<u32>) {
    let ring = 1.5;
    let r = d / 2.0;
    let edge = style::SP[1] + 1.0;
    let cx = cx.min(c.w - edge - r - ring);
    let cy = cy.max(edge + r + ring);
    c.circle(cx, cy, r + ring, c.halo());
    c.circle(cx, cy, r, RED);
    if let Some(n) = n {
        let t = if n > 9 { "9+".to_string() } else { n.to_string() };
        let fs = style::MICRO.size;
        c.text_cap(&t, cx, cy - c.cap_h(fs) / 2.0, fs, 700, WHITE, Align::Center, d * 2.0);
    }
}

/// A right chevron in text3 whose ink ends at `right`, centred on `cy`.
fn chevron(c: &Canvas, right: f32, cy: f32) {
    c.glyph(G_CHEV, right - 3.0, cy, 10.0, c.text3());
}

/// Large detail rows at the 24 pitch from `top`: Body label at the left, then either a
/// trailing footnote (shortcuts) or a chevron (links). Each row is a hit area.
fn link_rows(c: &Canvas, out: &mut Painted, items: Vec<(&str, &str, WAct)>, top: f32) {
    let m = style::M_CARD;
    let ls = style::BODY.size;
    for (i, (label, trailing, act)) in items.into_iter().enumerate() {
        let y = top + i as f32 * style::ROW_L;
        let base = y + (style::ROW_L + c.cap_h(ls)) / 2.0;
        let mut right = c.w - m;
        if trailing.is_empty() {
            chevron(c, right, base - c.cap_h(ls) / 2.0);
            right -= 10.0 + style::SP[3];
        } else {
            let tw = c.text_base(trailing, right, base, style::FOOT.size, 500, c.text2(), Align::Right, c.w * 0.5);
            right -= tw + style::SP[3];
        }
        c.text_base(label, m, base, ls, style::BODY.weight, c.text1(), Align::Left, right - m);
        out.hits.push(((style::SP[3], y, c.w - 2.0 * style::SP[3], style::ROW_L), act));
    }
}

/// The volume, as "64%" (or "Muted"), and the meter fraction (0 when muted).
fn volume_text(x: &Ctx) -> Option<(String, f32)> {
    let v = x.control.volume?;
    if x.control.muted == Some(true) {
        Some(("Muted".to_string(), 0.0))
    } else {
        Some((format!("{:.0}%", (v * 100.0).clamp(0.0, 100.0)), v))
    }
}

/// Large volume slider row centred on `cy`: speaker (mutes), track (sets), percentage.
fn volume_row(c: &Canvas, out: &mut Painted, x: &Ctx, cy: f32) {
    let m = style::M_CARD;
    let muted = x.control.muted == Some(true);
    let g = if muted { G_MUTE } else { G_VOL };
    c.glyph(g, m + 8.0, cy, 16.0, c.text2());
    let fs = style::FOOT.size;
    let base = cy + c.cap_h(fs) / 2.0;
    let (tx0, tx1) = (m + 28.0, c.w - m - 40.0);
    match volume_text(x) {
        Some((pct, f)) => {
            out.hits.push(((m - 6.0, cy - 14.0, 28.0, 28.0), WAct::Mute));
            c.bar(tx0, cy - style::METER_H / 2.0, tx1 - tx0, style::METER_H, f, BLUE);
            c.text_base(&pct, c.w - m, base, fs, 500, c.text2(), Align::Right, 40.0);
            out.hits.push(((tx0, cy - 12.0, tx1 - tx0, 24.0), WAct::Volume));
        }
        None => {
            c.text_base("Volume unavailable", tx0, base, fs, 400, c.text2(), Align::Left, c.w - m - tx0);
        }
    }
}

/// Wi-Fi and Bluetooth: the round button switches the radio; the name opens its list.
fn connectivity(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let ctl = x.control;
    let state = |on: Option<bool>| match on {
        Some(true) => "On",
        Some(false) => "Off",
        None => "Unavailable",
    };
    let rows = [
        (G_WIFI, "Wi-Fi", ctl.wifi, WAct::Wifi, WAct::WifiMenu),
        (G_BT, "Bluetooth", ctl.bluetooth, WAct::Bluetooth, WAct::Open("ms-settings:bluetooth".into())),
    ];
    let k = c.h / 54.0;
    match sh {
        Shape::Mini => {
            // One mark: the Wi-Fi button.
            let (g, _, on, act, _) = rows.into_iter().next().unwrap();
            round_toggle(c, c.w / 2.0, c.h / 2.0, 40.0 * k, g, on);
            if on.is_some() {
                out.hits.push(((0.0, 0.0, c.w, c.h), act));
            }
        }
        Shape::Dock => {
            let m = style::M_DOCK * k;
            let d = style::BADGE_DOCK * k;
            let half = c.w / 2.0;
            let wide = cc_wide(c);
            for (i, (g, name, on, act, more)) in rows.into_iter().enumerate() {
                let x0 = half * i as f32;
                if !wide {
                    // Dock M: each button centred in its half.
                    round_toggle(c, x0 + half / 2.0, c.h / 2.0, d, g, on);
                    if on.is_some() {
                        out.hits.push(((x0, 0.0, half, c.h), act));
                    }
                    continue;
                }
                // Dock L: each half gains its name and state.
                let cx = x0 + m + d / 2.0;
                round_toggle(c, cx, c.h / 2.0, d, g, on);
                let tx = x0 + m + d + style::SP[3] * k;
                let tw = x0 + half - tx - style::SP[2] * k;
                let ns = fit(c, name, style::TITLE.size * 12.0 / 13.0 * k, style::FOOT.size * k, 600, Font::Text, tw);
                c.text_base(name, tx, 29.0 * k, ns, 600, c.text1(), Align::Left, tw);
                let ss = fit(c, state(on), style::FOOT.size * k, style::MICRO.size * k, 400, Font::Text, tw);
                c.text_base(state(on), tx, 43.0 * k, ss, 400, c.text2(), Align::Left, tw + 2.0);
                out.hits.push(((tx - style::SP[1] * k, 0.0, x0 + half - tx + style::SP[1] * k, c.h), more));
                if on.is_some() {
                    out.hits.push(((x0, 0.0, tx - style::SP[1] * k - x0, c.h), act));
                }
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let d = 32.0;
            let tx = m + d + style::SP[3];
            let tw = c.w - m - tx;
            for (i, (g, name, on, act, more)) in rows.into_iter().enumerate() {
                let top = if i == 0 { m } else { c.h - m - d };
                let cy = top + d / 2.0;
                round_toggle(c, m + d / 2.0, cy, d, g, on);
                let ns = fit(c, name, style::TITLE.size, 12.0, 600, Font::Text, tw);
                // Name and state as one block, centred on the button.
                let gap = 15.0;
                let nb = cy - (c.cap_h(ns) + gap) / 2.0 + c.cap_h(ns);
                c.text_base(name, tx, nb, ns, 600, c.text1(), Align::Left, tw + 4.0);
                c.text_base(state(on), tx, nb + gap, style::FOOT.size, 400, c.text2(), Align::Left, tw);
                out.hits.push(((tx - style::SP[1], top - style::SP[2], c.w - tx + style::SP[1], d + style::SP[4]), more));
                if on.is_some() {
                    out.hits.push(((m - style::SP[1], top - style::SP[1], d + style::SP[3], d + style::SP[3]), act));
                }
            }
        }
        Shape::Medium | Shape::Large => {
            // Two columns side by side; Large keeps this row as it is and adds links.
            let m = style::M_CARD;
            let d = 36.0;
            let cy = 48.0;
            // Column B starts just right of centre, so the two read as a pair.
            let xs = [m, c.w / 2.0 + style::SP[3]];
            for (i, (g, name, on, act, more)) in rows.into_iter().enumerate() {
                let x0 = xs[i];
                let col_w = if i == 0 { xs[1] - m } else { c.w - m - xs[1] };
                round_toggle(c, x0 + d / 2.0, cy, d, g, on);
                let tx = x0 + d + style::SP[3];
                let tw = x0 + col_w - tx - if i == 0 { style::SP[3] } else { 0.0 };
                c.text_base(name, tx, 46.0, style::TITLE.size, 600, c.text1(), Align::Left, tw);
                let st = state(on);
                let cs = style::CAPTION.size;
                let sw = c.text_base(st, tx, 62.0, cs, 400, c.text2(), Align::Left, tw - 12.0);
                chevron(c, tx + sw + style::SP[1] + 6.0, 62.0 - c.cap_h(cs) / 2.0);
                out.hits.push(((tx - style::SP[1], cy - 24.0, x0 + col_w - tx + style::SP[1], 48.0), more));
                if on.is_some() {
                    out.hits.push(((x0 - style::SP[1], cy - d / 2.0 - style::SP[1], d + style::SP[3], d + style::SP[3]), act));
                }
            }
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                link_rows(
                    c,
                    out,
                    vec![
                        ("Network & internet", "", WAct::Open("ms-settings:network".into())),
                        ("Bluetooth & devices", "", WAct::Open("ms-settings:bluetooth".into())),
                        ("Airplane mode", "", WAct::Open("ms-settings:network-airplanemode".into())),
                    ],
                    style::DETAIL_TOP,
                );
            }
        }
    }
}

/// Control tiles (Quick Toggles, Quick Controls): one tile on the Mini, a row of glyph
/// tiles in the dock (plus volume on Dock L), 2×2 on Small, a labelled row on Medium, and
/// on Large that row with a volume slider and `extra` rows under it.
fn button_grid(c: &Canvas, out: &mut Painted, x: &Ctx, items: Vec<(&'static str, &'static str, bool, WAct)>, extra: Vec<(&str, &str, WAct)>, sh: Shape) {
    let k = c.h / 54.0;
    let tile = |tx: f32, ty: f32, tw: f32, th: f32, r: f32, g: &str, gs: f32, gy: f32, on: bool| {
        let (fill, ink) = cc_fill(c, on);
        c.rr(tx, ty, tw, th, r, fill);
        c.glyph(g, tx + tw / 2.0, gy, gs, ink);
    };
    match sh {
        Shape::Mini => {
            if let Some((g, _, on, act)) = items.into_iter().next() {
                let s = 40.0 * k;
                let (tx, ty) = ((c.w - s) / 2.0, (c.h - s) / 2.0);
                tile(tx, ty, s, s, 10.0 * k, g, 18.0 * k, ty + s / 2.0, on);
                out.hits.push(((0.0, 0.0, c.w, c.h), act));
            }
        }
        Shape::Dock => {
            let m = style::M_DOCK * k;
            let s = style::BTN_SECONDARY_DOCK * k;
            let gap = style::SP[1] * k;
            let ty = (c.h - s) / 2.0;
            for (i, (g, _, on, act)) in items.into_iter().enumerate() {
                let tx = m + i as f32 * (s + gap);
                tile(tx, ty, s, s, style::R_MIN * k, g, s * style::GLYPH_IN, ty + s / 2.0, on);
                out.hits.push(((tx - gap / 2.0, 0.0, s + gap, c.h), act));
            }
            if cc_wide(c) {
                if let Some((pct, f)) = volume_text(x) {
                    let (sx, sw) = cc_slot(c);
                    let fs = style::FOOT.size * k;
                    let base = 26.0 * k;
                    let g = if x.control.muted == Some(true) { G_MUTE } else { G_VOL };
                    c.glyph(g, sx + 6.0 * k, base - c.cap_h(fs) / 2.0, fs, c.text2());
                    c.text_base(&pct, sx + sw, base, fs, 500, c.text2(), Align::Right, sw - 16.0 * k);
                    c.bar(sx, 32.0 * k, sw, style::METER_H_COMPACT * k, f, BLUE);
                    out.hits.push(((sx, 0.0, sw, c.h), WAct::Volume));
                }
            }
        }
        Shape::Small => {
            let inset = style::SP[3];
            let gap = style::SP[3];
            let r = style::concentric(style::CARD_R, inset);
            let tw = (c.w - 2.0 * inset - gap) / 2.0;
            let th = (c.h - 2.0 * inset - gap) / 2.0;
            for (i, (g, _, on, act)) in items.into_iter().enumerate() {
                let (tx, ty) = (inset + (i % 2) as f32 * (tw + gap), inset + (i / 2) as f32 * (th + gap));
                tile(tx, ty, tw, th, r, g, 20.0, ty + th / 2.0, on);
                out.hits.push(((tx, ty, tw, th), act));
            }
        }
        Shape::Medium | Shape::Large => {
            // Inset 6 → radius 12 (§2.1), gap 6: four tiles about 59.5 wide.
            let inset = style::SP[2];
            let gap = style::SP[2];
            let r = style::concentric(style::CARD_R, inset);
            let n = items.len().max(1);
            let tw = (c.w - 2.0 * inset - gap * (n - 1) as f32) / n as f32;
            // Medium's tile row; Large draws it unchanged in its top 96.
            let th = style::SEP_Y_LARGE - 2.0 * inset;
            // Every label keeps 6 px of air to each tile edge.
            let room = tw - 2.0 * style::SP[2];
            let labels: Vec<TileLabel> = items.iter().map(|it| tile_label(c, it.1, room)).collect();
            // One vertical rhythm per row, so glyphs and first lines align across tiles:
            // a two-line row lifts the glyph to make room for the second line.
            let two = labels.iter().any(|l| l.lines.len() > 1);
            let (gy, base) = if two { (32.0, 60.0) } else { (38.0, 67.0) };
            let pitch = 13.0;
            for (i, ((g, _, on, act), l)) in items.into_iter().zip(labels).enumerate() {
                let (tx, ty) = (inset + i as f32 * (tw + gap), inset);
                tile(tx, ty, tw, th, r, g, 20.0, gy, on);
                let ink = if on { WHITE } else { c.text1() };
                for (j, line) in l.lines.iter().enumerate() {
                    c.text_base(line, tx + tw / 2.0, base + j as f32 * pitch, l.size, l.weight, ink, Align::Center, room + 1.0);
                }
                out.hits.push(((tx, ty, tw, th), act));
            }
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                volume_row(c, out, x, style::DETAIL_TOP + style::ROW_L / 2.0);
                link_rows(c, out, extra, style::DETAIL_TOP + style::ROW_L + style::SP[1]);
            }
        }
    }
}

/// A toggle tile's label, laid out to fit `room` without cutting a word.
struct TileLabel {
    lines: Vec<String>,
    size: f32,
    weight: u32,
}

/// One line at Footnote 11/500 when it fits; otherwise two lines split at the space
/// nearest the middle ("Dark" / "mode"); a single long word shrinks to 10, then drops to
/// weight 400. The ink always stays inside `room`.
fn tile_label(c: &Canvas, s: &str, room: f32) -> TileLabel {
    let wide = |t: &str, size: f32, weight: u32| c.measure(t, size, weight, Font::Text).0;
    let top = style::FOOT.size;
    let min = style::MICRO.size;
    let lines: Vec<String> = if wide(s, top, 500) <= room {
        vec![s.to_string()]
    } else {
        let mid = s.len() / 2;
        match s.match_indices(' ').map(|(i, _)| i).min_by_key(|i| i.abs_diff(mid)) {
            Some(i) => vec![s[..i].to_string(), s[i + 1..].to_string()],
            None => vec![s.to_string()],
        }
    };
    let widest = |size: f32, weight: u32| lines.iter().map(|l| wide(l, size, weight)).fold(0.0, f32::max);
    let w = widest(top, 500);
    if w <= room {
        return TileLabel { lines, size: top, weight: 500 };
    }
    let size = (top * room / w).max(min);
    if widest(size, 500) <= room {
        return TileLabel { lines, size, weight: 500 };
    }
    let w4 = widest(top, 400);
    let size = (top * room / w4).clamp(min, top);
    TileLabel { lines, size, weight: 400 }
}

/// Dark mode, mute, snip and lock.
fn control_tiles(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let ctl = x.control;
    let items = vec![
        (G_MOON, "Dark mode", ctl.dark, WAct::Dark),
        (if ctl.muted == Some(true) { G_MUTE } else { G_VOL }, "Mute", ctl.muted == Some(true), WAct::Mute),
        (G_SNIP, "Snip", false, WAct::Snip),
        (G_LOCK, "Lock", false, WAct::Lock),
    ];
    let extra = vec![("Snip", "Win + Shift + S", WAct::Snip), ("Lock", "Win + L", WAct::Lock)];
    button_grid(c, out, x, items, extra, sh);
}

/// Wi-Fi, Bluetooth, dark mode and mute.
fn toggles(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let ctl = x.control;
    let items = vec![
        (G_WIFI, "Wi-Fi", ctl.wifi == Some(true), WAct::Wifi),
        (G_BT, "Bluetooth", ctl.bluetooth == Some(true), WAct::Bluetooth),
        (G_MOON, "Dark mode", ctl.dark, WAct::Dark),
        (if ctl.muted == Some(true) { G_MUTE } else { G_VOL }, "Mute", ctl.muted == Some(true), WAct::Mute),
    ];
    let extra = vec![
        ("Network & internet", "", WAct::Open("ms-settings:network".into())),
        ("Bluetooth & devices", "", WAct::Open("ms-settings:bluetooth".into())),
    ];
    button_grid(c, out, x, items, extra, sh);
}

/// A "+N" chip for apps that don't fit: a plate circle of diameter `d`.
fn more_chip(c: &Canvas, cx: f32, cy: f32, d: f32, n: usize, fs: f32) {
    c.circle(cx, cy, d / 2.0, c.plate());
    c.text_cap(&format!("+{n}"), cx, cy - c.cap_h(fs) / 2.0, fs, 600, c.text2(), Align::Center, d * 2.0);
}

/// Background Apps: the tray icons (the host draws the pictures). `slots` cells; when
/// there are more apps the last cell becomes "+N".
fn tray(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let n = x.tray.len();
    let k = c.h / 54.0;
    let hover_name = x.hover_tray.and_then(|h| x.tray.iter().find(|t| t.0 == h)).map(|(_, tip)| tip.lines().next().unwrap_or("").to_string()).filter(|s| !s.is_empty());
    // Icon i at (ix, iy) with size s and a hit cell around it.
    let place = |out: &mut Painted, i: usize, ix: f32, iy: f32, s: f32, cell: (f32, f32, f32, f32)| {
        let (key, _) = &x.tray[i];
        if x.hover_tray == Some(*key) && !matches!(sh, Shape::Mini | Shape::Dock) {
            let p = style::SP[2];
            c.rr(ix - p, iy - p, s + 2.0 * p, s + 2.0 * p, style::R_MIN + 2.0, c.plate());
        }
        out.icons.push((ix, iy, s, format!("tray:{key}")));
        out.hits.push((cell, WAct::Tray(*key)));
    };
    // How many icons to draw in `slots` cells, and how many the "+N" chip stands for.
    let split = |slots: usize| if n > slots { (slots - 1, n - (slots - 1)) } else { (n, 0) };
    let empty_tile = |x0: f32, y0: f32, s: f32| cc_plate(c, G_UP, x0, y0, s, c.plate(), c.text2());
    let apps = |n: usize| if n == 1 { "1 app".to_string() } else { format!("{n} apps") };
    match sh {
        Shape::Mini => {
            if n == 0 {
                let s = 22.0 * k;
                empty_tile((c.w - s) / 2.0, (c.h - s) / 2.0, s);
                return;
            }
            // A 2×2 folder of the first four icons.
            let (cell, gap, s) = (20.0 * k, style::SP[1] * k, 16.0 * k);
            let grid = 2.0 * cell + gap;
            let (x0, y0) = ((c.w - grid) / 2.0, (c.h - grid) / 2.0);
            for i in 0..n.min(4) {
                let (cx, cy) = (x0 + (i % 2) as f32 * (cell + gap), y0 + (i / 2) as f32 * (cell + gap));
                place(out, i, cx + (cell - s) / 2.0, cy + (cell - s) / 2.0, s, (cx - gap / 2.0, cy - gap / 2.0, cell + gap, cell + gap));
            }
        }
        Shape::Dock => {
            let m = style::M_DOCK * k;
            let s = 22.0 * k;
            let pitch = 28.0 * k;
            let y = (c.h - s) / 2.0;
            if n == 0 {
                empty_tile(m, y, s);
                let fs = style::FOOT.size * k;
                let tx = m + s + style::SP[4] * k;
                c.text_base("None running", tx, c.h / 2.0 + c.cap_h(fs) / 2.0, fs, 400, c.text2(), Align::Left, c.w - m - tx);
                return;
            }
            let slots = (((c.w - 2.0 * m + pitch - s) / pitch).floor() as usize).max(1);
            let (show, more) = split(slots);
            for i in 0..show {
                let ix = m + i as f32 * pitch;
                place(out, i, ix, y, s, (ix - (pitch - s) / 2.0, 0.0, pitch, c.h));
            }
            if more > 0 {
                more_chip(c, m + show as f32 * pitch + s / 2.0, c.h / 2.0, s, more, style::FOOT.size * k);
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let fs = style::CAPTION.size;
            if n == 0 {
                empty_tile(m, m, style::BADGE_SMALL);
                c.text_base("None running", m, 106.0, fs, 400, c.text2(), Align::Left, c.w - 2.0 * m);
                return;
            }
            // A 3×2 grid spread edge to edge inside the margins, a caption under it.
            let s = 20.0;
            let pitch = (c.w - 2.0 * m - s) / 2.0;
            let (show, more) = split(6);
            for i in 0..show {
                let (ix, iy) = (m + (i % 3) as f32 * pitch, m + (i / 3) as f32 * pitch);
                place(out, i, ix, iy, s, (ix - (pitch - s) / 2.0, iy - (pitch - s) / 2.0, pitch, pitch));
            }
            if more > 0 {
                let (ix, iy) = (m + 2.0 * pitch, m + pitch);
                more_chip(c, ix + s / 2.0, iy + s / 2.0, 22.0, more, style::FOOT.size);
            }
            let cap = hover_name.unwrap_or_else(|| apps(n));
            c.text_base(&cap, m, 106.0, fs, 400, c.text2(), Align::Left, c.w - 2.0 * m);
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            // Header: glyph plate, title, the count (or the app under the pointer).
            let bs = style::BADGE_HEADER;
            cc_plate(c, G_UP, m, m, bs, c.plate(), c.text1());
            let fs = style::FOOT.size;
            let trailing = hover_name.unwrap_or_else(|| if n == 0 { String::new() } else { n.to_string() });
            let tw = c.text_base(&trailing, c.w - m, style::HEADER_BASE, fs, 500, c.text2(), Align::Right, (c.w - 2.0 * m) * 0.45);
            let tx = m + bs + style::SP[3];
            c.text_base("Background Apps", tx, style::HEADER_BASE, style::TITLE.size, 600, c.text1(), Align::Left, c.w - m - tw - style::SP[3] - tx);
            if n == 0 {
                let cs = style::CAPTION.size;
                if sh == Shape::Large {
                    // A centred empty state: a plate, the message and what will appear here.
                    let p = 40.0;
                    let py = 72.0;
                    cc_plate(c, G_UP, (c.w - p) / 2.0, py, p, c.plate(), c.text2());
                    c.text_base("No background apps", c.w / 2.0, py + p + 24.0, style::TITLE.size, 600, c.text1(), Align::Center, c.w - 2.0 * m);
                    c.text_base("Apps that run beside the clock show here", c.w / 2.0, py + p + 42.0, style::FOOT.size, 400, c.text2(), Align::Center, c.w - 2.0 * m);
                } else {
                    c.text_base("No background apps", m, 62.0 + c.cap_h(cs) / 2.0, cs, 400, c.text2(), Align::Left, c.w - 2.0 * m);
                }
                return;
            }
            let (cols, rows, s) = if sh == Shape::Large { (6usize, 4usize, 24.0) } else { (7, 1, 22.0) };
            // First row centred on y 62 (as Medium); Large spreads its rows to the bottom margin.
            let y0 = 62.0 - s / 2.0;
            let px = (c.w - 2.0 * m - s) / (cols - 1) as f32;
            let py = if rows > 1 { (c.h - m - s - y0) / (rows - 1) as f32 } else { 0.0 };
            let (show, more) = split(cols * rows);
            for i in 0..show {
                let (ix, iy) = (m + (i % cols) as f32 * px, y0 + (i / cols) as f32 * py);
                let ch = if rows > 1 { py } else { 36.0 };
                place(out, i, ix, iy, s, (ix - (px - s) / 2.0, iy - (ch - s) / 2.0, px, ch));
            }
            if more > 0 {
                let i = show;
                let (ix, iy) = (m + (i % cols) as f32 * px, y0 + (i / cols) as f32 * py);
                more_chip(c, ix + s / 2.0, iy + s / 2.0, s, more, fs);
            }
            // Large with rows to spare: a summary in the next row.
            let used = (show + usize::from(more > 0)).div_ceil(cols);
            if sh == Shape::Large && used < rows {
                let cy = y0 + used as f32 * py + s / 2.0;
                c.text_base(&format!("{} running", apps(n)), m, cy + c.cap_h(fs) / 2.0, fs, 400, c.text2(), Align::Left, c.w - 2.0 * m);
            }
        }
    }
}

/// What Windows Update shows in each state.
struct UpdView {
    /// The hero (Small, Medium) and the dock value.
    hero: String,
    dock: String,
    /// Caption under the hero (Small, Medium) and in the dock.
    caption: &'static str,
    dock_caption: &'static str,
    /// Medium's third line.
    third: Option<&'static str>,
    /// Blue plate (updates waiting or a restart needed).
    attention: bool,
    restart: bool,
    /// Count for the red badge; Some(None) is a plain dot.
    badge: Option<Option<u32>>,
    /// Couldn't ask Windows Update: the caption starts with an orange warning glyph.
    warn: bool,
    ok: bool,
}

fn upd_view(x: &Ctx) -> Option<UpdView> {
    use laminar_sys::update::UpdateStatus as U;
    let v = |hero: String, dock: String, caption, dock_caption, third, attention, badge| UpdView { hero, dock, caption, dock_caption, third, attention, restart: false, badge, warn: false, ok: false };
    Some(match x.update? {
        U::RestartRequired => UpdView { restart: true, ..v("Restart".into(), "Restart".into(), "to finish installing", "to finish", Some("Windows Update"), true, Some(None)) },
        U::Pending(n) => {
            let one = *n == 1;
            v(n.to_string(), n.to_string(), if one { "Update waiting" } else { "Updates waiting" }, if one { "update" } else { "updates" }, Some("Windows Update"), true, Some(Some(*n)))
        }
        U::UpToDate => UpdView { ok: true, ..v("Up to date".into(), "Current".into(), "Windows Update", "up to date", None, false, None) },
        U::Unknown => UpdView { warn: true, ..v("Updates".into(), "Updates".into(), "Status unavailable", "unavailable", None, false, None) },
    })
}

/// A caption line, with the orange warning glyph first when `warn`.
#[allow(clippy::too_many_arguments)]
fn upd_caption(c: &Canvas, s: &str, x: f32, base: f32, size: f32, warn: bool, max_w: f32) {
    let g = if warn { 12.0 + style::SP[1] } else { 0.0 };
    if warn {
        c.glyph(G_WARN, x + 6.0, base - c.cap_h(size) / 2.0, 12.0, ORANGE);
    }
    c.text_base(s, x + g, base, size, 400, c.text2(), Align::Left, max_w - g);
}

fn win_update(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let open = WAct::Open("ms-settings:windowsupdate".into());
    let view = upd_view(x);
    let k = c.h / 54.0;
    let restart = view.as_ref().is_some_and(|v| v.restart);
    // The card opens Windows Update; a needed restart takes the compact cards to the power menu.
    let card_act = if restart && sh != Shape::Medium && sh != Shape::Large && !(sh == Shape::Dock && cc_wide(c)) { WAct::Power } else { open.clone() };
    out.hits.push(((0.0, 0.0, c.w, c.h), card_act));
    let (fill, ink) = cc_fill(c, view.as_ref().is_some_and(|v| v.attention));
    let glyph = if view.as_ref().is_some_and(|v| v.ok) { G_CHECK } else { G_UPDATE };
    let ink = if view.as_ref().is_some_and(|v| v.ok) { GREEN } else { ink };
    // The plate at (px, py) size p, with its count badge of diameter d. Only the Mini,
    // which shows no value, numbers the badge; elsewhere the hero or value already says
    // the count, so the plate gets the plain 8 px attention dot (as Medium does).
    let plate_badge = |px: f32, py: f32, p: f32, d: f32| {
        cc_plate(c, glyph, px, py, p, fill, ink);
        if let Some(Some(b)) = view.as_ref().map(|v| v.badge) {
            let b = if sh == Shape::Mini { b } else { None };
            let d = if b.is_some() { d } else if sh == Shape::Mini { d * 0.625 } else if sh == Shape::Dock { 8.0 * k } else { 8.0 };
            count_badge(c, px + p - d * 0.15, py + d * 0.15, d, b);
        }
    };
    match sh {
        Shape::Mini => {
            let p = style::BADGE_MINI_VIS * k;
            plate_badge((c.w - p) / 2.0, (c.h - p) / 2.0, p, 16.0 * k);
        }
        Shape::Dock => {
            let m = style::M_DOCK * k;
            let p = style::BADGE_DOCK * k;
            plate_badge(m, (c.h - p) / 2.0, p, 14.0 * k);
            let tx = m + p + style::SP[4] * k;
            let wide = cc_wide(c);
            let (sx, sw) = cc_slot(c);
            let right = if wide { sx - style::SP[4] * k } else { c.w - m };
            let Some(v) = view else {
                skeleton(c, tx, (c.h - 10.0 * k) / 2.0, (60.0 * k).min(right - tx), 10.0 * k);
                return;
            };
            // Beside a Restart button the value says "Ready", so "Restart" isn't said twice.
            let (dock, dcap) = if wide && restart { ("Ready", "to restart") } else { (v.dock.as_str(), v.dock_caption) };
            let role = Type { size: style::VALUE.size * k, ..style::VALUE };
            let (val, fs) = fit_value(c, dock, role, right - tx);
            c.text_base(&val, tx, 30.0 * k, fs, role.weight, c.text1(), Align::Left, c.w * 2.0);
            upd_caption(c, dcap, tx, 44.0 * k, style::FOOT.size * k, v.warn, right - tx);
            if wide {
                // A pill button in the accessory slot: Restart (blue) or Open (plate).
                let bh = style::BTN_SECONDARY_DOCK * k;
                let label = if restart { "Restart" } else { "Open" };
                let bw = (c.measure(label, style::FOOT.size * k, 600, Font::Text).0 + 2.0 * style::SP[5] * k).max(56.0 * k).min(sw);
                let (bx, by) = (sx + sw - bw, (c.h - bh) / 2.0);
                let (bf, bi) = cc_fill(c, restart);
                c.rr(bx, by, bw, bh, bh / 2.0, bf);
                let ls = style::FOOT.size * k;
                c.text_cap(label, bx + bw / 2.0, by + (bh - c.cap_h(ls)) / 2.0, ls, 600, bi, Align::Center, bw);
                out.hits.push(((bx, 0.0, bw, c.h), if restart { WAct::Power } else { open }));
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            plate_badge(m, m, style::BADGE_SMALL, 16.0);
            let inner = c.w - 2.0 * m;
            let Some(v) = view else {
                let ch = c.cap_h(style::HERO_S.size);
                skeleton(c, m, 86.0 - ch, inner * 0.6, ch);
                skeleton(c, m, 106.0 - 8.0, 40.0, 8.0);
                return;
            };
            let (val, fs) = fit_value(c, &v.hero, style::HERO_S, inner);
            c.text_base(&val, m, 86.0, fs, style::HERO_S.weight, c.text1(), Align::Left, c.w * 2.0);
            // Small says less: "updates waiting" reads on from the number; "Unavailable" fits.
            let cap = if v.warn { "Unavailable".to_string() } else if matches!(x.update, Some(laminar_sys::update::UpdateStatus::Pending(_))) { v.caption.to_lowercase() } else { v.caption.to_string() };
            upd_caption(c, &cap, m, 106.0, style::CAPTION.size, v.warn, inner);
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let bs = style::BADGE_GLANCE;
            let (bx, by) = (c.w - m - bs, m);
            cc_plate(c, glyph, bx, by, bs, fill, ink);
            match &view {
                None => {
                    skeleton(c, m, m, (c.w - 2.0 * m - bs - style::SP[3]) * 0.6, c.cap_h(style::HERO.size));
                    skeleton(c, m, 58.0 - 8.0, 40.0, 8.0);
                }
                Some(v) => {
                    if v.badge.is_some() {
                        count_badge(c, bx + bs - 1.0, by + 1.0, 8.0, None);
                    }
                    // Beside the Restart button the hero says "Ready", so "Restart" isn't said twice.
                    let (hero, cap) = if restart { ("Ready", "Restart to finish") } else { (v.hero.as_str(), v.caption) };
                    let (val, fs) = fit_value(c, hero, style::HERO, c.w - 2.0 * m - bs - style::SP[3]);
                    c.text_cap(&val, m, m, fs, style::HERO.weight, c.text1(), Align::Left, c.w * 2.0);
                    let bw = if restart { 88.0 } else { 0.0 };
                    let cap_w = c.w - 2.0 * m - if restart { bw + style::SP[3] } else { 0.0 };
                    upd_caption(c, cap, m, 58.0, style::CAPTION.size, v.warn, cap_w);
                    if let Some(t) = v.third {
                        c.text_base(t, m, 76.0, style::FOOT.size, 400, c.text3(), Align::Left, cap_w);
                    }
                    if restart {
                        let bh = style::BTN_SECONDARY;
                        pill_button(c, out, c.w - m - bw, 80.0 - bh, bw, bh, "Restart", WAct::Power);
                    }
                }
            }
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                link_rows(
                    c,
                    out,
                    vec![
                        ("Open Windows Update", "", open),
                        ("Update history", "", WAct::Open("ms-settings:windowsupdate-history".into())),
                        ("Advanced options", "", WAct::Open("ms-settings:windowsupdate-options".into())),
                    ],
                    style::DETAIL_TOP,
                );
            }
        }
    }
}

/// Windows Settings: a gear that opens Settings, with shortcuts to common pages as the
/// card grows.
fn win_settings(c: &Canvas, out: &mut Painted, sh: Shape) {
    let open = |p: &str| WAct::Open(format!("ms-settings:{p}"));
    out.hits.push(((0.0, 0.0, c.w, c.h), open("")));
    let shortcuts = [(G_SCREEN, "display"), (G_VOL, "sound"), (G_WIFI, "network"), (G_BT, "bluetooth")];
    let (pf, pi) = (c.plate(), c.text1());
    // Round secondary buttons from the right edge `right`, centred on `cy`.
    let buttons = |out: &mut Painted, n: usize, right: f32, cy: f32, d: f32, gap: f32, hit: f32| {
        for (i, (g, page)) in shortcuts.iter().take(n).enumerate() {
            let cx = right - d / 2.0 - (n - 1 - i) as f32 * (d + gap);
            c.circle(cx, cy, d / 2.0, pf);
            c.glyph(g, cx, cy, d * style::GLYPH_IN, pi);
            // Hit squares are at least 28 (§2.4); neighbours may overlap into the gap.
            let hd = (d + gap).max(hit);
            out.hits.push(((cx - hd / 2.0, cy - hd / 2.0, hd, hd), open(page)));
        }
    };
    let k = c.h / 54.0;
    match sh {
        Shape::Mini => {
            let p = style::BADGE_MINI_VIS * k;
            cc_plate(c, G_SETTINGS, (c.w - p) / 2.0, (c.h - p) / 2.0, p, pf, pi);
        }
        Shape::Dock => {
            let m = style::M_DOCK * k;
            let p = style::BADGE_DOCK * k;
            cc_plate(c, G_SETTINGS, m, (c.h - p) / 2.0, p, pf, pi);
            let tx = m + p + style::SP[4] * k;
            let (sx, _) = cc_slot(c);
            let right = if cc_wide(c) { sx - style::SP[4] * k } else { c.w - m };
            c.text_base("Settings", tx, 30.0 * k, style::TITLE.size * k, 600, c.text1(), Align::Left, right - tx);
            c.text_base("Windows", tx, 44.0 * k, style::FOOT.size * k, 400, c.text2(), Align::Left, right - tx);
            if cc_wide(c) {
                buttons(out, 3, c.w - m, c.h / 2.0, 22.0 * k, style::SP[1] * k, style::BTN_SECONDARY * k);
            }
        }
        Shape::Small => {
            let p = 44.0;
            cc_plate(c, G_SETTINGS, (c.w - p) / 2.0, 22.0, p, pf, pi);
            c.text_base("Settings", c.w / 2.0, 88.0, style::TITLE.size, 600, c.text1(), Align::Center, c.w - 2.0 * style::M_SMALL);
            c.text_base("Windows", c.w / 2.0, 104.0, style::FOOT.size, 400, c.text2(), Align::Center, c.w - 2.0 * style::M_SMALL);
        }
        Shape::Medium => {
            let m = style::M_CARD;
            let p = 44.0;
            cc_plate(c, G_SETTINGS, m, 26.0, p, pf, pi);
            let tx = m + p + style::SP[5];
            let d = style::BTN_SECONDARY;
            let n = 3;
            let bl = c.w - m - n as f32 * d - (n - 1) as f32 * style::SP[3];
            c.text_base("Settings", tx, 44.0, style::TITLE.size, 600, c.text1(), Align::Left, bl - style::SP[3] - tx);
            c.text_base("Windows", tx, 60.0, style::CAPTION.size, 400, c.text2(), Align::Left, bl - style::SP[3] - tx);
            buttons(out, n, c.w - m, 48.0, d, style::SP[3], style::BTN_SECONDARY);
        }
        Shape::Large => {
            // A state tile with the gear, then Medium's shortcuts (plus Apps) as a row of
            // labelled round buttons on the bottom margin.
            let inset = style::SP[2];
            let th = 112.0;
            c.rr(inset, inset, c.w - 2.0 * inset, th, style::concentric(style::CARD_R, inset), c.plate());
            let d = 56.0;
            let (cx, cy) = (inset + style::M_CARD + d / 2.0, inset + th / 2.0);
            // A card-coloured disc, so the gear separates from the tile behind it.
            c.circle(cx, cy, d / 2.0, c.halo());
            c.glyph(G_SETTINGS, cx, cy, 32.0, c.text1());
            let tx = cx + d / 2.0 + style::SP[5];
            let vs = style::VALUE.size;
            c.text_base("Settings", tx, cy - 2.0, vs, 600, c.text1(), Align::Left, c.w - inset - style::M_CARD - tx);
            c.text_base("Windows", tx, cy + 16.0, style::BODY.size, 400, c.text2(), Align::Left, c.w - inset - style::M_CARD - tx);
            let m = style::M_CARD;
            let row = [(G_SCREEN, "Display", "display"), (G_VOL, "Sound", "sound"), (G_WIFI, "Network", "network"), (G_APPS, "Apps", "appsfeatures")];
            let col_w = (c.w - 2.0 * m) / row.len() as f32;
            let bd = style::BTN_SECONDARY;
            let fs = style::FOOT.size;
            let base = c.h - m - style::SP[1];
            let bcy = base - c.cap_h(fs) - style::SP[3] - bd / 2.0;
            let top = inset + th + style::SP[2];
            for (i, (g, label, page)) in row.into_iter().enumerate() {
                let x0 = m + i as f32 * col_w;
                let bx = x0 + col_w / 2.0;
                c.circle(bx, bcy, bd / 2.0, pf);
                c.glyph(g, bx, bcy, bd * style::GLYPH_IN, pi);
                c.text_base(label, bx, base, fs, 500, c.text2(), Align::Center, col_w);
                out.hits.push(((x0, top, col_w, c.h - top - style::SP[2]), open(page)));
            }
        }
    }
}

// ------------------------------------------------------------------ progress

/// "1 day", "14 days"; on the last day, the time left today instead of "0 days".
fn span_days(n: u32, rest_of_today: u64) -> String {
    match n {
        0 => dur(rest_of_today),
        1 => "1 day".into(),
        _ => format!("{n} days"),
    }
}

/// A grid of units (hours, days, weeks): done ones in the accent, future ones in the track,
/// the current one as a track dot with an accent ring. `left` is the first dot's left ink
/// and `cy` the grid's vertical centre.
#[allow(clippy::too_many_arguments)]
fn unit_grid(c: &Canvas, total: u32, current: u32, cols: u32, left: f32, cy: f32, pitch: f32, dot: f32, col: Col) {
    let rows = total.div_ceil(cols);
    let top = cy - ((rows - 1) as f32 * pitch + dot) / 2.0;
    let r = dot / 2.0;
    for i in 0..total {
        let (px, py) = (left + r + (i % cols) as f32 * pitch, top + r + (i / cols) as f32 * pitch);
        if i < current {
            c.circle(px, py, r, col);
        } else {
            c.circle(px, py, r, c.track());
            if i == current {
                c.circle_stroke(px, py, r - 0.75, col, 1.5);
            }
        }
    }
}

fn progress(c: &Canvas, s: Span, x: &Ctx, sh: Shape) {
    let n = x.now;
    let mm = n.hour * 60 + n.minute;
    let today_left = ((1440 - mm) * 60) as u64;
    let col = match s {
        Span::Day => BLUE,
        Span::Month => GREEN,
        Span::Year => ORANGE,
    };
    // Units, the one in progress, fraction gone, and the words around them.
    let (total, current, cols, frac, caption, position, elapsed, left, ends, ticks) = match s {
        Span::Day => (
            24,
            n.hour,
            8,
            mm as f32 / 1440.0,
            "of today".to_string(),
            format!("Hour {} of 24", n.hour + 1),
            dur((mm * 60) as u64),
            dur(today_left),
            ("Midnight".to_string(), "Ends".to_string()),
            vec![(0.0, "12AM".to_string()), (0.25, "6AM".into()), (0.5, "12PM".into()), (0.75, "6PM".into())],
        ),
        Span::Month => {
            let dim = n.days_in_month();
            let last_wd = (n.weekday + dim - n.day) % 7;
            let mon = MONTHS[n.month as usize - 1];
            (
                dim,
                n.day - 1,
                8,
                ((n.day - 1) as f32 + mm as f32 / 1440.0) / dim as f32,
                format!("of {mon}"),
                format!("Day {} of {dim}", n.day),
                span_days(n.day - 1, 0),
                span_days(dim - n.day, today_left),
                (format!("{} {dim}", &mon[..3]), format!("Ends {}", &DAYS[last_wd as usize][..3])),
                [1u32, 8, 15, 22, 29].iter().filter(|d| **d <= dim).map(|d| ((d - 1) as f32 / dim as f32, d.to_string())).collect(),
            )
        }
        Span::Year => {
            let (doy, diy) = (n.day_of_year(), n.days_in_year());
            let week = ((doy - 1) / 7).min(51);
            // Dec 31 is (days left) days after today.
            let last_wd = (n.weekday + diy - doy) % 7;
            let ticks = [(1u32, "JAN"), (4, "APR"), (7, "JUL"), (10, "OCT")]
                .iter()
                .map(|(m, l)| {
                    let before: u32 = (1..*m).map(|k| super::days_in_month(n.year, k)).sum();
                    (before as f32 / diy as f32, l.to_string())
                })
                .collect();
            (
                52,
                week,
                13,
                ((doy - 1) as f32 + mm as f32 / 1440.0) / diy as f32,
                format!("of {}", n.year),
                format!("Week {} of 52", week + 1),
                span_days(doy - 1, 0),
                span_days(diy - doy, today_left),
                ("Dec 31".to_string(), format!("Ends {}", &DAYS[last_wd as usize][..3])),
                ticks,
            )
        }
    };
    // Whole percent, rounded down so 100% only shows when it's over.
    let pct = format!("{:.0}%", (frac * 100.0).floor().clamp(0.0, 100.0));
    let (t1, t2, t3) = (c.text1(), c.text2(), c.text3());
    match sh {
        Shape::Mini => {
            // Visual-led: the ring is the icon, the percent inside it.
            let k = c.h / 54.0;
            let d = 40.0 * k;
            let th = (d * style::RING_T).max(3.0 * k);
            let (cx, cy) = (c.w / 2.0, c.h / 2.0);
            c.ring(cx, cy, (d - th) / 2.0, th, frac, col);
            let room = d - 2.0 * th - 4.0 * k;
            let (lo, hi) = (style::MINI_VALUE.min * k, 12.0 * k);
            let mut v = pct.clone();
            let mut fs = fit(c, &v, hi, lo, 600, Font::Text, room);
            if c.measure(&v, fs, 600, Font::Text).0 > room {
                v = v.trim_end_matches('%').to_string();
                fs = fit(c, &v, hi, lo, 600, Font::Text, room);
            }
            c.text_cap(&v, cx, cy - c.cap_h(fs) / 2.0, fs, 600, t1, Align::Center, d);
        }
        Shape::Dock => {
            let k = c.h / 54.0;
            let m = style::M_DOCK * k;
            let bs = style::BADGE_DOCK * k;
            let th = (bs * style::RING_T).max(3.0 * k);
            c.ring(m + bs / 2.0, m + bs / 2.0, (bs - th) / 2.0, th, frac, col);
            let x0 = m + bs + style::SP[4] * k;
            let wide = c.w >= c.h * 3.4;
            let slot_w = (c.w * 0.34).round();
            let right = if wide { c.w - m - slot_w - style::SP[4] * k } else { c.w - m };
            c.text_base(&pct, x0, 30.0 * k, style::VALUE.size * k, style::VALUE.weight, t1, Align::Left, c.w);
            c.text_base(&caption, x0, 44.0 * k, style::FOOT.size * k, 400, t2, Align::Left, right - x0);
            if wide {
                let r = c.w - m;
                c.text_base(&left, r, 28.0 * k, style::TITLE.size * k, 600, t1, Align::Right, slot_w);
                c.text_base("left", r, 41.0 * k, style::FOOT.size * k, 400, t2, Align::Right, slot_w);
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let w = c.w - 2.0 * m;
            // The grid fills the width but stays in y m..58, so even month's four rows keep
            // a clear gap above the hero's cap top (~y67); a deeper grid shrinks to fit.
            let rows = total.div_ceil(cols) as f32;
            let (gy0, gy1) = (m, 58.0);
            let pitch = (w / (cols as f32 - 1.0 + 0.68)).min((gy1 - gy0) / (rows - 1.0 + 0.68));
            unit_grid(c, total, current, cols, m, (gy0 + gy1) / 2.0, pitch, pitch * 0.68, col);
            let (v, fs) = fit_value(c, &pct, style::HERO_S, w);
            c.text_base(&v, m, 86.0, fs, style::HERO_S.weight, t1, Align::Left, c.w * 2.0);
            c.text_base(&caption, m, 106.0, style::CAPTION.size, style::CAPTION.weight, t2, Align::Left, w);
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let r = c.w - m;
            c.text_cap(&pct, m, m, style::HERO.size, style::HERO.weight, t1, Align::Left, c.w);
            let left_w = 136.0 - m - style::SP[3];
            c.text_base(&caption, m, 58.0, style::CAPTION.size, style::CAPTION.weight, t2, Align::Left, left_w);
            c.text_base(&position, m, 76.0, style::FOOT.size, style::FOOT.weight, t3, Align::Left, left_w);
            let (pitch, dot) = if cols > 8 { (8.9, 6.0) } else { (14.5, 10.0) };
            let gw = (cols - 1) as f32 * pitch + dot;
            unit_grid(c, total, current, cols, r - gw, 48.0, pitch, dot, col);
            if sh != Shape::Large {
                return;
            }
            separator(c, style::SEP_Y_LARGE);
            // The whole span as a meter, ticked at its true positions.
            let (x0, mw) = (m, r - m);
            let h = style::METER_H;
            c.bar(x0, 112.0 - h / 2.0, mw, h, frac, col);
            let mic = style::MICRO;
            for (f, l) in &ticks {
                let px = x0 + mw * f;
                let (al, px) = if *f <= 0.0 { (Align::Left, px) } else { (Align::Center, px.min(r - c.measure(l, mic.size, mic.weight, Font::Text).0 / 2.0)) };
                c.text_base(l, px, 132.0, mic.size, mic.weight, t3, al, 40.0);
            }
            let cw = mw / 3.0;
            for (i, (v, l)) in [(elapsed.as_str(), "Elapsed"), (left.as_str(), "Left"), (ends.0.as_str(), ends.1.as_str())].iter().enumerate() {
                let cx = x0 + i as f32 * cw;
                let (v, fs) = fit_value(c, v, style::VALUE, cw - style::SP[3]);
                c.text_base(&v, cx, 166.0, fs, style::VALUE.weight, t1, Align::Left, c.w);
                c.text_base(l, cx, 182.0, style::FOOT.size, style::FOOT.weight, t2, Align::Left, cw - style::SP[3]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_figures_use_signs_not_triangles() {
        assert_eq!(fmt_delta_text(12.44), "+12.4%");
        assert_eq!(fmt_delta_text(-3.1), "\u{2212}3.1%");
        assert_eq!(fmt_delta_text(0.01), "0.0%");
    }

    #[test]
    fn compact_forms_keep_prefix_and_suffix() {
        assert_eq!(compact_str("$5,336").as_deref(), Some("$5.3K"));
        assert_eq!(compact_str("$83.46").as_deref(), Some("$83"));
        assert_eq!(compact_str("1,900,000 tokens").as_deref(), Some("1.9M tokens"));
        assert_eq!(compact_str("412,000").as_deref(), Some("412K"));
        assert_eq!(compact_str("82%"), None);
        assert_eq!(compact_str("Now"), None);
    }

    #[test]
    fn money_follows_the_spec() {
        assert_eq!(money(83.456, None), "$83.46");
        assert_eq!(money(126.0, None), "$126");
        assert_eq!(money(5336.0, None), "$5,336");
        assert_eq!(money(17491.0, None), "$17.5K");
        assert_eq!(fmt_money(5336.0, None, 5), "$5.3K");
        assert_eq!(fmt_money(126.0, None, 5), "$126");
    }
}
