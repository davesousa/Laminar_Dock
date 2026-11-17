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

fn got<'a>(x: &'a Ctx, conn: &str) -> Got<'a> {
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
/// its prefix and suffix. None when there is no shorter form.
fn compact_str(s: &str) -> Option<String> {
    let start = s.find(|ch: char| ch.is_ascii_digit())?;
    let end = s[start..].find(|ch: char| !(ch.is_ascii_digit() || ch == ',' || ch == '.')).map(|e| start + e).unwrap_or(s.len());
    let num = &s[start..end];
    let v: f64 = num.replace(',', "").parse().ok()?;
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
    Glyph(&'static str, Col),
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
        Vis::Glyph(g, col) => {
            c.rr(x, y, size, size, size * style::BADGE_R, c.plate());
            c.glyph(g, x + size / 2.0, y + size / 2.0, size * 0.5, *col);
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

/// Large cards: the detail region under the medium layout (§3 Large): a history chart,
/// else up to the rows that fit, inside the margins.
fn details(c: &Canvas, f: &Feed, col: Col) {
    let (x, y, w, h) = detail_region(c);
    if f.series.len() > 2 {
        separator(c, style::SEP_Y_LARGE);
        c.chart(x, y, w, h, &f.series, line_col(c, col), ChartOpts { fill: true, dot: true, stroke: style::STROKE, smooth: f.series.len() >= 8 });
        return;
    }
    let items: Vec<(&str, &str)> = f.rows.iter().map(|r| (r.label.as_str(), r.value.as_str())).collect();
    if items.is_empty() {
        return;
    }
    separator(c, style::SEP_Y_LARGE);
    // Rows start a little above the region so the first one's capitals sit at its top.
    let top = y - style::SP[1];
    let fit_rows = ((c.h - style::M_CARD - top) / style::ROW_L).floor() as usize;
    rows(c, &items, top, style::ROW_L, fit_rows.min(style::ROWS_MAX_L));
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
                c.text_base(name, x0, 30.0 * k, style::TITLE.size * k, 600, c.text1(), Align::Left, tw);
                reason_line(c, &reason, x0, 44.0 * k, style::FOOT.size * k, false, Align::Left, tw);
                let (px, py) = (c.w - m - slot_w, (c.h - ph) / 2.0);
                c.rr(px, py, slot_w, ph, ph / 2.0, c.blue());
                let fs = style::FOOT.size * k;
                c.text_cap("Connect", px + slot_w / 2.0, py + (ph - c.cap_h(fs)) / 2.0, fs, 600, WHITE, Align::Center, slot_w);
            } else {
                c.text_base(name, x0, 30.0 * k, style::TITLE.size * k, 600, c.text1(), Align::Left, tw);
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

#[allow(clippy::too_many_arguments)]
fn dial(c: &Canvas, cx: f32, cy: f32, r: f32, h: u32, m: u32, s: Option<u32>, dark: bool) {
    let (face, ink) = if dark { ((0.11, 0.11, 0.13, 1.0), (1.0, 1.0, 1.0, 0.95)) } else { ((0.97, 0.97, 0.98, 1.0), (0.08, 0.08, 0.1, 1.0)) };
    c.circle(cx, cy, r, face);
    for i in 0..12 {
        let a = (i as f32 * 30.0).to_radians();
        let (r0, r1) = if i % 3 == 0 { (r * 0.78, r * 0.9) } else { (r * 0.83, r * 0.9) };
        c.line(cx + r0 * a.sin(), cy - r0 * a.cos(), cx + r1 * a.sin(), cy - r1 * a.cos(), if i % 3 == 0 { r * 0.06 } else { r * 0.035 }, alpha(ink, 0.7));
    }
    let hand = |deg: f32, len: f32, t: f32, col: Col| {
        let a = deg.to_radians();
        c.line(cx - len * 0.15 * a.sin(), cy + len * 0.15 * a.cos(), cx + len * a.sin(), cy - len * a.cos(), t, col);
    };
    hand((h % 12) as f32 * 30.0 + m as f32 * 0.5, r * 0.5, r * 0.09, ink);
    hand(m as f32 * 6.0 + s.unwrap_or(0) as f32 * 0.1, r * 0.74, r * 0.065, ink);
    if let Some(s) = s {
        hand(s as f32 * 6.0, r * 0.8, r * 0.03, ORANGE);
    }
    c.circle(cx, cy, r * 0.06, ORANGE);
}

/// The analog face in a rounded square, with numerals at 12, 3, 6 and 9 when there's room.
#[allow(clippy::too_many_arguments)]
fn square_dial(c: &Canvas, x0: f32, y0: f32, s: f32, h: u32, m: u32, sec: u32) {
    let dark = !c.light;
    let (face, ink) = if dark { ((0.11, 0.11, 0.13, 1.0), (1.0, 1.0, 1.0, 0.95)) } else { ((0.97, 0.97, 0.98, 1.0), (0.08, 0.08, 0.1, 1.0)) };
    c.rr(x0, y0, s, s, s * 0.24, face);
    let (cx, cy) = (x0 + s / 2.0, y0 + s / 2.0);
    if s >= 60.0 {
        let fs = s * 0.13;
        for (num, dx, dy) in [("12", 0.0, -1.0), ("3", 1.0, 0.0), ("6", 0.0, 1.0), ("9", -1.0, 0.0)] {
            let r = s * 0.37;
            c.text(num, cx + dx * r, cy + dy * r - fs * 0.66, fs, 600, alpha(ink, 0.8), Align::Center, fs * 2.0, Font::Display);
        }
    } else {
        for i in 0..4 {
            let a = (i as f32 * 90.0).to_radians();
            let r = s * 0.4;
            c.circle(cx + r * a.sin(), cy - r * a.cos(), s * 0.025, alpha(ink, 0.7));
        }
    }
    let hand = |deg: f32, len: f32, t: f32, col: Col| {
        let a = deg.to_radians();
        c.line(cx - len * 0.12 * a.sin(), cy + len * 0.12 * a.cos(), cx + len * a.sin(), cy - len * a.cos(), t, col);
    };
    hand((h % 12) as f32 * 30.0 + m as f32 * 0.5, s * 0.22, s * 0.045, ink);
    hand(m as f32 * 6.0 + sec as f32 * 0.1, s * 0.33, s * 0.032, ink);
    hand(sec as f32 * 6.0, s * 0.36, s * 0.015, ORANGE);
    c.circle(cx, cy, s * 0.03, ORANGE);
}

#[allow(clippy::too_many_arguments)]
fn seg_digit(c: &Canvas, x: f32, y: f32, w: f32, h: f32, d: u32, on: Col, off: Col) {
    // a b c d e f g
    const SEGS: [u8; 10] = [0x3F, 0x06, 0x5B, 0x4F, 0x66, 0x6D, 0x7D, 0x07, 0x7F, 0x6F];
    let m = SEGS[d as usize % 10];
    let t = w * 0.16;
    let hh = h / 2.0;
    let rects = [
        (x + t * 0.6, y, w - t * 1.2, t),
        (x + w - t, y + t * 0.6, t, hh - t * 0.9),
        (x + w - t, y + hh + t * 0.3, t, hh - t * 0.9),
        (x + t * 0.6, y + h - t, w - t * 1.2, t),
        (x, y + hh + t * 0.3, t, hh - t * 0.9),
        (x, y + t * 0.6, t, hh - t * 0.9),
        (x + t * 0.6, y + hh - t / 2.0, w - t * 1.2, t),
    ];
    for (i, r) in rects.iter().enumerate() {
        c.rr(r.0, r.1, r.2, r.3, t / 2.0, if m & (1 << i) != 0 { on } else { off });
    }
}

fn month_grid(c: &Canvas, n: &super::Now, x0: f32, y0: f32, w: f32, h: f32) {
    let colw = w / 7.0;
    let first = n.first_weekday();
    let rows = (first + n.days_in_month()).div_ceil(7) as f32;
    let head = (h * 0.12).clamp(10.0, 20.0);
    let rh = (h - head) / rows;
    let fs = (rh * 0.55).min(colw * 0.5).clamp(7.0, 12.5);
    for (i, d) in ["S", "M", "T", "W", "T", "F", "S"].iter().enumerate() {
        c.tc(d, x0 + colw * (i as f32 + 0.5), y0, fs * 0.85, 600, c.text3(), colw);
    }
    for day in 1..=n.days_in_month() {
        let idx = first + day - 1;
        let cx = x0 + colw * ((idx % 7) as f32 + 0.5);
        let cy = y0 + head + rh * (idx / 7) as f32 + rh / 2.0;
        let today = day == n.day;
        if today {
            c.circle(cx, cy, (rh * 0.46).min(colw * 0.46), RED);
        }
        c.tc(&day.to_string(), cx, cy - fs * 0.68, fs, if today { 700 } else { 400 }, if today { WHITE } else { c.text1() }, colw);
    }
}

/// This week as a row: day letters and dates, today marked.
fn week_row(c: &Canvas, n: &super::Now, x0: f32, y0: f32, w: f32, h: f32) {
    let colw = w / 7.0;
    let r = (h * 0.28).min(colw * 0.42).min(16.0);
    let fs = (r * 0.95).max(8.0);
    let ls = (h * 0.2).clamp(8.0, 11.0);
    for i in 0..7u32 {
        let off = i as i64 - n.weekday as i64;
        let d = n.day as i64 + off;
        let dim = super::days_in_month(n.year, n.month) as i64;
        let prev = super::days_in_month(n.year, if n.month == 1 { 12 } else { n.month - 1 }) as i64;
        let shown = if d < 1 { d + prev } else if d > dim { d - dim } else { d };
        let cx = x0 + colw * (i as f32 + 0.5);
        let today = off == 0;
        // Today in red, as in every calendar face (month grid, date tile, day page).
        c.tc(&DAYS[i as usize][..1], cx, y0, ls, 600, if today { RED } else { c.text3() }, colw);
        let cy = y0 + ls * 1.5 + (h - ls * 1.5) / 2.0;
        if today {
            c.circle(cx, cy, r, RED);
        }
        c.tc(&shown.to_string(), cx, cy - fs * 0.68, fs, if today { 700 } else { 500 }, if today { WHITE } else { c.text1() }, colw);
    }
}

/// A tear-off calendar page for today.
fn day_tile(c: &Canvas, n: &super::Now, sh: Shape) {
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    let s = c.h.min(c.w) - if dense { 6.0 } else { 16.0 };
    let (x0, y0) = ((c.w - s) / 2.0, (c.h - s) / 2.0);
    if !dense && c.w > c.h * 1.3 {
        bg(c, false);
    }
    c.rr(x0, y0, s, s, s * 0.2, (0.98, 0.98, 0.99, 1.0));
    let hh = s * 0.28;
    c.rr(x0, y0, s, hh + s * 0.2, s * 0.2, RED);
    c.rr(x0, y0 + hh, s, s * 0.2, 0.0, (0.98, 0.98, 0.99, 1.0));
    c.tc(&MONTHS[n.month as usize - 1][..3].to_uppercase(), x0 + s / 2.0, y0 + hh * 0.18, hh * 0.62, 700, WHITE, s);
    c.text(&n.day.to_string(), x0 + s / 2.0, y0 + hh + s * 0.02, s * 0.5, 600, (0.1, 0.1, 0.12, 1.0), Align::Center, s, Font::Display);
}

fn clock(c: &Canvas, out: &mut Painted, k: ClockKind, x: &Ctx, sh: Shape) {
    let n = x.now;
    let time = format!("{}:{:02}", h12(n.hour), n.minute);
    let mon = MONTHS[n.month as usize - 1];
    let day = DAYS[n.weekday as usize];
    let date_long = format!("{day}, {mon} {}", n.day);
    let date_short = format!("{} {} {}", &day[..3], &mon[..3], n.day);
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("ms-settings:dateandtime".into())));
    match k {
        ClockKind::Digital | ClockKind::ClockWeather => {
            let weather = if k == ClockKind::ClockWeather { x.weather.map(|w| (weather_emoji(w.code, w.is_day), format!("{:.0}°", d0(w.temp_c)))) } else { None };
            match sh {
                Shape::Mini => {
                    let fs = fit(c, &time, c.h * 0.36, 9.0, 600, Font::Display, c.w - 6.0);
                    c.text(&time, c.w / 2.0, c.h * 0.2, fs, 600, c.text1(), Align::Center, c.w, Font::Display);
                    let sub = weather.as_ref().map(|w| w.1.clone()).unwrap_or_else(|| ampm(n.hour).to_string());
                    c.tc(&sub, c.w / 2.0, c.h * 0.2 + fs * 1.22, c.h * 0.18, 600, c.text2(), c.w);
                }
                Shape::Dock => {
                    // Time on the left; the date (or weather) beside it when there's room, else under it.
                    let ts = (c.h * 0.42).max(15.0);
                    let tw = c.measure(&time, ts, 600, Font::Display).0;
                    let aw = c.measure(ampm(n.hour), ts * 0.4, 600, Font::Text).0;
                    let beside = c.w > 12.0 + tw + aw + 10.0 + c.h * 1.2;
                    let x0 = 12.0;
                    if beside {
                        let ty = (c.h - ts * 1.3) / 2.0;
                        c.text(&time, x0, ty, ts, 600, c.text1(), Align::Left, c.w, Font::Display);
                        c.tw(ampm(n.hour), x0 + tw + 3.0, ty + ts * 0.66, ts * 0.38, 600, c.text2(), 30.0);
                        let rx = x0 + tw + aw + 16.0;
                        c.line(rx - 7.0, c.h * 0.28, rx - 7.0, c.h * 0.72, 1.0, c.ink(0.15));
                        let cs = (c.h * 0.2).max(9.5);
                        match &weather {
                            Some((e, t)) => {
                                c.text(e, rx, c.h / 2.0 - cs * 1.05, cs * 1.4, 400, c.text1(), Align::Left, 40.0, Font::Text);
                                c.tw(t, rx + cs * 1.9, c.h / 2.0 - cs * 0.85, cs * 1.25, 600, c.text1(), c.w - rx - cs * 1.9 - 6.0);
                            }
                            None => {
                                c.tw(&day[..3].to_uppercase(), rx, c.h / 2.0 - cs * 1.3, cs, 700, RED, c.w - rx - 6.0);
                                c.tw(&format!("{} {}", &mon[..3], n.day), rx, c.h / 2.0 - 1.0, cs * 1.15, 600, c.text1(), c.w - rx - 6.0);
                            }
                        }
                    } else {
                        let ty = c.h * 0.12;
                        c.text(&time, x0, ty, ts, 600, c.text1(), Align::Left, c.w, Font::Display);
                        c.tw(ampm(n.hour), x0 + tw + 3.0, ty + ts * 0.42, ts * 0.42, 500, c.text2(), 30.0);
                        let cap = weather.map(|(e, t)| format!("{e} {t}")).unwrap_or(date_short);
                        c.tw(&cap, x0, ty + ts * 1.24, (c.h * 0.19).max(9.5), 400, c.text2(), c.w - x0 - 4.0);
                    }
                }
                Shape::Small => {
                    // AM/PM beside the minutes, not on a line of its own.
                    let aw = c.measure(ampm(n.hour), 11.0, 600, Font::Text).0;
                    let fs = fit(c, &time, 40.0, 20.0, 300, Font::Display, c.w - 28.0 - aw - 3.0);
                    let tw = c.num(&time, 14.0, 16.0, fs, 300, c.text1());
                    c.tw(ampm(n.hour), 14.0 + tw + 3.0, 16.0 + fs * 0.62, 11.0, 600, c.text2(), 40.0);
                    match weather {
                        Some((e, t)) => {
                            c.text(e, 14.0, c.h - 36.0, 18.0, 400, c.text1(), Align::Left, 30.0, Font::Text);
                            c.tw(&t, 40.0, c.h - 32.0, 14.0, 600, c.text1(), c.w - 54.0);
                        }
                        None => {
                            c.tw(&date_short, 14.0, c.h - 28.0, 11.5, 400, c.text2(), c.w - 28.0);
                        }
                    }
                }
                Shape::Medium if k == ClockKind::Digital => {
                    let tw = c.num(&time, 16.0, 6.0, 50.0, 300, c.text1());
                    c.num(&format!(":{:02}", n.second), 18.0 + tw, 30.0, 22.0, 300, c.text3());
                    c.tw(ampm(n.hour), 20.0 + tw + 34.0, 34.0, 15.0, 500, c.text2(), 40.0);
                    c.tw(&date_long, 18.0, 68.0, 13.0, 400, c.text2(), c.w - 36.0);
                }
                Shape::Medium => {
                    let tw = c.num(&time, 16.0, 12.0, 40.0, 400, c.text1());
                    c.tw(ampm(n.hour), 20.0 + tw, 34.0, 13.0, 500, c.text2(), 40.0);
                    c.tw(&date_long, 18.0, 66.0, 12.0, 400, c.text2(), c.w * 0.6);
                    if let Some((e, t)) = weather {
                        c.text(e, c.w - 60.0, 16.0, 28.0, 400, c.text1(), Align::Center, 60.0, Font::Text);
                        c.tr(&t, c.w - 16.0, 56.0, 20.0, 500, c.text1());
                    }
                }
                Shape::Large => {
                    // Big time with seconds, the date, and this week underneath.
                    if weather.is_some() {
                        // The weather takes the right side: no seconds, so AM/PM has room.
                        let tw = c.num(&time, 16.0, 8.0, 58.0, 200, c.text1());
                        c.tw(ampm(n.hour), 20.0 + tw, 42.0, 16.0, 500, c.text2(), 40.0);
                    } else {
                        let tw = c.num(&time, 16.0, 6.0, 64.0, 200, c.text1());
                        c.num(&format!(":{:02}", n.second), 20.0 + tw, 38.0, 26.0, 300, c.text3());
                        c.tw(ampm(n.hour), 20.0 + tw + 42.0, 42.0, 16.0, 500, c.text2(), 40.0);
                    }
                    c.tw(&date_long, 18.0, 86.0, 13.0, 500, c.text2(), c.w - 36.0);
                    if let Some((e, t)) = weather {
                        c.text(e, c.w - 56.0, 18.0, 30.0, 400, c.text1(), Align::Center, 60.0, Font::Text);
                        c.tr(&t, c.w - 18.0, 62.0, 20.0, 500, c.text1());
                    }
                    week_row(c, &n, 14.0, 124.0, c.w - 28.0, c.h - 136.0);
                }
            }
        }
        ClockKind::Flip => {
            let digits = [h12(n.hour) / 10, h12(n.hour) % 10, n.minute / 10, n.minute % 10];
            let tile_col = if c.light { (0.13, 0.13, 0.15, 1.0) } else { (0.06, 0.06, 0.075, 1.0) };
            let tile = |tx: f32, ty: f32, tw: f32, th: f32, d: u32| {
                c.rr(tx, ty, tw, th, tw * 0.16, tile_col);
                // Upper half a touch lighter, like a real flap.
                c.rr(tx, ty, tw, th / 2.0, tw * 0.16, (1.0, 1.0, 1.0, 0.035));
                let fs = (th * 0.66).min(tw * 1.08);
                c.text(&d.to_string(), tx + tw / 2.0, ty + th / 2.0 - fs * 0.68, fs, 600, (0.97, 0.97, 0.98, 1.0), Align::Center, tw, Font::Display);
                c.line(tx + 1.0, ty + th / 2.0, tx + tw - 1.0, ty + th / 2.0, 1.0, (0.0, 0.0, 0.0, 0.55));
            };
            if matches!(sh, Shape::Mini | Shape::Small) {
                // Square spaces: hours above minutes, each a pair of flaps.
                let pad = if sh == Shape::Mini { 5.0 } else { 14.0 };
                let gap = if sh == Shape::Mini { 2.0 } else { 6.0 };
                let tw = (c.w - 2.0 * pad - gap) / 2.0;
                let th = ((c.h - 2.0 * pad - gap) / 2.0).min(tw * 1.25);
                let top = (c.h - 2.0 * th - gap) / 2.0;
                for (i, d) in digits.iter().enumerate() {
                    let (col, row) = ((i % 2) as f32, (i / 2) as f32);
                    tile(pad + col * (tw + gap), top + row * (th + gap), tw, th, *d);
                }
            } else {
                let dense = sh == Shape::Dock;
                let pad = if dense { 6.0 } else { 14.0 };
                let gap = if dense { 2.0 } else { 5.0 };
                let mid = if dense { 9.0 } else { 18.0 };
                let tw = (c.w - 2.0 * pad - 2.0 * gap - mid) / 4.0;
                let th = (c.h - 2.0 * pad).min(tw * 1.45);
                let top = (c.h - th) / 2.0;
                let mut tx = pad;
                for (i, d) in digits.iter().enumerate() {
                    if i == 2 {
                        // The colon between hours and minutes.
                        let cx = tx - gap + mid / 2.0;
                        let r = (mid * 0.16).max(1.5);
                        c.circle(cx, top + th * 0.36, r, c.text2());
                        c.circle(cx, top + th * 0.64, r, c.text2());
                        tx += mid - gap;
                    }
                    tile(tx, top, tw, th, *d);
                    tx += tw + if i == 1 { 0.0 } else { gap };
                }
            }
        }
        ClockKind::World => {
            let cities = super::world_cities(x.cfg);
            let n_c = match sh {
                Shape::Mini => 1,
                Shape::Dock | Shape::Small => 2,
                _ => 3,
            }
            .min(cities.len());
            let labels = !matches!(sh, Shape::Mini | Shape::Dock);
            let slot = c.w / n_c as f32;
            for (i, (label, key)) in cities.iter().take(n_c).enumerate() {
                let (h, m, s, _) = super::tz_now(key).unwrap_or((n.hour, n.minute, n.second, n.weekday));
                let cx = slot * (i as f32 + 0.5);
                let room = if labels { c.h - if sh == Shape::Large { 50.0 } else { 34.0 } } else { c.h - 6.0 };
                let r = (room / 2.0).min(slot / 2.0 - 6.0);
                let cy = if labels { 10.0 + r } else { c.h / 2.0 };
                dial(c, cx, cy, r, h, m, Some(s), !(6..18).contains(&h));
                if labels {
                    let name = if sh == Shape::Large { label.clone() } else { label.chars().take(3).collect::<String>().to_uppercase() };
                    c.tc(&name, cx, cy + r + 6.0, 11.0, 600, c.text2(), slot);
                    if sh == Shape::Large {
                        c.tc(&format!("{}:{:02} {}", h12(h), m, ampm(h)), cx, cy + r + 22.0, 11.0, 400, c.text3(), slot);
                    }
                }
            }
        }
        ClockKind::WeekStrip => match sh {
            Shape::Mini | Shape::Small => day_tile(c, &n, sh),
            Shape::Dock => week_row(c, &n, 6.0, 4.0, c.w - 12.0, c.h - 8.0),
            _ => {
                c.tw(&format!("{mon} {}", n.year), 14.0, 10.0, 12.5, 600, c.text1(), c.w - 30.0);
                week_row(c, &n, 14.0, 32.0, c.w - 28.0, if sh == Shape::Large { 70.0 } else { c.h - 40.0 });
                if sh == Shape::Large {
                    month_grid(c, &n, 14.0, 110.0, c.w - 28.0, c.h - 116.0);
                }
            }
        },
        ClockKind::WorldTime => {
            let cities = super::world_cities(x.cfg);
            let (label, key) = cities.into_iter().nth(2).unwrap_or(("Tokyo".into(), "Tokyo Standard Time".into()));
            let (h, m, _, wd) = super::tz_now(&key).unwrap_or((n.hour, n.minute, 0, n.weekday));
            let dname = if wd == n.weekday { "Today" } else if wd == (n.weekday + 1) % 7 { "Tomorrow" } else { "Yesterday" };
            let diff = {
                let mut d = (h as i32 * 60 + m as i32) - (n.hour as i32 * 60 + n.minute as i32);
                if wd == (n.weekday + 1) % 7 {
                    d += 24 * 60;
                } else if wd != n.weekday {
                    d -= 24 * 60;
                }
                d as f32 / 60.0
            };
            let t = format!("{}:{:02}", h12(h), m);
            let offset = format!("{dname}, {}{:.0}h", if diff >= 0.0 { "+" } else { "" }, diff);
            match sh {
                Shape::Mini | Shape::Dock => compact_sum(c, &Sum::new(Vis::Glyph(G_CLOCK, c.text1()), t, label.clone()), sh),
                Shape::Small => {
                    c.circle(20.0, 22.0, 5.0, if (6..18).contains(&h) { YELLOW } else { hex(0x8E8EFF) });
                    c.tw(&label, 32.0, 14.0, 12.5, 600, c.text1(), c.w - 44.0);
                    let fs = fit(c, &t, 36.0, 18.0, 300, Font::Display, c.w - 50.0);
                    c.num(&t, 14.0, 40.0, fs, 300, c.text1());
                    c.tw(ampm(h), 18.0 + c.measure(&t, fs, 300, Font::Display).0, 40.0 + fs * 0.62, 11.0, 600, c.text2(), 30.0);
                    c.tw(&offset, 14.0, c.h - 26.0, 11.0, 400, c.text2(), c.w - 28.0);
                }
                _ => {
                    c.circle(22.0, 24.0, 5.0, if (6..18).contains(&h) { YELLOW } else { hex(0x8E8EFF) });
                    c.tw(&label, 34.0, 15.0, 13.0, 600, c.text1(), c.w * 0.5);
                    c.tw(&offset, 16.0, 40.0, 11.5, 400, c.text2(), c.w * 0.5);
                    let tw = c.measure(ampm(h), 14.0, 500, Font::Text).0;
                    c.text(&t, c.w - 22.0 - tw, 14.0, 40.0, 300, c.text1(), Align::Right, c.w, Font::Display);
                    c.tr(ampm(h), c.w - 16.0, 36.0, 14.0, 500, c.text2());
                    if sh == Shape::Large {
                        // The other cities too.
                        for (i, (l, key)) in super::world_cities(x.cfg).into_iter().take(2).enumerate() {
                            let (hh, mm, _, _) = super::tz_now(&key).unwrap_or((n.hour, n.minute, 0, n.weekday));
                            let y = 110.0 + i as f32 * 42.0;
                            c.line(16.0, y - 8.0, c.w - 16.0, y - 8.0, 1.0, c.ink(0.08));
                            c.tw(&l, 16.0, y + 4.0, 13.0, 600, c.text1(), c.w * 0.5);
                            c.tr(&format!("{}:{:02} {}", h12(hh), mm, ampm(hh)), c.w - 16.0, y, 20.0, 300, c.text1());
                        }
                    }
                }
            }
        }
        ClockKind::Today => {
            let next = x.hub.feed("calendar").and_then(|f| f.extra.get("next_title").map(|t| format!("{t} · {}", f.extra.get("next_time").cloned().unwrap_or_default())));
            match sh {
                Shape::Mini => day_tile(c, &n, sh),
                Shape::Dock => {
                    c.tw(&day.to_uppercase(), 10.0, c.h * 0.12, (c.h * 0.18).max(9.0), 700, RED, c.w - 12.0);
                    let t = format!("{} {}", &mon[..3], n.day);
                    c.num(&t, 10.0, c.h * 0.36, fit(c, &t, c.h * 0.36, 9.0, 600, Font::Display, c.w - 12.0), 600, c.text1());
                }
                Shape::Small => {
                    c.tw(&day.to_uppercase(), 14.0, 14.0, 11.0, 700, RED, c.w - 28.0);
                    c.num(&n.day.to_string(), 12.0, 28.0, 44.0, 400, c.text1());
                    c.tw(mon, 14.0, 84.0, 12.0, 600, c.text1(), c.w - 28.0);
                    c.tw(next.as_deref().unwrap_or("No more events"), 14.0, c.h - 20.0, 10.0, 400, c.text3(), c.w - 28.0);
                }
                _ => {
                    c.tw(&day.to_uppercase(), 16.0, 12.0, 11.5, 700, RED, 160.0);
                    c.num(&format!("{mon} {}", n.day), 16.0, 28.0, 26.0, 600, c.text1());
                    c.tr(&format!("{} {}", time, ampm(n.hour)), c.w - 16.0, 14.0, 13.0, 500, c.text2());
                    c.tw(&next.unwrap_or_else(|| "No more events today".into()), 16.0, 68.0, 11.5, 400, c.text2(), c.w - 32.0);
                    if sh == Shape::Large {
                        month_grid(c, &n, 14.0, 100.0, c.w - 28.0, c.h - 108.0);
                    }
                }
            }
        }
        ClockKind::Date => match sh {
            Shape::Mini => day_tile(c, &n, sh),
            Shape::Dock => {
                c.tw(day, 12.0, c.h * 0.14, (c.h * 0.19).max(9.5), 400, c.text2(), c.w);
                let t = format!("{} {}", &mon[..3], n.day);
                c.num(&t, 12.0, c.h * 0.38, fit(c, &t, c.h * 0.36, 10.0, 600, Font::Display, c.w - 16.0), 600, c.text1());
            }
            Shape::Small => {
                c.tw(day, 14.0, 16.0, 12.0, 400, c.text2(), c.w - 28.0);
                c.num(&n.day.to_string(), 12.0, 32.0, 46.0, 500, c.text1());
                c.tw(mon, 14.0, 90.0, 13.0, 600, c.text1(), c.w - 28.0);
            }
            _ => {
                c.tw(day, 18.0, 18.0, 13.0, 400, c.text2(), c.w);
                c.num(&format!("{mon} {}", n.day), 18.0, 36.0, 30.0, 600, c.text1());
                if sh == Shape::Large {
                    month_grid(c, &n, 14.0, 92.0, c.w - 28.0, c.h - 100.0);
                }
            }
        },
        ClockKind::DayPage => day_tile(c, &n, sh),
        ClockKind::Analog => {
            let r = c.h.min(c.w) / 2.0 - if matches!(sh, Shape::Mini | Shape::Dock) { 3.0 } else { 10.0 };
            dial(c, c.w / 2.0, c.h / 2.0, r, n.hour, n.minute, Some(n.second), !c.light);
        }
        ClockKind::AnalogSquare => {
            let dense = matches!(sh, Shape::Mini | Shape::Dock);
            let s = c.h.min(c.w) - if dense { 4.0 } else { 16.0 };
            if !dense && c.w > c.h * 1.3 {
                bg(c, false);
            }
            square_dial(c, (c.w - s) / 2.0, (c.h - s) / 2.0, s, n.hour, n.minute, n.second);
        }
        ClockKind::Segment => {
            let on = RED;
            let off = c.ink(0.06);
            let dense = matches!(sh, Shape::Mini | Shape::Dock);
            let pad = if dense { 6.0 } else { 14.0 };
            let top = if dense || sh == Shape::Small { pad } else { 26.0 };
            // Four digits (0.55 wide, spaced 1.25) and a colon (0.45), fitted both ways.
            let units = 4.0 * 0.55 * 1.25 + 0.45 * 0.55;
            let dh = (c.h - top - pad).min((c.w - 2.0 * pad) / units);
            let dw = dh * 0.55;
            let colon = dw * 0.45;
            let total = 4.0 * dw + 3.0 * dw * 0.25 + colon;
            let mut sx = (c.w - total) / 2.0;
            let y0 = top + (c.h - top - pad - dh) / 2.0;
            if !dense && sh != Shape::Small {
                c.tw(ampm(n.hour), 16.0, 8.0, 10.5, 700, RED, 40.0);
                c.tr(&date_short.to_uppercase(), c.w - 16.0, 8.0, 10.5, 700, c.text3());
            }
            let d = [h12(n.hour) / 10, h12(n.hour) % 10, n.minute / 10, n.minute % 10];
            for (i, v) in d.iter().enumerate() {
                if i == 2 {
                    let blink = if n.second % 2 == 0 { on } else { off };
                    c.circle(sx + colon / 2.0 - dw * 0.12, y0 + dh * 0.3, dw * 0.09, blink);
                    c.circle(sx + colon / 2.0 - dw * 0.12, y0 + dh * 0.7, dw * 0.09, blink);
                    sx += colon;
                }
                if !(i == 0 && *v == 0) {
                    seg_digit(c, sx, y0, dw, dh, *v, on, off);
                } else {
                    seg_digit(c, sx, y0, dw, dh, 8, off, off);
                }
                sx += dw * 1.25;
            }
        }
        ClockKind::Month => match sh {
            Shape::Mini => day_tile(c, &n, sh),
            Shape::Dock => week_row(c, &n, 6.0, 4.0, c.w - 12.0, c.h - 8.0),
            Shape::Medium => {
                c.tw(mon, 16.0, 10.0, 13.0, 600, c.text1(), 160.0);
                c.tr(&n.year.to_string(), c.w - 16.0, 10.0, 13.0, 600, RED);
                week_row(c, &n, 14.0, 32.0, c.w - 28.0, c.h - 40.0);
            }
            _ => {
                let pad = if sh == Shape::Small { 10.0 } else { 14.0 };
                let hs = if sh == Shape::Small { 11.5 } else { 14.0 };
                c.tw(if sh == Shape::Small { &mon[..3] } else { mon }, pad + 2.0, 10.0, hs, 600, c.text1(), 160.0);
                c.tr(&n.year.to_string(), c.w - pad - 2.0, 10.0, hs, 600, RED);
                month_grid(c, &n, pad, 32.0, c.w - 2.0 * pad, c.h - 38.0);
            }
        },
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
        2 => "⛅",
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

/// Columns of hours or days: label, emoji, temperature.
fn forecast_cols(c: &Canvas, items: &[(String, &'static str, String)], x0: f32, y0: f32, w: f32, small_temp: bool) {
    let n = items.len().max(1);
    let colw = w / n as f32;
    for (i, (l, e, t)) in items.iter().enumerate() {
        let cx = x0 + colw * (i as f32 + 0.5);
        c.tc(l, cx, y0, 10.5, 500, c.text2(), colw);
        c.text(e, cx, y0 + 15.0, 16.0, 400, c.text1(), Align::Center, colw, Font::Text);
        c.tc(t, cx, y0 + 40.0, if small_temp { 10.0 } else { 12.0 }, 600, c.text1(), colw);
    }
}

type Day = (String, &'static str, String, String);

fn weather(c: &Canvas, out: &mut Painted, k: WeatherKind, x: &Ctx, sh: Shape) {
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open(format!("https://www.msn.com/en-ca/weather/forecast/in-{}", x.cfg.weather_city.replace(' ', "-")))));
    let Some(w) = x.weather else {
        if k == WeatherKind::Banner && !matches!(sh, Shape::Mini | Shape::Dock) {
            bg(c, false);
        }
        compact_or_text(c, sh, Vis::Glyph(G_SUN, c.text3()), "—", &x.cfg.weather_city, "Fetching weather…");
        return;
    };
    let city = &x.cfg.weather_city;
    let temp = format!("{:.0}°", d0(w.temp_c));
    let emoji = weather_emoji(w.code, w.is_day);
    let hourly: Vec<(String, &'static str, String)> = w.hourly.iter().enumerate().map(|(i, (h, t, code))| (if i == 0 { "Now".to_string() } else { format!("{}{}", h12(*h as u32), if *h < 12 { "a" } else { "p" }) }, weather_emoji(*code, (6..19).contains(h)), format!("{:.0}°", d0(*t)))).collect();
    let daily: Vec<Day> = w.daily.iter().enumerate().map(|(i, (d, hi, lo, code))| (if i == 0 { "Today".to_string() } else { weekday_of(d) }, weather_emoji(*code, true), format!("{:.0}°", d0(*hi)), format!("{:.0}°", d0(*lo)))).collect();
    if matches!(sh, Shape::Mini | Shape::Dock) {
        let s = if k == WeatherKind::Sun {
            // The next sunrise or sunset, not the weather.
            let now_m = (x.now.hour * 60 + x.now.minute) as f32;
            let (label, hm, e) = if now_m >= mins(&w.sunrise) && now_m < mins(&w.sunset) { ("Sunset", &w.sunset, "🌇") } else { ("Sunrise", &w.sunrise, "🌅") };
            let m = mins(hm) as u32;
            let t = format!("{}:{:02}", h12(m / 60), m % 60);
            let t = if sh == Shape::Dock { format!("{t} {}", ampm(m / 60)) } else { t };
            Sum::new(Vis::Emoji(e), t, if sh == Shape::Dock { label } else { "" })
        } else {
            Sum::new(Vis::Emoji(emoji), temp, if sh == Shape::Dock { w.description.clone() } else { String::new() })
        };
        compact_sum(c, &s, sh);
        return;
    }
    if sh == Shape::Small {
        match k {
            WeatherKind::Banner => {
                let (top, bottom) = if w.is_day { (hex(0x4A90E2), hex(0x2D6CC0)) } else { (hex(0x1B2550), hex(0x0E1430)) };
                c.rr_grad(0.0, 0.0, c.w, c.h, 18.0, top, bottom);
                c.tw(city, 14.0, 14.0, 12.0, 600, WHITE, c.w - 28.0);
                c.num(&temp, 14.0, 32.0, 34.0, 300, WHITE);
                c.text(emoji, c.w - 30.0, 36.0, 24.0, 400, WHITE, Align::Center, 40.0, Font::Text);
                c.tw(&w.description, 14.0, c.h - 26.0, 11.0, 500, (1.0, 1.0, 1.0, 0.85), c.w - 28.0);
            }
            WeatherKind::Hourly | WeatherKind::Week => {
                c.tw(city, 12.0, 10.0, 11.5, 600, c.text1(), c.w - 50.0);
                c.tr(&temp, c.w - 12.0, 10.0, 11.5, 500, c.text2());
                let items: Vec<(String, &'static str, String)> = if k == WeatherKind::Hourly {
                    hourly.iter().take(3).cloned().collect()
                } else {
                    daily.iter().take(3).map(|(l, e, hi, _)| (if l == "Today" { l.clone() } else { l.chars().take(3).collect() }, *e, hi.clone())).collect()
                };
                forecast_cols(c, &items, 6.0, 40.0, c.w - 12.0, false);
            }
            WeatherKind::Sun => sun_arc(c, w, x, true),
            _ => {
                c.tw(city, 14.0, 12.0, 12.0, 600, c.text1(), c.w - 28.0);
                c.text(emoji, 14.0, 34.0, 26.0, 400, c.text1(), Align::Left, 40.0, Font::Text);
                c.num(&temp, 52.0, 30.0, 32.0, 300, c.text1());
                c.tw(&w.description, 14.0, 78.0, 11.0, 400, c.text2(), c.w - 28.0);
                c.tw(&format!("H:{:.0}° L:{:.0}°", d0(w.high_c), d0(w.low_c)), 14.0, 94.0, 10.5, 400, c.text3(), c.w - 28.0);
            }
        }
        return;
    }
    let large = sh == Shape::Large;
    match k {
        WeatherKind::Now => {
            c.text(emoji, 46.0, 18.0, 40.0, 400, c.text1(), Align::Center, 80.0, Font::Text);
            c.num(&temp, 90.0, 10.0, 40.0, 300, c.text1());
            c.tw(&format!("H:{:.0}°  L:{:.0}°", d0(w.high_c), d0(w.low_c)), 92.0, 62.0, 12.0, 400, c.text2(), 140.0);
            c.tr(city, c.w - 16.0, 16.0, 12.5, 600, c.text1());
            c.tr(&w.description, c.w - 16.0, 36.0, 11.5, 400, c.text2());
        }
        WeatherKind::Detail => {
            c.text(emoji, 34.0, 16.0, 30.0, 400, c.text1(), Align::Center, 60.0, Font::Text);
            c.num(&temp, 62.0, 10.0, 34.0, 300, c.text1());
            c.tw(&w.description, 18.0, 62.0, 12.0, 400, c.text2(), c.w * 0.5);
            let rows = [("Feels like", format!("{:.0}°", d0(w.feels_c))), ("Humidity", format!("{:.0}%", w.humidity)), ("Wind", format!("{:.0} km/h", w.wind_kmh)), ("UV", format!("{:.0}", w.uv))];
            for (i, (l, v)) in rows.iter().enumerate() {
                let y = 12.0 + i as f32 * 18.0;
                c.tw(l, c.w * 0.55, y, 11.0, 400, c.text3(), 80.0);
                c.tr(v, c.w - 16.0, y, 11.5, 600, c.text1());
            }
        }
        WeatherKind::Hourly => {
            c.tw(city, 16.0, 10.0, 12.0, 600, c.text1(), c.w * 0.5);
            c.tr(&format!("{temp}  {}", w.description), c.w - 16.0, 10.0, 11.5, 400, c.text2());
            forecast_cols(c, &hourly.iter().take(6).cloned().collect::<Vec<_>>(), 12.0, 32.0, c.w - 24.0, false);
        }
        WeatherKind::Week if !large => {
            c.tw(city, 16.0, 10.0, 12.0, 600, c.text1(), c.w * 0.5);
            c.tr(&format!("{temp}  {}", w.description), c.w - 16.0, 10.0, 11.5, 400, c.text2());
            // Day, icon, then the high bright and the low dimmed, so pairs read apart.
            let days: Vec<&Day> = daily.iter().take(7).collect();
            let colw = (c.w - 24.0) / days.len().max(1) as f32;
            for (i, (l, e, hi, lo)) in days.iter().enumerate() {
                let cx = 12.0 + colw * (i as f32 + 0.5);
                c.tc(l, cx, 32.0, 10.5, 500, c.text2(), colw);
                c.text(e, cx, 47.0, 16.0, 400, c.text1(), Align::Center, colw, Font::Text);
                let (hw, lw) = (c.measure(hi, 10.5, 600, Font::Text).0, c.measure(lo, 10.0, 400, Font::Text).0);
                let x0 = cx - (hw + 2.0 + lw) / 2.0;
                c.tw(hi, x0, 72.0, 10.5, 600, c.text1(), hw + 2.0);
                c.tw(lo, x0 + hw + 2.0, 72.5, 10.0, 400, c.text3(), lw + 2.0);
            }
        }
        WeatherKind::Week => {
            c.tw(city, 16.0, 12.0, 12.0, 600, c.text1(), c.w * 0.5);
            c.tr(&format!("{temp}  {}", w.description), c.w - 16.0, 12.0, 11.5, 400, c.text2());
        }
        WeatherKind::Banner => {
            let (top, bottom) = if w.is_day { (hex(0x4A90E2), hex(0x2D6CC0)) } else { (hex(0x1B2550), hex(0x0E1430)) };
            c.rr_grad(0.0, 0.0, c.w, c.h, 18.0, top, bottom);
            c.tw(&w.description, 18.0, 18.0, 16.0, 600, WHITE, c.w * 0.55);
            c.tw(city, 18.0, 42.0, 12.0, 400, (1.0, 1.0, 1.0, 0.8), c.w * 0.55);
            c.tw(&format!("H:{:.0}°  L:{:.0}°", d0(w.high_c), d0(w.low_c)), 18.0, 64.0, 11.5, 400, (1.0, 1.0, 1.0, 0.8), c.w * 0.55);
            c.text(&temp, c.w - 70.0, 18.0, 44.0, 300, WHITE, Align::Right, c.w, Font::Display);
            c.text(emoji, c.w - 40.0, 44.0, 30.0, 400, WHITE, Align::Center, 60.0, Font::Text);
        }
        WeatherKind::Sun => sun_arc(c, w, x, false),
    }
    if large && k != WeatherKind::Sun {
        // The week underneath (for the weekly card, from the top).
        let start = if k == WeatherKind::Week { 40.0 } else { 108.0 };
        if k != WeatherKind::Week {
            c.line(14.0, start - 8.0, c.w - 14.0, start - 8.0, 1.0, if k == WeatherKind::Banner { (1.0, 1.0, 1.0, 0.2) } else { c.ink(0.08) });
        }
        let ink = if k == WeatherKind::Banner { WHITE } else { c.text1() };
        let n = (((c.h - start - 6.0) / 22.0).floor() as usize).min(daily.len());
        day_rows(c, &daily[..n], start, ink);
    }
}

/// Rows of days with a temperature range bar.
fn day_rows(c: &Canvas, days: &[Day], start: f32, ink: Col) {
    let t = |s: &str| s.trim_end_matches('°').parse::<f32>().unwrap_or(0.0);
    let (lo_t, hi_t) = days.iter().fold((f32::MAX, f32::MIN), |acc, d| (acc.0.min(t(&d.3)), acc.1.max(t(&d.2))));
    let span = (hi_t - lo_t).max(1.0);
    for (i, (l, e, hi, lo)) in days.iter().enumerate() {
        let y = start + i as f32 * 22.0;
        c.tw(l, 16.0, y, 11.5, 500, ink, 60.0);
        c.text(e, 86.0, y - 2.0, 14.0, 400, ink, Align::Center, 30.0, Font::Text);
        c.tr(lo, 132.0, y, 11.5, 400, alpha(ink, 0.6));
        let (bx, bw) = (142.0, c.w - 142.0 - 50.0);
        c.rr(bx, y + 6.0, bw, 4.0, 2.0, alpha(ink, 0.12));
        let a = bx + bw * (t(lo) - lo_t) / span;
        let b = bx + bw * (t(hi) - lo_t) / span;
        c.rr(a, y + 6.0, (b - a).max(4.0), 4.0, 2.0, ORANGE);
        c.tr(hi, c.w - 16.0, y, 11.5, 600, ink);
    }
}

fn sun_arc(c: &Canvas, w: &laminar_sys::weather::Weather, x: &Ctx, small: bool) {
    let (rise, set) = (mins(&w.sunrise), mins(&w.sunset));
    let now_m = (x.now.hour * 60 + x.now.minute) as f32;
    let frac = if set > rise { ((now_m - rise) / (set - rise)).clamp(0.0, 1.0) } else { 0.5 };
    let bottom = if c.h > 150.0 { c.h - 40.0 } else { c.h - 22.0 };
    let r = (c.w * 0.36).min(bottom - 30.0).min(if c.h > 150.0 { 110.0 } else { 56.0 });
    let (cx, cy) = (c.w / 2.0, bottom);
    c.arc(cx, cy, r, 2.0, 180.0, 360.0, c.ink(0.2));
    c.arc(cx, cy, r, 2.5, 180.0, 180.0 + 180.0 * frac, ORANGE);
    let a = (180.0 + 180.0 * frac).to_radians();
    c.circle(cx + r * a.cos(), cy + r * a.sin(), 6.0, YELLOW);
    c.line(12.0, cy, c.w - 12.0, cy, 1.0, c.ink(0.15));
    let fs = if small { 10.0 } else { 11.5 };
    c.tw(&format!("↑ {}", w.sunrise), 12.0, cy + 4.0, fs, 500, c.text2(), 80.0);
    c.tr(&format!("{} ↓", w.sunset), c.w - 12.0, cy + 4.0, fs, 500, c.text2());
    if !small {
        c.tw("Sunrise & Sunset", 16.0, 10.0, 11.5, 600, c.text2(), 140.0);
        let left = (set - now_m).max(0.0) as u64 * 60;
        c.tr(&if left > 0 { format!("{} of daylight left", dur(left)) } else { "After sunset".into() }, c.w - 16.0, 10.0, 11.0, 400, c.text3());
    }
}

/// For faces without data yet: the compact layout in the dock, a line of text in cards.
fn compact_or_text(c: &Canvas, sh: Shape, vis: Vis, value: &str, caption: &str, text: &str) {
    match sh {
        Shape::Mini | Shape::Dock => compact_sum(c, &Sum::new(vis, value, caption), sh),
        _ => {
            c.tw(caption, 16.0, 16.0, 13.0, 600, c.text1(), c.w - 32.0);
            c.text(text, 16.0, 40.0, 12.0, 400, c.text2(), Align::Left, c.w - 32.0, Font::Text);
        }
    }
}

// ------------------------------------------------------------------ timers

fn timer(c: &Canvas, out: &mut Painted, k: TimerKind, x: &Ctx, sh: Shape) {
    let (col, label, text, running, frac, done) = match k {
        TimerKind::Stopwatch => {
            let e = x.hub.stopwatch.elapsed().as_secs_f32();
            (ORANGE, "Stopwatch".to_string(), hms(e), x.hub.stopwatch.start.is_some(), None, false)
        }
        _ => {
            let t = &x.hub.timers[&k];
            let left = t.left();
            let label = match (k, t.phase, t.done) {
                (_, _, true) => "Done!".to_string(),
                (TimerKind::Pomodoro, 0, _) => "Focus".to_string(),
                (TimerKind::Pomodoro, _, _) => "Break".to_string(),
                _ => "Timer".to_string(),
            };
            (if k == TimerKind::Pomodoro { RED } else { BLUE }, label, hms(left), t.running(), Some(1.0 - left / t.total.max(1) as f32), t.done)
        }
    };
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Timer(k)));
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    if running || done {
        c.rr_stroke(0.0, 0.0, c.w, c.h, if dense { 12.0 } else { 18.0 }, alpha(col, 0.9), 1.6);
    }
    let play = if running { G_PAUSE } else { G_PLAY };
    match sh {
        Shape::Mini => {
            // A ring around the time.
            let r = c.h * 0.42;
            c.ring(c.w / 2.0, c.h / 2.0, r, 3.0, frac.unwrap_or(if running { 1.0 } else { 0.0 }), col);
            let fs = fit(c, &text, c.h * 0.24, 7.0, 600, Font::Display, r * 1.6);
            c.text(&text, c.w / 2.0, c.h / 2.0 - fs * 0.68, fs, 600, c.text1(), Align::Center, r * 2.0, Font::Display);
        }
        Shape::Dock => {
            let r = c.h * 0.3;
            let (cx, cy) = (c.h / 2.0, c.h / 2.0);
            match frac {
                Some(f) => c.ring(cx, cy, r, 3.5, f, col),
                None => c.circle(cx, cy, r, alpha(col, 0.9)),
            }
            c.glyph(play, cx, cy, r * 0.8, if frac.is_some() { col } else { WHITE });
            let x0 = c.h * 0.96;
            let vs = (c.h * 0.34).max(13.0);
            let cs = (c.h * 0.19).max(9.5);
            let top = (c.h - vs * 1.25 - cs * 1.3) / 2.0 - 1.0;
            c.text(&text, x0, top, vs, 600, c.text1(), Align::Left, c.w - x0 - 6.0, Font::Display);
            c.tw(&label, x0, top + vs * 1.25, cs, 400, c.text2(), c.w - x0 - 6.0);
        }
        Shape::Small => {
            c.tw(&label, 14.0, 12.0, 11.5, 600, col, c.w - 28.0);
            let fs = fit(c, &text, 30.0, 16.0, 400, Font::Display, c.w - 28.0);
            c.text(&text, 14.0, 28.0, fs, 400, c.text1(), Align::Left, c.w - 28.0, Font::Display);
            let r = 16.0;
            let (bx, by) = (c.w - 12.0 - r, c.h - 10.0 - r);
            c.circle(bx, by, r, col);
            c.glyph(play, bx, by, 13.0, WHITE);
            let rx = bx - r - 20.0;
            c.circle(rx, by, 13.0, c.ink(0.12));
            c.glyph(G_RESET, rx, by, 10.0, c.text1());
            out.hits.push(((rx - 14.0, by - 14.0, 28.0, 28.0), WAct::TimerReset(k)));
        }
        Shape::Medium => {
            c.tw(&label, 18.0, 16.0, 12.0, 600, col, 140.0);
            c.text(&text, 18.0, 34.0, 38.0, 400, c.text1(), Align::Left, c.w * 0.6, Font::Display);
            let r = 22.0;
            let (bx, by) = (c.w - 18.0 - r, c.h / 2.0);
            c.circle(bx, by, r, col);
            c.glyph(play, bx, by, 16.0, WHITE);
            let (rx, ry) = (bx - r - 26.0, by);
            c.circle(rx, ry, 15.0, c.ink(0.12));
            c.glyph(G_RESET, rx, ry, 12.0, c.text1());
            out.hits.push(((rx - 16.0, ry - 16.0, 32.0, 32.0), WAct::TimerReset(k)));
        }
        Shape::Large => {
            // A big ring with the time inside, buttons under it.
            // Card padding (16) above the ring and below the buttons, like the other cards.
            let r = ((c.h - 92.0) / 2.0).min(c.w / 2.0 - 30.0);
            let (cx, cy) = (c.w / 2.0, 20.0 + r);
            c.ring(cx, cy, r, 7.0, frac.unwrap_or(0.0), col);
            c.tc(&label, cx, cy - 34.0, 12.0, 600, col, 2.0 * r);
            let fs = fit(c, &text, 34.0, 16.0, 400, Font::Display, 2.0 * r - 24.0);
            c.text(&text, cx, cy - fs * 0.62, fs, 400, c.text1(), Align::Center, 2.0 * r, Font::Display);
            let by = c.h - 34.0;
            c.circle(cx + 26.0, by, 18.0, col);
            c.glyph(play, cx + 26.0, by, 14.0, WHITE);
            c.circle(cx - 26.0, by, 18.0, c.ink(0.12));
            c.glyph(G_RESET, cx - 26.0, by, 12.0, c.text1());
            out.hits.push(((cx - 46.0, by - 20.0, 40.0, 40.0), WAct::TimerReset(k)));
        }
    }
}

// ------------------------------------------------------------------ media

#[allow(clippy::too_many_arguments)]
fn art_src(x: &Ctx, wic: &IWICImagingFactory) -> Option<IWICBitmapSource> {
    x.hub.art_img.as_ref().map(|a| a.1.clone()).or_else(|| x.control.media.as_ref().and_then(|m| m.thumbnail_png.as_ref()).and_then(|b| load_image_bytes(wic, b)))
}

fn art(c: &Canvas, x: &Ctx, wic: &IWICImagingFactory, ax: f32, ay: f32, s: f32, r: f32) {
    match art_src(x, wic) {
        Some(img) => c.image(&img, ax, ay, s, s, r),
        None => {
            c.rr_grad(ax, ay, s, s, r, hex(0xFF5F6D), hex(0xC2185B));
            c.glyph(G_MUSIC, ax + s / 2.0, ay + s / 2.0, s * 0.42, (1.0, 1.0, 1.0, 0.95));
        }
    }
}

fn controls(c: &Canvas, out: &mut Painted, cx: f32, cy: f32, gap: f32, playing: bool) {
    for (i, (g, a)) in [(G_PREV, WAct::MediaPrev), (if playing { G_PAUSE } else { G_PLAY }, WAct::MediaPlay), (G_NEXT, WAct::MediaNext)].into_iter().enumerate() {
        let x = cx + (i as f32 - 1.0) * gap;
        c.glyph(g, x, cy, if i == 1 { 18.0 } else { 14.0 }, c.text1());
        out.hits.push(((x - 16.0, cy - 16.0, 32.0, 32.0), a));
    }
}

fn media(c: &Canvas, out: &mut Painted, k: MediaKind, x: &Ctx, wic: &IWICImagingFactory, sh: Shape) {
    let m = x.control.media.as_ref();
    let (title, artist, playing) = match m {
        Some(m) => (m.title.clone(), m.artist.clone(), m.playing),
        None => ("Nothing playing".to_string(), String::new(), false),
    };
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    match k {
        MediaKind::Output => {
            let vol = x.control.volume.unwrap_or(0.0);
            let muted = x.control.muted == Some(true);
            let g = if muted { G_MUTE } else { G_VOL };
            let pct = format!("{:.0}%", vol * 100.0);
            let dev = x.hub.output_name.clone().unwrap_or_else(|| "Sound".into());
            if dense {
                let mut s = Sum::new(Vis::Glyph(g, c.text1()), pct, dev);
                s.meter = Some((vol, BLUE));
                compact_sum(c, &s, sh);
                out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Mute));
                return;
            }
            let small = sh == Shape::Small;
            c.glyph(g, 26.0, 26.0, 16.0, c.text1());
            out.hits.push(((10.0, 10.0, 32.0, 32.0), WAct::Mute));
            c.tw(if small { "Sound" } else { &dev }, 46.0, 17.0, 12.5, 600, c.text1(), c.w - 100.0);
            c.tr(&pct, c.w - 16.0, 17.0, 12.0, 500, c.text2());
            let (bx, bw, bh) = (16.0, c.w - 32.0, 24.0);
            let by = if sh == Shape::Large { c.h / 2.0 - bh / 2.0 } else { c.h - bh - 16.0 };
            c.rr(bx, by, bw, bh, bh / 2.0, c.ink(0.12));
            c.rr(bx, by, (bw * vol).max(bh), bh, bh / 2.0, if muted { c.ink(0.3) } else { BLUE });
            out.hits.push(((bx, by - 4.0, bw, bh + 8.0), WAct::Volume));
            if small {
                c.tw(&dev, 16.0, 48.0, 10.5, 400, c.text3(), c.w - 32.0);
            }
            return;
        }
        MediaKind::Mic => {
            let muted = x.hub.mic_muted == Some(true);
            tile_sub(c, out, G_MIC, "Microphone", if muted { "Muted" } else { "On" }, muted, RED, WAct::Mic, dense);
            if muted {
                // A slash across the plate's microphone.
                let (cx, cy, s) = plate_center(c, dense);
                c.line(cx - s * 0.2, cy - s * 0.2, cx + s * 0.2, cy + s * 0.2, 2.0, WHITE);
            }
            return;
        }
        _ => {}
    }
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::MediaPlay));
    if m.is_none() {
        if !dense && k == MediaKind::Disc {
            bg(c, false);
        }
        compact_or_text(c, sh, Vis::Glyph(G_MUSIC, c.text3()), if sh == Shape::Mini { "" } else { "Not playing" }, "Nothing playing", "Play something in any app to control it here.");
        return;
    }
    if k == MediaKind::Disc {
        if !dense {
            bg(c, false);
        }
        let s = c.h.min(c.w) - if dense { 6.0 } else { 16.0 };
        let (ax, ay) = ((c.w - s) / 2.0, (c.h - s) / 2.0);
        let (cx, cy) = (c.w / 2.0, c.h / 2.0);
        if art_src(x, wic).is_some() {
            art(c, x, wic, ax, ay, s, s / 2.0);
        } else {
            // No cover: a record, with grooves and a coloured label.
            c.circle(cx, cy, s / 2.0, (0.07, 0.07, 0.08, 1.0));
            for k in 1..5 {
                c.circle_stroke(cx, cy, s / 2.0 * (0.48 + k as f32 * 0.11), (1.0, 1.0, 1.0, 0.06), 1.0);
            }
            c.rr_grad(cx - s * 0.19, cy - s * 0.19, s * 0.38, s * 0.38, s * 0.19, hex(0xFF5F6D), hex(0xC2185B));
        }
        c.circle_stroke(cx, cy, s / 2.0 - 0.5, (1.0, 1.0, 1.0, 0.2), 1.0);
        if playing {
            c.circle(cx, cy, (s * 0.05).max(2.0), (0.1, 0.1, 0.12, 1.0));
        } else {
            // Paused: a clear play button where the spindle would be.
            c.circle(cx, cy, s * 0.17, (0.0, 0.0, 0.0, 0.55));
            c.glyph(G_PLAY, cx + s * 0.01, cy, s * 0.15, WHITE);
        }
        return;
    }
    match sh {
        Shape::Mini => {
            let s = c.h - 8.0;
            art(c, x, wic, (c.w - s) / 2.0, 4.0, s, 9.0);
            if !playing {
                c.glyph(G_PLAY, c.w / 2.0, c.h / 2.0, s * 0.3, WHITE);
            }
        }
        Shape::Dock => {
            let s = c.h - 10.0;
            art(c, x, wic, 5.0, 5.0, s, 8.0);
            let tx = s + 12.0;
            let room = if c.w > c.h * 3.2 { c.w - tx - 34.0 } else { c.w - tx - 6.0 };
            c.tw(&title, tx, c.h * 0.16, (c.h * 0.24).max(10.5), 600, c.text1(), room);
            c.tw(&artist, tx, c.h * 0.52, (c.h * 0.2).max(9.5), 400, c.text2(), room);
            if c.w > c.h * 3.2 {
                c.glyph(if playing { G_PAUSE } else { G_PLAY }, c.w - 18.0, c.h / 2.0, 13.0, c.text1());
            }
        }
        Shape::Small => {
            art(c, x, wic, 14.0, 14.0, 44.0, 9.0);
            c.tw(&title, 14.0, 64.0, 12.5, 600, c.text1(), c.w - 28.0);
            c.tw(&artist, 14.0, 80.0, 10.5, 400, c.text2(), c.w - 28.0);
            if k == MediaKind::Full {
                out.hits.clear();
                // The tile itself plays/pauses; the buttons along the bottom do the rest.
                out.hits.push(((0.0, 0.0, c.w, c.h - 26.0), WAct::MediaPlay));
                controls(c, out, c.w / 2.0, c.h - 12.0, 34.0, playing);
            } else {
                c.circle(c.w - 30.0, 36.0, 15.0, c.ink(0.12));
                c.glyph(if playing { G_PAUSE } else { G_PLAY }, c.w - 30.0, 36.0, 12.0, c.text1());
            }
        }
        Shape::Large => {
            let s = (c.h - 74.0).min(c.w * 0.5);
            art(c, x, wic, 16.0, 16.0, s, 14.0);
            let tx = 16.0 + s + 14.0;
            c.text(&title, tx, 20.0, 15.0, 600, c.text1(), Align::Left, c.w - tx - 14.0, Font::Text);
            c.tw(&artist, tx, 44.0, 12.0, 400, c.text2(), c.w - tx - 14.0);
            out.hits.clear();
            controls(c, out, c.w / 2.0, c.h - 28.0, 54.0, playing);
        }
        Shape::Medium => match k {
            MediaKind::Full => {
                art(c, x, wic, 14.0, 14.0, 68.0, 12.0);
                c.tw(&title, 94.0, 16.0, 13.5, 600, c.text1(), c.w - 108.0);
                c.tw(&artist, 94.0, 36.0, 11.5, 400, c.text2(), c.w - 108.0);
                out.hits.clear();
                controls(c, out, 140.0, 70.0, 36.0, playing);
            }
            MediaKind::Compact => {
                art(c, x, wic, 16.0, (c.h - 48.0) / 2.0, 48.0, 10.0);
                c.tw(&title, 76.0, c.h / 2.0 - 18.0, 13.0, 600, c.text1(), c.w - 140.0);
                c.tw(&artist, 76.0, c.h / 2.0 + 2.0, 11.5, 400, c.text2(), c.w - 140.0);
                c.circle(c.w - 36.0, c.h / 2.0, 18.0, c.ink(0.12));
                c.glyph(if playing { G_PAUSE } else { G_PLAY }, c.w - 36.0, c.h / 2.0, 14.0, c.text1());
            }
            _ => {
                art(c, x, wic, 16.0, 14.0, 40.0, 8.0);
                c.tw(&title, 66.0, 15.0, 13.0, 600, c.text1(), c.w - 120.0);
                c.tw(&artist, 66.0, 34.0, 11.5, 400, c.text2(), c.w - 120.0);
                c.glyph(if playing { G_PAUSE } else { G_PLAY }, c.w - 30.0, 34.0, 16.0, c.text1());
                // A level meter that sways while playing.
                let n = 28;
                let bw = (c.w - 32.0) / n as f32;
                for i in 0..n {
                    let ph = (x.now.second * 7 + i * 13) % 17;
                    let hgt = if playing { 3.0 + ph as f32 * 0.9 } else { 3.0 };
                    c.rr(16.0 + i as f32 * bw, c.h - 12.0 - hgt, bw * 0.6, hgt, 1.5, if playing { alpha(PURPLE, 0.85) } else { c.ink(0.2) });
                }
            }
        },
    }
}

// ------------------------------------------------------------------ system

fn rate(bps: f64) -> String {
    if bps >= 1_000_000.0 {
        format!("{:.1} MB/s", bps / 1_000_000.0)
    } else {
        format!("{:.0} KB/s", bps / 1000.0)
    }
}

/// A ring face (CPU, memory, battery, disk) at every size.
#[allow(clippy::too_many_arguments)]
fn ring_face(c: &Canvas, sh: Shape, frac: f32, col: Col, big: &str, label: &str, sub: &str, series: Option<Vec<f64>>) {
    match sh {
        Shape::Mini => compact_sum(c, &Sum::new(Vis::Ring(frac, col, big.to_string()), "", ""), sh),
        Shape::Dock => compact_sum(c, &Sum::new(Vis::Ring(frac, col, String::new()), big, label), sh),
        Shape::Small => {
            let r = 30.0;
            c.ring(c.w / 2.0, 14.0 + r, r - 4.0, 7.0, frac, col);
            c.tc(big, c.w / 2.0, 14.0 + r - 9.0, 13.5, 600, c.text1(), 2.0 * r);
            c.tc(label, c.w / 2.0, 82.0, 12.5, 600, c.text1(), c.w - 16.0);
            c.tc(sub, c.w / 2.0, 99.0, 10.5, 400, c.text2(), c.w - 16.0);
        }
        _ => {
            let big_card = sh == Shape::Large;
            let r = if big_card { 44.0 } else { 30.0 };
            let (cx, cy) = (18.0 + r, if big_card { 16.0 + r } else { c.h / 2.0 });
            c.ring(cx, cy, r, if big_card { 9.0 } else { 7.0 }, frac, col);
            c.tc(big, cx, cy - if big_card { 11.0 } else { 9.0 }, if big_card { 17.0 } else { 13.5 }, 600, c.text1(), 2.0 * r);
            let tx = cx + r + 18.0;
            c.tw(label, tx, cy - 22.0, 13.0, 600, c.text1(), c.w - tx - 14.0);
            c.tw(sub, tx, cy - 2.0, 11.5, 400, c.text2(), c.w - tx - 14.0);
            if let Some(sv) = series.filter(|s| s.len() > 2) {
                if big_card {
                    c.area(14.0, 2.0 * r + 34.0, c.w - 28.0, c.h - 2.0 * r - 48.0, &sv, col, true);
                } else {
                    c.area(tx, 60.0, c.w - tx - 16.0, 22.0, &sv, col, true);
                }
            }
        }
    }
}

fn sys(c: &Canvas, out: &mut Painted, k: SysKind, x: &Ctx, sh: Shape) {
    let st = x.stats;
    let cpu = st.map(|s| s.cpu_percent).unwrap_or(0.0);
    let (mu, mt) = st.map(|s| (s.mem_used_gb, s.mem_total_gb)).unwrap_or((0.0, 0.0));
    let memf = if mt > 0.0 { mu / mt } else { 0.0 };
    if matches!(k, SysKind::Resource | SysKind::Cpu | SysKind::Memory | SysKind::Network) {
        out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("taskmgr".into())));
    }
    let cpu_hist: Vec<f64> = x.hub.cpu_hist.iter().map(|v| *v as f64).collect();
    match k {
        SysKind::Resource => match sh {
            Shape::Mini => compact_sum(c, &Sum::new(Vis::Ring(cpu / 100.0, BLUE, format!("{cpu:.0}")), "", ""), sh),
            Shape::Dock => {
                let r = c.h * 0.3;
                c.ring(c.h / 2.0, c.h / 2.0, r, 4.0, cpu / 100.0, BLUE);
                let tx = c.h;
                let fs = (c.h * 0.22).max(10.0);
                c.tw(&format!("CPU {cpu:.0}%"), tx, c.h * 0.2, fs, 600, c.text1(), c.w - tx - 6.0);
                c.tw(&format!("RAM {:.0}%", memf * 100.0), tx, c.h * 0.54, fs, 600, c.text2(), c.w - tx - 6.0);
            }
            Shape::Small => {
                let r = 28.0;
                c.ring(c.w / 2.0, 12.0 + r, r - 4.0, 7.0, cpu / 100.0, GREEN);
                c.tc(&format!("{cpu:.0}%"), c.w / 2.0, 12.0 + r - 15.0, 13.0, 600, c.text1(), 2.0 * r);
                c.tc("CPU", c.w / 2.0, 12.0 + r + 1.0, 9.0, 600, c.text3(), 2.0 * r);
                let y = 82.0;
                c.tw("MEM", 14.0, y, 10.5, 600, c.text2(), 40.0);
                c.bar(48.0, y + 5.0, c.w - 48.0 - 46.0, 5.0, memf, PURPLE);
                c.tr(&format!("{:.0}%", memf * 100.0), c.w - 14.0, y, 11.0, 600, c.text1());
            }
            _ => {
                c.ring(48.0, 48.0, 30.0, 7.0, cpu / 100.0, GREEN);
                c.tc(&format!("{cpu:.0}%"), 48.0, 48.0 - 14.0, 13.5, 600, c.text1(), 60.0);
                c.tc("CPU", 48.0, 48.0 + 3.0, 9.5, 600, c.text3(), 60.0);
                for (i, (l, f, v, col)) in [("CPU", cpu / 100.0, format!("{cpu:.0}%"), BLUE), ("MEM", memf, format!("{:.0}%", memf * 100.0), PURPLE)].iter().enumerate() {
                    let y = 26.0 + i as f32 * 26.0;
                    c.tw(l, 96.0, y, 11.0, 600, c.text2(), 40.0);
                    c.bar(132.0, y + 5.0, c.w - 132.0 - 56.0, 5.0, *f, *col);
                    c.tr(v, c.w - 16.0, y, 12.0, 600, c.text1());
                }
                if sh == Shape::Large {
                    c.tw(&format!("{mu:.1} of {mt:.0} GB memory in use"), 16.0, 98.0, 11.0, 400, c.text3(), c.w - 32.0);
                    c.area(14.0, 120.0, c.w - 28.0, c.h - 134.0, &cpu_hist, BLUE, true);
                }
            }
        },
        SysKind::Cpu => ring_face(c, sh, cpu / 100.0, BLUE, &format!("{cpu:.0}%"), "CPU", "Processor load", Some(cpu_hist)),
        SysKind::Memory => ring_face(c, sh, memf, PURPLE, &format!("{:.0}%", memf * 100.0), "Memory", &format!("{mu:.1} of {mt:.0} GB"), None),
        SysKind::Network => {
            let (d, u) = st.map(|s| (s.net_down_bps, s.net_up_bps)).unwrap_or((0.0, 0.0));
            let down: Vec<f64> = x.hub.net_hist.iter().map(|v| v.0).collect();
            let up: Vec<f64> = x.hub.net_hist.iter().map(|v| v.1).collect();
            let short = |b: f64| rate(b).replace(" KB/s", "K").replace(" MB/s", "M");
            match sh {
                Shape::Mini => {
                    let fs = (c.h * 0.2).max(8.0);
                    c.tc(&format!("↓{}", short(d)), c.w / 2.0, c.h * 0.18, fs, 600, BLUE, c.w);
                    c.tc(&format!("↑{}", short(u)), c.w / 2.0, c.h * 0.54, fs, 600, GREEN, c.w);
                }
                Shape::Dock => {
                    let s = (c.h * 0.24).max(10.0);
                    c.tw(&format!("↓ {}", rate(d)), 10.0, c.h * 0.16, s, 600, BLUE, c.w);
                    c.tw(&format!("↑ {}", rate(u)), 10.0, c.h * 0.52, s, 600, GREEN, c.w);
                }
                Shape::Small => {
                    c.tw("Network", 14.0, 12.0, 11.5, 600, c.text2(), 100.0);
                    c.tw(&format!("↓ {}", rate(d)), 14.0, 32.0, 14.0, 600, BLUE, c.w - 28.0);
                    c.tw(&format!("↑ {}", rate(u)), 14.0, 52.0, 14.0, 600, GREEN, c.w - 28.0);
                    c.area(14.0, 80.0, c.w - 28.0, c.h - 92.0, &down, BLUE, true);
                }
                _ => {
                    c.tw("Network", 16.0, 12.0, 12.0, 600, c.text2(), 100.0);
                    c.tw(&format!("↓ {}", rate(d)), 16.0, 32.0, 15.0, 600, BLUE, c.w / 2.0);
                    c.tw(&format!("↑ {}", rate(u)), c.w / 2.0, 32.0, 15.0, 600, GREEN, c.w / 2.0 - 16.0);
                    let gh = c.h - 70.0;
                    c.area(16.0, 58.0, c.w - 32.0, gh, &down, BLUE, true);
                    c.area(16.0, 58.0, c.w - 32.0, gh, &up, GREEN, false);
                }
            }
        }
        SysKind::Battery | SysKind::Disk | SysKind::Storage | SysKind::Language | SysKind::Uptime => {
            let conn = match k {
                SysKind::Battery => "battery",
                SysKind::Language => "language",
                SysKind::Uptime => "uptime",
                _ => "storage",
            };
            let badge = super::canvas::gly(G_PLUG, 0x2C2C34, 0x30D158);
            let g = got(x, conn);
            let Got::Feed(f) = g else {
                let name = match k {
                    SysKind::Battery => "Battery",
                    SysKind::Language => "Keyboard Language",
                    SysKind::Uptime => "Uptime",
                    _ => "Storage",
                };
                placeholder(c, out, name, &badge, conn, &g, sh);
                return;
            };
            match k {
                SysKind::Battery => {
                    let charging = f.extra.get("charging").map(|v| v == "true").unwrap_or(false);
                    let frac = f.frac.unwrap_or(0.0);
                    let col = if frac < 0.2 && !charging { RED } else { GREEN };
                    let sub = if charging { "Charging".to_string() } else { f.extra.get("time_left").filter(|s| !s.is_empty()).map(|t| format!("{t} left")).unwrap_or_else(|| "On battery".into()) };
                    ring_face(c, sh, frac, col, &format!("{:.0}%", frac * 100.0), "Battery", &sub, None);
                }
                SysKind::Disk => {
                    let used = f.frac.unwrap_or(0.0);
                    ring_face(c, sh, used, ORANGE, &format!("{:.0}%", used * 100.0), "Disk C:", &format!("{} GB free", compact(f.value.unwrap_or(0.0))), None);
                    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("ms-settings:storagesense".into())));
                }
                SysKind::Storage => {
                    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("ms-settings:storagesense".into())));
                    let used = f.frac.unwrap_or(0.0);
                    if compactish(sh) {
                        let mut s = Sum::new(Vis::Glyph(G_DRIVE, c.text2()), format!("{:.0} GB", f.value.unwrap_or(0.0)), "Free on C:");
                        s.meter = Some((used, if used > 0.9 { RED } else { BLUE }));
                        compact_sum(c, &s, sh);
                        return;
                    }
                    c.num(&format!("{:.0} GB", f.value.unwrap_or(0.0)), 16.0, 14.0, 28.0, 600, c.text1());
                    c.tr("Free on C:", c.w - 16.0, 22.0, 12.0, 400, c.text2());
                    c.bar(16.0, 58.0, c.w - 32.0, 8.0, used, if used > 0.9 { RED } else { BLUE });
                    if sh == Shape::Large {
                        details(c, &f, BLUE);
                    } else {
                        let total = f.rows.first().map(|r| r.value.clone()).unwrap_or_default();
                        c.tw(&total, 16.0, 72.0, 11.0, 400, c.text3(), c.w - 32.0);
                    }
                }
                SysKind::Language => {
                    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Language));
                    let code = f.text.clone().unwrap_or_else(|| "—".into());
                    match sh {
                        Shape::Mini | Shape::Dock => {
                            let fs = fit(c, &code, c.h * 0.36, 9.0, 700, Font::Display, c.w - 8.0);
                            c.text(&code, c.w / 2.0, (c.h - fs * 1.3) / 2.0, fs, 700, c.text1(), Align::Center, c.w, Font::Display);
                        }
                        Shape::Small => {
                            c.glyph(G_KEYBOARD, 30.0, 30.0, 20.0, c.text2());
                            c.num(&code, 14.0, 52.0, 28.0, 700, c.text1());
                            c.tw(&f.caption, 14.0, 92.0, 10.5, 400, c.text2(), c.w - 28.0);
                        }
                        _ => {
                            c.glyph(G_KEYBOARD, 34.0, c.h / 2.0, 22.0, c.text2());
                            c.num(&code, 64.0, c.h / 2.0 - 30.0, 28.0, 700, c.text1());
                            c.tw(&f.caption, 64.0, c.h / 2.0 + 10.0, 12.0, 400, c.text2(), c.w - 80.0);
                        }
                    }
                }
                _ => {
                    let t = f.text.clone().unwrap_or_else(|| headline(&f));
                    if compactish(sh) {
                        compact_sum(c, &Sum::new(Vis::Glyph(G_CLOCK, c.text2()), t, "Uptime"), sh);
                        return;
                    }
                    c.glyph(G_CLOCK, 34.0, c.h / 2.0, 22.0, c.text2());
                    c.num(&t, 64.0, c.h / 2.0 - 28.0, 26.0, 600, c.text1());
                    c.tw("Since Windows started", 64.0, c.h / 2.0 + 10.0, 12.0, 400, c.text2(), c.w - 80.0);
                }
            }
        }
        SysKind::Brightness => {
            let b = x.hub.brightness;
            if matches!(sh, Shape::Mini | Shape::Dock) || (sh == Shape::Small && b.is_none()) {
                let mut s = Sum::new(Vis::Glyph(G_SUN, c.text1()), b.map(|v| format!("{:.0}%", v * 100.0)).unwrap_or_else(|| "—".into()), "Brightness");
                s.meter = b.map(|v| (v, YELLOW));
                compact_sum(c, &s, sh);
                out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("ms-settings:display".into())));
                return;
            }
            c.glyph(G_SUN, 26.0, 26.0, 16.0, c.text1());
            c.tw("Brightness", 46.0, 17.0, 12.5, 600, c.text1(), c.w - 100.0);
            match b {
                Some(v) => {
                    c.tr(&format!("{:.0}%", v * 100.0), c.w - 16.0, 17.0, 12.0, 500, c.text2());
                    let (bx, bw, bh) = (16.0, c.w - 32.0, 24.0);
                    let by = if sh == Shape::Large { c.h / 2.0 - bh / 2.0 } else { c.h - bh - 16.0 };
                    c.rr(bx, by, bw, bh, bh / 2.0, c.ink(0.12));
                    c.rr(bx, by, (bw * v).max(bh), bh, bh / 2.0, (1.0, 1.0, 1.0, 0.95));
                    c.glyph(G_SUN, bx + 14.0, by + bh / 2.0, 11.0, (0.1, 0.1, 0.12, 0.85));
                    out.hits.push(((bx, by - 4.0, bw, bh + 8.0), WAct::Brightness));
                }
                None => {
                    c.text("This monitor doesn't report its brightness (DDC/CI). Click for Display settings.", 16.0, 48.0, 11.0, 400, c.text3(), Align::Left, c.w - 32.0, Font::Text);
                    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("ms-settings:display".into())));
                }
            }
        }
    }
}

// ------------------------------------------------------------------ tools

/// Where a tile's plate sits (centre and size), to draw on top of it.
fn plate_center(c: &Canvas, dense: bool) -> (f32, f32, f32) {
    if dense {
        let p = (c.h * 0.62).round();
        if c.w > c.h * 1.5 {
            (c.h / 2.0, c.h / 2.0, p)
        } else {
            (c.w / 2.0, c.h / 2.0, p)
        }
    } else if c.w >= 180.0 {
        (18.0 + 24.0, c.h / 2.0, 48.0)
    } else {
        let p = 52.0;
        (c.w / 2.0, (c.h - p - 26.0) / 2.0 + p / 2.0, p)
    }
}

/// A control tile: a rounded-square plate with an icon (filled with `col` when on), and
/// its name. Wide cards put the name beside the plate; small ones under it; the dock
/// shows the plate alone (with the name when it's wider).
#[allow(clippy::too_many_arguments)]
fn tile(c: &Canvas, out: &mut Painted, g: &str, label: &str, on: bool, col: Col, act: WAct, dense: bool) {
    tile_sub(c, out, g, label, "", on, col, act, dense);
}

#[allow(clippy::too_many_arguments)]
fn tile_sub(c: &Canvas, out: &mut Painted, g: &str, label: &str, sub: &str, on: bool, col: Col, act: WAct, dense: bool) {
    out.hits.push(((0.0, 0.0, c.w, c.h), act));
    let plate_fill = if on { col } else { c.ink(0.12) };
    let ink = if on { WHITE } else { c.text1() };
    let (cx, cy, p) = plate_center(c, dense);
    plate(c, g, cx - p / 2.0, cy - p / 2.0, p, plate_fill, ink);
    if dense {
        if c.w > c.h * 1.5 && !label.is_empty() {
            let tx = c.h * 0.96;
            let fs = (c.h * 0.24).max(10.5);
            let two = !sub.is_empty();
            c.tw(label, tx, c.h / 2.0 - if two { fs * 1.25 } else { fs * 0.68 }, fs, 600, c.text1(), c.w - tx - 6.0);
            if two {
                c.tw(sub, tx, c.h / 2.0 + 1.0, fs * 0.82, 400, c.text2(), c.w - tx - 6.0);
            }
        }
        return;
    }
    if c.w >= 180.0 {
        let tx = cx + p / 2.0 + 14.0;
        let sub = if sub.is_empty() && on { "On" } else { sub };
        let ty = if sub.is_empty() { c.h / 2.0 - 10.0 } else { c.h / 2.0 - 19.0 };
        c.tw(label, tx, ty, 14.0, 600, c.text1(), c.w - tx - 14.0);
        if !sub.is_empty() {
            c.tw(sub, tx, ty + 21.0, 11.5, 400, c.text2(), c.w - tx - 14.0);
        }
        return;
    }
    c.tc(label, c.w / 2.0, cy + p / 2.0 + 9.0, 11.5, 600, c.text2(), c.w - 12.0);
}

/// The icon plate used by tiles.
fn plate(c: &Canvas, g: &str, x: f32, y: f32, p: f32, fill: Col, ink: Col) {
    c.rr(x, y, p, p, p * 0.3, fill);
    c.glyph(g, x + p / 2.0, y + p / 2.0, p * 0.42, ink);
}

fn tool(c: &Canvas, out: &mut Painted, k: ToolKind, x: &Ctx, sh: Shape) {
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    match k {
        ToolKind::Tray => tray(c, out, x, sh),
        ToolKind::Connectivity => connectivity(c, out, x, sh),
        ToolKind::ControlTiles => control_tiles(c, out, x, sh),
        ToolKind::Toggles => toggles(c, out, x, sh),
        ToolKind::WinSettings => tile(c, out, G_SETTINGS, "Windows Settings", false, BLUE, WAct::Open("ms-settings:".into()), dense),
        ToolKind::WinUpdate => win_update(c, out, x, sh),
        ToolKind::DarkMode => tile_sub(c, out, G_MOON, "Dark mode", if x.control.dark { "On" } else { "Off" }, x.control.dark, hex(0x5E5CE6), WAct::Dark, dense),
        ToolKind::Focus => tile(c, out, G_DND, "Focus", false, hex(0x5E5CE6), WAct::Open("ms-settings:quiethours".into()), dense),
        ToolKind::Notifications => tile(c, out, G_BELL, "Notifications", false, BLUE, WAct::Open("ms-actioncenter:".into()), dense),
        ToolKind::ShowDesktop => tile(c, out, G_DESKTOP, "Show desktop", x.hub.desktop_shown, BLUE, WAct::ShowDesktop, dense),
        ToolKind::KeepAwake => tile_sub(c, out, G_COFFEE, "Keep awake", if x.hub.keep_awake { "PC stays awake" } else { "Off" }, x.hub.keep_awake, ORANGE, WAct::KeepAwake, dense),
        ToolKind::Search if !matches!(sh, Shape::Medium | Shape::Large) => tile(c, out, G_SEARCH, "Search", false, BLUE, WAct::Search, dense),
        ToolKind::Search => {
            out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Search));
            c.rr(16.0, c.h / 2.0 - 20.0, c.w - 32.0, 40.0, 20.0, c.ink(0.10));
            c.glyph(G_SEARCH, 38.0, c.h / 2.0, 14.0, c.text2());
            c.tw("Search apps, files and settings", 58.0, c.h / 2.0 - 9.0, 13.0, 400, c.text3(), c.w - 80.0);
        }
        ToolKind::Run => tile(c, out, G_RUN, "Run", false, BLUE, WAct::Open("shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}".into()), dense),
        ToolKind::Trash => tile(c, out, G_TRASH, "Recycle Bin", false, BLUE, WAct::Open("shell:RecycleBinFolder".into()), dense),
        ToolKind::WhatsApp => {
            let att = x.whatsapp == Some(true);
            tile_sub(c, out, G_CHAT, "WhatsApp", if att { "New messages" } else if x.whatsapp.is_some() { "Open" } else { "Not running" }, true, hex(0x25D366), WAct::WhatsApp, dense);
            if att {
                let (cx, cy, p) = plate_center(c, dense);
                c.circle(cx + p * 0.42, cy - p * 0.42, (p * 0.13).max(4.0), RED);
            }
        }
        ToolKind::Screenshot => {
            let items = [("\u{F407}", "Region", "ms-screenclip:?clippingMode=Rectangle", BLUE), ("\u{E737}", "Window", "ms-screenclip:?clippingMode=Window", GREEN), ("\u{E740}", "Screen", "ms-screenclip:?clippingMode=Fullscreen", PURPLE)];
            let n = if sh == Shape::Mini { 1 } else { 3 };
            let slot = c.w / n as f32;
            for (i, (g, l, uri, col)) in items.into_iter().take(n).enumerate() {
                let cx = slot * (i as f32 + 0.5);
                let s = if dense { (c.h * 0.62).min(slot - 6.0) } else { (slot - 14.0).min(42.0) };
                let cy = if dense { c.h / 2.0 } else { c.h / 2.0 - 8.0 };
                c.rr(cx - s / 2.0, cy - s / 2.0, s, s, s * 0.28, alpha(col, 0.22));
                c.glyph(g, cx, cy, s * 0.42, col);
                if !dense {
                    c.tc(l, cx, cy + s / 2.0 + 6.0, 10.5, 500, c.text2(), slot);
                }
                out.hits.push(((cx - slot / 2.0, 0.0, slot, c.h), WAct::Open(uri.into())));
            }
        }
        ToolKind::Apps => {
            let max = match sh {
                Shape::Mini => 1,
                Shape::Dock => ((c.w - 8.0) / (c.h - 8.0)).floor().max(1.0) as usize,
                Shape::Small => 4,
                Shape::Medium => 6,
                Shape::Large => 12,
            };
            let ids: Vec<&String> = x.pinned.iter().take(max).collect();
            if ids.is_empty() {
                c.text("Pin apps in Launchpad to see them here", 14.0, c.h / 2.0 - 8.0, 11.5, 400, c.text3(), Align::Left, c.w - 28.0, Font::Text);
                return;
            }
            let cols = match sh {
                Shape::Small => 2,
                Shape::Large => 4,
                _ => ids.len(),
            };
            let rows = ids.len().div_ceil(cols);
            let labels = matches!(sh, Shape::Medium | Shape::Large);
            let cw = (c.w - 12.0) / cols as f32;
            let ch = (c.h - 8.0) / rows as f32;
            let s = if dense { c.h - 12.0 } else { (cw - 14.0).min(ch - if labels { 22.0 } else { 10.0 }).min(46.0) };
            for (i, id) in ids.iter().enumerate() {
                let (col, row) = (i % cols, i / cols);
                let cx = 6.0 + cw * (col as f32 + 0.5);
                let cy = 4.0 + ch * row as f32 + (ch - s - if labels { 16.0 } else { 0.0 }) / 2.0;
                out.icons.push((cx - s / 2.0, cy, s, (*id).clone()));
                out.hits.push(((cx - cw / 2.0, 4.0 + ch * row as f32, cw, ch), WAct::App((*id).clone())));
                if labels {
                    let name = x.names.get(*id).cloned().unwrap_or_default();
                    c.tc(&name, cx, cy + s + 4.0, 9.5, 400, c.text2(), cw - 2.0);
                }
            }
        }
        ToolKind::Notes => {
            out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Notes));
            let lines: Vec<&str> = x.hub.notes.lines().filter(|l| !l.trim().is_empty()).collect();
            if dense {
                let s = Sum::new(Vis::Glyph(G_NOTE, YELLOW), if sh == Shape::Mini { String::new() } else { lines.first().copied().unwrap_or("Notes").to_string() }, if lines.is_empty() { "Click to write".to_string() } else { format!("{} lines", lines.len()) });
                compact_sum(c, &s, sh);
                return;
            }
            c.rr(14.0, 14.0, 28.0, 28.0, 8.0, YELLOW);
            c.glyph(G_NOTE, 28.0, 28.0, 13.0, (0.2, 0.2, 0.2, 1.0));
            let (tx, top) = if sh == Shape::Small { (14.0, 52.0) } else { (52.0, 14.0) };
            let n = (((c.h - top - 8.0) / 17.0).floor() as usize).max(1);
            if lines.is_empty() {
                c.tw("Notes", tx, top, 13.0, 600, c.text1(), c.w - tx - 14.0);
                c.text("Click to write a note.", tx, top + 20.0, 11.0, 400, c.text2(), Align::Left, c.w - tx - 14.0, Font::Text);
            } else {
                c.tw(lines[0], tx, top, 13.0, 600, c.text1(), c.w - tx - 14.0);
                for (i, l) in lines.iter().skip(1).take(n.saturating_sub(1)).enumerate() {
                    c.tw(l, tx, top + 20.0 + i as f32 * 17.0, 11.0, 400, c.text2(), c.w - tx - 14.0);
                }
            }
        }
        ToolKind::ScreenTime => {
            let total = x.hub.screen.total();
            let mut apps: Vec<(&String, &u64)> = x.hub.screen.apps.iter().collect();
            apps.sort_by(|a, b| b.1.cmp(a.1));
            if dense {
                compact_sum(c, &Sum::new(Vis::Glyph(G_SCREEN, BLUE), dur(total), apps.first().map(|a| a.0.as_str()).unwrap_or("Screen time")), sh);
                return;
            }
            c.tw("Screen Time", 14.0, 12.0, 11.5, 600, c.text2(), 120.0);
            c.num(&dur(total), 14.0, 28.0, 26.0, 600, c.text1());
            let cols = [BLUE, PURPLE, ORANGE, GREEN, YELLOW, RED];
            let (x0, y0, n) = match sh {
                Shape::Small => (14.0, 66.0, 2),
                Shape::Large => (14.0, 76.0, 5),
                _ => (c.w * 0.45, 16.0, 3),
            };
            for (i, (name, secs)) in apps.iter().take(n).enumerate() {
                let y = y0 + i as f32 * 24.0;
                c.tw(name, x0, y, 11.0, 500, c.text1(), c.w - x0 - 60.0);
                c.tr(&dur(**secs), c.w - 14.0, y, 11.0, 400, c.text2());
                c.bar(x0, y + 16.0, c.w - x0 - 14.0, 3.0, if total > 0 { **secs as f32 / total as f32 } else { 0.0 }, cols[i % cols.len()]);
            }
            if apps.is_empty() && sh != Shape::Small {
                c.tw("Counting from now", 14.0, 66.0, 11.0, 400, c.text3(), c.w);
            }
        }
        ToolKind::Timeline => {
            let hours = &x.hub.screen.hours;
            let vals: Vec<f64> = (0..24).map(|h| *hours.get(h).unwrap_or(&0) as f64).collect();
            if sh == Shape::Mini {
                compact_sum(c, &Sum::new(Vis::Glyph(G_CLOCK, BLUE), dur(x.hub.screen.total()), ""), sh);
                return;
            }
            let (top, bot) = if dense { (6.0, 6.0) } else { (32.0, 22.0) };
            if !dense {
                // The total first; the title takes what's left (just "Today" on small cards).
                let total = dur(x.hub.screen.total());
                let vw = c.measure(&total, 11.5, 500, Font::Text).0;
                c.tr(&total, c.w - 14.0, 10.0, 11.5, 500, c.text1());
                let title = if sh == Shape::Small { "Today" } else { "Day Timeline" };
                c.tw(title, 14.0, 10.0, 11.5, 600, c.text2(), c.w - 28.0 - vw - 8.0);
            }
            let w = c.w - 28.0;
            let slot = w / 24.0;
            let hi = vals.iter().cloned().fold(60.0, f64::max);
            for (h, v) in vals.iter().enumerate() {
                let ht = ((v / hi) as f32 * (c.h - top - bot)).max(2.0);
                let col = if h as u32 == x.now.hour { BLUE } else if *v > 0.0 { alpha(BLUE, 0.55) } else { c.ink(0.1) };
                c.rr(14.0 + slot * h as f32 + slot * 0.15, c.h - bot - ht, slot * 0.7, ht, 1.5, col);
            }
            if !dense {
                let marks: &[(usize, &str)] = if sh == Shape::Small { &[(0, "12a"), (12, "12p")] } else { &[(0, "12a"), (6, "6a"), (12, "12p"), (18, "6p")] };
                for (h, l) in marks {
                    c.tw(l, 14.0 + slot * *h as f32, c.h - 18.0, 9.5, 400, c.text3(), 40.0);
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Control Center

fn round_toggle(c: &Canvas, cx: f32, cy: f32, r: f32, g: &str, on: bool) {
    c.circle(cx, cy, r, if on { BLUE } else { c.ink(0.12) });
    c.glyph(g, cx, cy, r * 0.8, if on { WHITE } else { c.text1() });
}

/// Wi-Fi and Bluetooth: the round button switches the radio; the name opens its list.
fn connectivity(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let ctl = x.control;
    let rows = [
        (G_WIFI, "Wi-Fi", ctl.wifi, WAct::Wifi, WAct::WifiMenu),
        (G_BT, "Bluetooth", ctl.bluetooth, WAct::Bluetooth, WAct::Open("ms-settings:bluetooth".into())),
    ];
    if matches!(sh, Shape::Mini | Shape::Dock) {
        let slot = c.w / 2.0;
        for (i, (g, _, on, act, _)) in rows.into_iter().enumerate() {
            let cx = slot * (i as f32 + 0.5);
            round_toggle(c, cx, c.h / 2.0, (c.h * 0.32).min(slot * 0.42), g, on == Some(true));
            out.hits.push(((slot * i as f32, 0.0, slot, c.h), act));
        }
        return;
    }
    let rh = (c.h - 16.0) / rows.len() as f32;
    let r = if sh == Shape::Large { 22.0 } else { 18.0 };
    for (i, (g, name, on, act, more)) in rows.into_iter().enumerate() {
        let cy = 8.0 + rh * (i as f32 + 0.5);
        let cx = 14.0 + r;
        round_toggle(c, cx, cy, r, g, on == Some(true));
        let tx = cx + r + 10.0;
        let state = match on {
            Some(true) => "On",
            Some(false) => "Off",
            None => "Unavailable",
        };
        c.tw(name, tx, cy - 17.0, 12.5, 600, c.text1(), c.w - tx - 22.0);
        c.tw(state, tx, cy + 1.0, 11.0, 400, c.text2(), c.w - tx - 22.0);
        c.glyph(G_CHEV, c.w - 14.0, cy, 9.0, c.text3());
        out.hits.push(((tx - 4.0, cy - rh / 2.0, c.w - tx + 4.0, rh), more));
        if on.is_some() {
            out.hits.push(((cx - r - 4.0, cy - r - 4.0, 2.0 * r + 8.0, 2.0 * r + 8.0), act));
        }
    }
}

/// A grid of control buttons, 2 or 4 across.
fn button_grid(c: &Canvas, out: &mut Painted, items: Vec<(&'static str, &'static str, bool, WAct)>, sh: Shape) {
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    if sh == Shape::Mini {
        // Just the first one.
        if let Some((g, _, on, act)) = items.into_iter().next() {
            tile(c, out, g, "", on, BLUE, act, true);
        }
        return;
    }
    // Small and Large cards: two columns (Large would otherwise stretch four tiles into
    // tall bars); Medium and the dock: one row.
    let cols = if matches!(sh, Shape::Small | Shape::Large) { 2 } else { items.len().min(4) };
    let rows = items.len().div_ceil(cols);
    let pad = if dense { 4.0 } else { 10.0 };
    let gap = if dense { 4.0 } else { 8.0 };
    let tw = (c.w - 2.0 * pad - gap * (cols - 1) as f32) / cols as f32;
    // Never taller than a little over square; the grid is centred in what's left.
    let th = ((c.h - 2.0 * pad - gap * (rows - 1) as f32) / rows as f32).min(tw * 1.15);
    let top = (c.h - th * rows as f32 - gap * (rows - 1) as f32) / 2.0;
    for (i, (g, label, on, act)) in items.into_iter().enumerate() {
        let (tx, ty) = (pad + (i % cols) as f32 * (tw + gap), top + (i / cols) as f32 * (th + gap));
        c.rr(tx, ty, tw, th, (th.min(tw) * 0.28).min(16.0), if on { BLUE } else { c.ink(0.08) });
        let ink = if on { WHITE } else { c.text1() };
        if dense || th < 50.0 {
            c.glyph(g, tx + tw / 2.0, ty + th / 2.0, (th.min(tw) * 0.42).min(18.0), ink);
        } else {
            c.glyph(g, tx + tw / 2.0, ty + th * 0.4, 18.0, ink);
            // A narrow tile gets the first word ("Dark" for "Dark mode") rather than edge-to-edge text.
            let label = if c.measure(label, 10.5, 500, Font::Text).0 > tw - 12.0 { label.split_whitespace().next().unwrap_or(label) } else { label };
            c.tc(label, tx + tw / 2.0, ty + th * 0.4 + 16.0, 10.5, 500, if on { (1.0, 1.0, 1.0, 0.9) } else { c.text2() }, tw);
        }
        out.hits.push(((tx, ty, tw, th), act));
    }
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
    button_grid(c, out, items, sh);
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
    button_grid(c, out, items, sh);
}

/// Background Apps: the tray icons in a grid (the host draws the pictures).
fn tray(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let n = x.tray.len();
    if matches!(sh, Shape::Mini | Shape::Dock) {
        let s = (c.h * 0.42).round();
        let slot = s + 6.0;
        let fit_n = (((c.w - 8.0) / slot).floor() as usize).max(1).min(n);
        for (i, (key, _)) in x.tray.iter().take(fit_n).enumerate() {
            let ix = (c.w - fit_n as f32 * slot) / 2.0 + i as f32 * slot + 3.0;
            out.icons.push((ix, (c.h - s) / 2.0, s, format!("tray:{key}")));
            out.hits.push(((ix - 3.0, 0.0, slot, c.h), WAct::Tray(*key)));
        }
        if n == 0 {
            c.glyph(G_UP, c.w / 2.0, c.h / 2.0, c.h * 0.3, c.text2());
        }
        return;
    }
    let pad = 14.0;
    c.tw("Background Apps", pad, 12.0, 12.5, 600, c.text1(), c.w - 60.0);
    c.tr(&n.to_string(), c.w - pad, 13.0, 11.5, 400, c.text3());
    let head = 36.0;
    let foot = if sh == Shape::Small { 6.0 } else { 24.0 };
    let cols = ((c.w - 2.0 * pad) / 40.0).floor().max(1.0) as usize;
    let tile_w = (c.w - 2.0 * pad) / cols as f32;
    let rows = (((c.h - head - foot) / tile_w).floor() as usize).max(1);
    let isz = (tile_w * 0.5).min(22.0);
    for (i, (key, _)) in x.tray.iter().take(cols * rows).enumerate() {
        let (tx, ty) = (pad + (i % cols) as f32 * tile_w, head + (i / cols) as f32 * tile_w);
        if x.hover_tray == Some(*key) {
            c.rr(tx + 2.0, ty + 2.0, tile_w - 4.0, tile_w - 4.0, 10.0, c.ink(0.10));
        }
        out.icons.push((tx + (tile_w - isz) / 2.0, ty + (tile_w - isz) / 2.0, isz, format!("tray:{key}")));
        out.hits.push(((tx, ty, tile_w, tile_w), WAct::Tray(*key)));
    }
    if sh != Shape::Small {
        let hint = match x.hover_tray.and_then(|k| x.tray.iter().find(|t| t.0 == k)) {
            Some((_, tip)) if !tip.is_empty() => tip.lines().next().unwrap_or("").to_string(),
            _ if n == 0 => "No background apps".to_string(),
            _ => "Click to open · right-click for the app’s menu".to_string(),
        };
        c.tw(&hint, pad, c.h - foot + 2.0, 11.0, 400, c.text3(), c.w - 2.0 * pad);
    }
}

fn win_update(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    use laminar_sys::update::UpdateStatus as U;
    let (sub, restart, attention) = match x.update {
        None => ("Checking…".to_string(), false, false),
        Some(U::RestartRequired) => ("Restart to finish installing updates".to_string(), true, true),
        Some(U::Pending(1)) => ("1 update waiting".to_string(), false, true),
        Some(U::Pending(n)) => (format!("{n} updates waiting"), false, true),
        Some(U::UpToDate) => ("You’re up to date".to_string(), false, false),
        Some(U::Unknown) => ("Status unavailable".to_string(), false, false),
    };
    let open = WAct::Open("ms-settings:windowsupdate".into());
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    if dense || sh == Shape::Small {
        tile_sub(c, out, G_UPDATE, if restart { "Restart" } else { "Updates" }, &sub, attention, BLUE, if restart { WAct::Power } else { open }, dense);
        if attention {
            let (cx, cy, p) = plate_center(c, dense);
            c.circle(cx + p * 0.42, cy - p * 0.42, (p * 0.12).max(4.0), RED);
        }
        return;
    }
    out.hits.push(((0.0, 0.0, c.w, c.h), open));
    let p = 48.0;
    plate(c, G_UPDATE, 18.0, (c.h - p) / 2.0, p, if attention { BLUE } else { c.ink(0.12) }, if attention { WHITE } else { c.text1() });
    let tx = 18.0 + p + 14.0;
    let bw = if restart { 78.0 } else { 0.0 };
    c.tw("Windows Update", tx, c.h / 2.0 - 19.0, 14.0, 600, c.text1(), c.w - tx - bw - 20.0);
    c.tw(&sub, tx, c.h / 2.0 + 2.0, 11.5, 400, c.text2(), c.w - tx - bw - 20.0);
    if restart {
        let (bx, bh) = (c.w - bw - 14.0, 28.0);
        let by = (c.h - bh) / 2.0;
        c.rr(bx, by, bw, bh, bh / 2.0, BLUE);
        c.tc("Restart", bx + bw / 2.0, by + 6.0, 11.5, 600, WHITE, bw);
        out.hits.push(((bx, by, bw, bh), WAct::Power));
    }
}

// ------------------------------------------------------------------ progress

fn progress(c: &Canvas, s: Span, x: &Ctx, sh: Shape) {
    let n = x.now;
    let (total, done, label, frac) = match s {
        Span::Day => {
            let m = n.hour * 60 + n.minute;
            (24u32, n.hour, "Today", m as f32 / 1440.0)
        }
        Span::Month => (n.days_in_month(), n.day - 1, MONTHS[n.month as usize - 1], (n.day - 1) as f32 / n.days_in_month() as f32 + (n.hour as f32 / 24.0) / n.days_in_month() as f32),
        Span::Year => {
            let d = n.day_of_year();
            (52, (d - 1) / 7, "This year", (d - 1) as f32 / n.days_in_year() as f32)
        }
    };
    let pct = format!("{:.0}%", frac * 100.0);
    let col = match s {
        Span::Day => BLUE,
        Span::Month => GREEN,
        Span::Year => ORANGE,
    };
    if sh == Shape::Mini {
        compact_sum(c, &Sum::new(Vis::Ring(frac, col, pct), "", ""), sh);
        return;
    }
    let dense = sh == Shape::Dock;
    let (gx, gy, gw, gh) = match sh {
        Shape::Dock => (8.0, 8.0, c.w * 0.62, c.h - 16.0),
        Shape::Small => (12.0, 12.0, c.w - 24.0, c.h - 46.0),
        _ => (16.0, 14.0, c.w - 32.0, c.h - 44.0),
    };
    let cols = match (s, sh) {
        (Span::Day, Shape::Small) => 6,
        (Span::Day, _) => 12,
        (Span::Month, Shape::Small) => 7,
        (Span::Month, _) => 11,
        (Span::Year, Shape::Small) => 8,
        (Span::Year, _) => 13,
    };
    let rows = total.div_ceil(cols);
    let cell = (gw / cols as f32).min(gh / rows as f32);
    let r = cell * 0.32;
    let ox = gx + (gw - cell * cols as f32) / 2.0;
    for i in 0..total {
        let (cx, cy) = (ox + cell * ((i % cols) as f32 + 0.5), gy + cell * ((i / cols) as f32 + 0.5));
        c.circle(cx, cy, r, if i < done { col } else if i == done { alpha(col, 0.5) } else { c.ink(0.14) });
    }
    if dense {
        c.text(&pct, c.w - 8.0, c.h / 2.0 - c.h * 0.2, c.h * 0.3, 600, c.text1(), Align::Right, c.w * 0.36, Font::Display);
        return;
    }
    c.tw(label, gx, c.h - 24.0, 11.5, 500, c.text2(), c.w * 0.6);
    c.tr(&pct, c.w - gx, c.h - 25.0, 12.5, 600, c.text1());
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
