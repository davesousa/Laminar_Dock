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

struct Sum {
    vis: Vis,
    value: String,
    caption: String,
    delta: Option<f64>,
    series: Vec<f64>,
    meter: Option<(f32, Col)>,
}

impl Sum {
    fn new(vis: Vis, value: impl Into<String>, caption: impl Into<String>) -> Self {
        Sum { vis, value: value.into(), caption: caption.into(), delta: None, series: Vec::new(), meter: None }
    }
    /// The trend's colour: up/down for a change, else the meter's (identity) colour, else blue.
    fn spark_col(&self) -> Col {
        match (self.delta, self.meter) {
            (Some(d), _) => up_down(d),
            (None, Some((_, col))) => col,
            _ => BLUE,
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
            if s.value.is_empty() {
                let v = style::BADGE_MINI_VIS * k;
                draw_vis(c, &s.vis, (c.w - v) / 2.0, (c.h - v) / 2.0, v);
                return;
            }
            // Value-led: a small mark over one compact value.
            let v = style::BADGE_MINI * k;
            draw_vis(c, &s.vis, (c.w - v) / 2.0, 8.0 * k, v);
            let role = Type { size: style::MINI_VALUE.size * k, ..style::MINI_VALUE };
            let (val, fs) = fit_value(c, &s.value, role, c.w - 2.0 * style::M_MINI * k);
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
            let (val, fs) = fit_value(c, &s.value, role, right - x0);
            // With a meter under the caption the three lines move up as one block, so the
            // caption keeps clear of the value (baseline 40 under a value at 30 touched it).
            let (val_base, cap_base, bar_y) = if col_meter.is_some() { (23.0, 36.0, 41.0) } else { (30.0, 44.0, 45.0) };
            let (val_base, cap_base, bar_y) = (val_base * k, cap_base * k, bar_y * k);
            c.text_base(&val, x0, val_base, fs, role.weight, vcol, Align::Left, c.w * 2.0);
            let cs = style::FOOT.size * k;
            // The change shares the caption line only when both fit whole; otherwise the
            // caption alone (Dock L shows the trend in its slot instead).
            let cap = short_caption(&s.caption);
            let both = s.delta.filter(|_| slot != Slot::Delta).map(|d| fmt_delta(c, d)).filter(|(dt, _)| {
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
                Slot::Spark => c.area(slot_x, sy0, slot_w, sy1 - sy0, &s.series, s.spark_col(), true),
                Slot::Meter => {
                    if let Some((f, col)) = s.meter {
                        let pct = format!("{:.0}%", (f * 100.0).clamp(0.0, 999.0));
                        c.text_base(&pct, c.w - m, 23.0 * k, cs, 500, c.text2(), Align::Right, slot_w);
                        c.bar(slot_x, 29.0 * k, slot_w, style::METER_H_COMPACT * k, f, col);
                    }
                }
                Slot::Delta => {
                    if let Some(d) = s.delta {
                        let (dt, dcol) = fmt_delta(c, d);
                        c.text_base(&dt, c.w - m, 32.0 * k, style::TITLE.size * k, 600, dcol, Align::Right, slot_w);
                    }
                }
                Slot::None => {}
            }
        }
        _ => {
            // Small card, bottom-anchored: mark and accessory on top, value and caption below.
            let m = style::M_SMALL;
            let bs = style::BADGE_SMALL;
            draw_vis(c, &s.vis, m, m, bs);
            let (sx0, sx1) = (m + bs + style::SP[4], c.w - m);
            let mut sy0 = 16.0;
            if let Some(d) = s.delta {
                let (t, col) = fmt_delta(c, d);
                c.text_base(&t, sx1, 26.0, style::FOOT.size, 600, col, Align::Right, sx1 - sx0);
                sy0 = 30.0;
            }
            if s.series.len() > 2 {
                c.area(sx0, sy0, sx1 - sx0, 42.0 - sy0, &s.series, s.spark_col(), true);
            } else if let Some((f, col)) = s.meter {
                c.bar(sx0, if s.delta.is_some() { 36.0 } else { 27.0 }, sx1 - sx0, style::METER_H_COMPACT, f, col);
            }
            let (v, fs) = fit_value(c, &s.value, style::HERO_S, c.w - 2.0 * m);
            c.text_base(&v, m, 86.0, fs, style::HERO_S.weight, vcol, Align::Left, c.w * 2.0);
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
        c.chart(x, y, w, h, &f.series, col, ChartOpts { fill: true, dot: true, stroke: style::STROKE, smooth: f.series.len() >= 8 });
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

/// The whole-card fallback for connector faces that have no data (§7): a skeleton while
/// loading, the Connect layout when it needs setting up, the reason when it failed.
fn placeholder(c: &Canvas, out: &mut Painted, name: &str, badge: &Badge, conn: &'static str, g: &Got, sh: Shape) -> bool {
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
    match sh {
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
                skeleton(c, x0, (c.h - 10.0 * k) / 2.0, (60.0 * k).min(tw), 10.0 * k);
            } else {
                c.text_base(name, x0, 30.0 * k, style::TITLE.size * k, 600, c.text1(), Align::Left, tw);
                if connect {
                    c.text_base("Connect", x0, 44.0 * k, style::FOOT.size * k, 600, BLUE, Align::Left, tw);
                } else {
                    reason_line(c, &reason, x0, 44.0 * k, style::FOOT.size * k, err, Align::Left, tw);
                }
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            c.badge(badge, m, m, style::BADGE_SMALL);
            let inner = c.w - 2.0 * m;
            if loading {
                let ch = c.cap_h(style::HERO_S.size);
                skeleton(c, m, 86.0 - ch, inner * 0.6, ch);
                skeleton(c, m, 106.0 - 8.0, 40.0, 8.0);
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
            let bs = style::BADGE_GLANCE;
            c.badge(badge, c.w - m - bs, m, bs);
            skeleton(c, m, m, (c.w - 2.0 * m - bs - style::SP[3]) * 0.6, c.cap_h(style::HERO.size));
            skeleton(c, m, 58.0 - 8.0, 40.0, 8.0);
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let (x, y, w, _) = detail_region(c);
                for (i, f) in [1.0, 0.75, 0.5].iter().enumerate() {
                    skeleton(c, x, y + 4.0 + i as f32 * style::ROW_L, w * f, 8.0);
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
            let bs = 48.0;
            c.badge(badge, (c.w - bs) / 2.0, 36.0, bs);
            c.text_base(name, c.w / 2.0, 108.0, 15.0, 600, c.text1(), Align::Center, c.w - 2.0 * style::M_CARD);
            reason_line(c, &reason, c.w / 2.0, 128.0, style::CAPTION.size, err, Align::Center, c.w - 2.0 * style::M_CARD);
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

#[allow(clippy::too_many_arguments)]
fn stat(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, open: &str, sh: Shape) {
    let g = got(x, conn);
    if placeholder(c, out, def.name, badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, open);
    let (value, caption) = if def.id == "github-streak" {
        (f.extra.get("streak").cloned().unwrap_or_else(|| "0".into()), "Day streak".to_string())
    } else {
        (headline(&f), if f.caption.is_empty() { def.name.to_string() } else { f.caption.clone() })
    };
    if compactish(sh) {
        let mut s = Sum::new(Vis::Badge(*badge), value, caption);
        s.delta = f.delta_pct;
        s.series = f.series.clone();
        compact_sum(c, &s, sh);
        return;
    }
    c.badge(badge, c.w - 16.0 - 34.0, 16.0, 34.0);
    c.num(&value, 16.0, 14.0, fit(c, &value, 32.0, 18.0, 600, Font::Display, c.w - 80.0), 600, c.text1());
    // Subscribers share the caption's line ("MRR · 812 subscribers"), not a floating corner.
    let caption = match f.extra.get("active_subscriptions") {
        Some(n) => format!("{caption} · {n} subscribers"),
        None => caption,
    };
    let cw = c.tw(&caption, 16.0, 58.0, 12.0, 400, c.text2(), c.w - 120.0);
    if let Some(d) = f.delta_pct {
        c.tw(&fmt_delta_text(d), 22.0 + cw, 58.0, 11.5, 600, up_down(d), 90.0);
    }
    // Mail: the newest subject under the count (Large lists it with the others instead).
    if conn == "gmail" && sh != Shape::Large {
        if let Some(r) = f.rows.first() {
            c.tw(&r.label, 16.0, 74.0, 11.0, 400, c.text3(), c.w - 32.0);
        }
    }
    if sh == Shape::Large {
        details(c, &f, f.delta_pct.map(up_down).unwrap_or(GREEN));
    } else if f.series.len() > 2 {
        c.area(c.w - 120.0, 64.0, 104.0, 20.0, &f.series, GREEN, false);
    }
}

#[allow(clippy::too_many_arguments)]
fn chart(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, open: &str, sh: Shape) {
    let g = got(x, conn);
    if placeholder(c, out, def.name, badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, open);
    let col = match f.delta_pct {
        Some(d) if d < 0.0 => RED,
        _ => GREEN,
    };
    let title = match f.extra.get("symbol") {
        Some(s) => s.clone(),
        None => def.name.trim_end_matches(" Chart").to_string(),
    };
    let value = if f.unit == "tokens" { compact(f.value.unwrap_or(0.0)) } else { headline(&f) };
    if compactish(sh) {
        let mut s = Sum::new(Vis::Badge(*badge), value, title);
        s.delta = f.delta_pct;
        s.series = f.series.clone();
        compact_sum(c, &s, sh);
        return;
    }
    c.badge(badge, 14.0, 13.0, 20.0);
    c.tw(&title, 42.0, 14.0, 12.5, 600, c.text1(), c.w - 160.0);
    c.tr(&value, c.w - 14.0, 9.0, 16.0, 600, c.text1());
    match f.delta_pct {
        Some(d) => {
            c.tr(&fmt_delta_text(d), c.w - 14.0, 30.0, 10.5, 600, up_down(d));
        }
        None => {
            c.tr(&f.caption, c.w - 14.0, 30.0, 10.5, 400, c.text3());
        }
    }
    if f.series.len() > 1 {
        c.area(14.0, 48.0, c.w - 28.0, c.h - 60.0, &f.series, col, true);
    } else {
        c.tw("No chart data yet", 14.0, 60.0, 11.5, 400, c.text3(), c.w - 28.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn usage(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, open: &str, sh: Shape) {
    let g = got(x, conn);
    if placeholder(c, out, def.name, badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, open);
    let tokens = f.unit == "tokens";
    let short = if tokens { compact(f.value.unwrap_or(0.0)) } else { headline(&f) };
    let (meter_label, frac) = if tokens {
        let w5 = f.extra.get("tokens_5h").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        let wk = f.extra.get("tokens_week").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        // Share of the week's use in the last 5 hours (a sense of pace).
        (format!("5h window {} · week {}", compact(w5), compact(wk)), if wk > 0.0 { (w5 / wk) as f32 } else { 0.0 })
    } else {
        let lim = f.extra.get("total_credits").cloned().unwrap_or_default();
        (if lim.is_empty() { f.caption.clone() } else { format!("{}% of ${lim} used", (f.frac.unwrap_or(0.0) * 100.0).round() as i32) }, f.frac.unwrap_or(0.0))
    };
    if compactish(sh) {
        let mut s = Sum::new(Vis::Badge(*badge), short, if tokens { "tokens today".to_string() } else { f.caption.clone() });
        s.meter = Some((frac, CLAUDE));
        if sh == Shape::Small && f.series.len() > 2 {
            s.series = f.series.clone();
        }
        compact_sum(c, &s, sh);
        return;
    }
    let value = if tokens { format!("{short} tokens") } else { short };
    c.badge(badge, 16.0, 16.0, 36.0);
    c.tw(def.name, 64.0, 15.0, 12.5, 600, c.text2(), c.w - 80.0);
    c.num(&value, 64.0, 32.0, 19.0, 600, c.text1());
    c.bar(16.0, 70.0, c.w - 32.0, 6.0, frac, CLAUDE);
    c.tw(&meter_label, 16.0, 78.0, 10.0, 400, c.text3(), c.w - 32.0);
    if let Some(cost) = f.extra.get("cost_today_usd").and_then(|v| v.parse::<f64>().ok()) {
        c.tr(&format!("≈ {}", money(cost, None)), c.w - 16.0, 20.0, 11.0, 400, c.text3());
    }
    if sh == Shape::Large {
        details(c, &f, CLAUDE);
    }
}

#[allow(clippy::too_many_arguments)]
fn heat(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, color: Col, open: &str, sh: Shape) {
    let g = got(x, conn);
    if placeholder(c, out, def.name, badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    let open = match (conn, x.cfg.widget_settings.get(conn).and_then(|m| m.get("username"))) {
        ("github", Some(u)) => format!("https://github.com/{u}"),
        ("leetcode", Some(u)) => format!("https://leetcode.com/u/{u}"),
        _ => open.to_string(),
    };
    open_hit(out, c, &open);
    let streak = f.extra.get("streak").cloned();
    if sh == Shape::Mini {
        let s = Sum::new(Vis::Badge(*badge), streak.map(|s| format!("{s}d")).unwrap_or_else(|| headline(&f)), "");
        compact_sum(c, &s, sh);
        return;
    }
    let (gx, gy, gw, gh) = match sh {
        Shape::Dock => (8.0, 7.0, c.w - 16.0, c.h - 14.0),
        Shape::Small => {
            c.badge(badge, 12.0, 12.0, 18.0);
            c.tw(&headline(&f), 36.0, 12.0, 12.5, 600, c.text1(), c.w - 48.0);
            if let Some(s) = &streak {
                c.tw(&format!("{s}-day streak"), 12.0, c.h - 22.0, 10.5, 400, c.text2(), c.w - 24.0);
            }
            (12.0, 38.0, c.w - 24.0, c.h - 66.0)
        }
        _ => {
            c.badge(badge, 14.0, 12.0, 20.0);
            c.tw(def.name, 42.0, 14.0, 12.5, 600, c.text1(), c.w - 150.0);
            let st = streak.as_ref().map(|s| format!("{s}-day streak")).unwrap_or_default();
            c.tr(&format!("{}  {}", headline(&f), st), c.w - 14.0, 15.0, 11.5, 500, c.text2());
            let gh = if sh == Shape::Large { 100.0 } else { c.h - 50.0 };
            (14.0, 40.0, c.w - 28.0, gh)
        }
    };
    let cell_gap = if sh == Shape::Dock { 1.5 } else { 2.5 };
    let cell = ((gh - 6.0 * cell_gap) / 7.0).max(2.0);
    let cols = (((gw + cell_gap) / (cell + cell_gap)).floor() as usize).max(1);
    let n = f.grid.len();
    let today_row = x.now.weekday as usize;
    // The last column ends today; earlier days fill backwards.
    for col in 0..cols {
        for row in 0..7 {
            if row > today_row && col == cols - 1 {
                continue;
            }
            let back = (cols - 1 - col) * 7 + today_row;
            let idx = n as isize - 1 - (back as isize - row as isize);
            let v = if idx >= 0 && (idx as usize) < n { f.grid[idx as usize] } else { 0.0 };
            let fill = if v <= 0.0 { c.ink(0.08) } else { alpha(color, 0.3 + 0.7 * v) };
            c.rr(gx + col as f32 * (cell + cell_gap), gy + row as f32 * (cell + cell_gap), cell, cell, cell * 0.25, fill);
        }
    }
    if sh == Shape::Large {
        // Totals under the graph.
        let y = gy + gh + 14.0;
        let rows: Vec<_> = f.rows.iter().take(4).collect();
        let slot = (c.w - 28.0) / rows.len().max(1) as f32;
        for (k, r) in rows.iter().enumerate() {
            let cx = 14.0 + slot * k as f32;
            c.num(&r.value, cx, y, 18.0, 600, c.text1());
            c.tw(&r.label, cx, y + 24.0, 10.5, 400, c.text2(), slot - 6.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn list(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, open: &str, sh: Shape) {
    let g = got(x, conn);
    if placeholder(c, out, def.name, badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, open);
    let value = if conn == "calendar" { format!("{} today", f.value.unwrap_or(0.0)) } else { headline(&f) };
    if matches!(sh, Shape::Mini | Shape::Dock) {
        // A value and the label that belongs to it: the first row's own figure when it has
        // one ("42 Repositories", "12% Microsoft Edge"), not the headline beside a row name.
        let first = f.rows.first();
        let (v, cap) = match first {
            Some(r) if conn != "calendar" && conn != "recent_files" && !r.value.is_empty() && r.value.chars().count() <= 10 => (r.value.clone(), r.label.clone()),
            Some(r) => (value, r.label.clone()),
            None => (value, def.name.to_string()),
        };
        compact_sum(c, &Sum::new(Vis::Badge(*badge), v, cap), sh);
        return;
    }
    let small = sh == Shape::Small;
    let pad = if small { 12.0 } else { 14.0 };
    c.badge(badge, pad, 12.0, 22.0);
    if small {
        c.tr(&value, c.w - pad, 15.0, 11.5, 600, c.text2());
    } else {
        c.tw(def.name, 44.0, 15.0, 12.5, 600, c.text1(), c.w - 140.0);
        c.tr(&value, c.w - pad, 15.0, 12.0, 500, c.text2());
    }
    let top = 44.0;
    let rh = if c.h > 150.0 { 26.0 } else if small { 22.0 } else { 17.0 };
    let n = (((c.h - top - 6.0) / rh).floor() as usize).max(1);
    let rows: Vec<_> = f.rows.iter().take(n).collect();
    if rows.is_empty() {
        c.tw("Nothing to show", pad, top + 4.0, 11.5, 400, c.text3(), c.w - 2.0 * pad);
        return;
    }
    for (k, r) in rows.iter().enumerate() {
        let y = top + k as f32 * rh;
        let vmax = if small { (c.w - 2.0 * pad) * 0.4 } else { c.w * 0.45 };
        let vw = c.measure(&r.value, 11.0, 500, Font::Text).0.min(vmax);
        c.tw(&r.label, pad, y, 11.5, 400, c.text1(), c.w - 2.0 * pad - vw - 6.0);
        c.text(&r.value, c.w - pad, y, 11.0, 500, c.text2(), Align::Right, vmax, Font::Text);
        if let Some(fr) = r.frac.filter(|v| *v > 0.0 && rh >= 22.0) {
            c.bar(pad, y + 16.0, c.w - 2.0 * pad, 3.0, fr, if conn == "top_memory" { PURPLE } else { BLUE });
        }
        if conn == "recent_files" && !r.value.is_empty() {
            out.hits.push(((6.0, y - 3.0, c.w - 12.0, rh), WAct::File(r.value.clone())));
        }
    }
}

fn watch(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, sh: Shape) {
    let badge = super::canvas::gly("\u{E9D2}", 0x1F2A1F, 0x34C759);
    let g = got(x, conn);
    if placeholder(c, out, def.name, &badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, "https://finance.yahoo.com");
    if matches!(sh, Shape::Mini | Shape::Dock) {
        if let Some(r) = f.rows.first() {
            let mut s = Sum::new(Vis::Badge(badge), if sh == Shape::Mini { r.label.clone() } else { r.value.clone() }, r.label.clone());
            s.delta = r.delta;
            compact_sum(c, &s, sh);
        }
        return;
    }
    let small = sh == Shape::Small;
    let n = f.rows.len().min(if c.h > 150.0 { 6 } else { 4 }).max(1);
    let rh = (c.h - 16.0) / n as f32;
    for (k, r) in f.rows.iter().take(n).enumerate() {
        let y = 8.0 + k as f32 * rh + (rh - 16.0) / 2.0;
        let d = r.delta.unwrap_or(0.0);
        let pw = if small { 54.0 } else { 66.0 };
        c.tw(&r.label, 14.0, y, 12.0, 600, c.text1(), if small { c.w - pw - 30.0 } else { 80.0 });
        if !small {
            c.text(&r.value, c.w * 0.62, y, 12.5, 500, c.text1(), Align::Right, 120.0, Font::Text);
        }
        c.rr(c.w - 14.0 - pw, y - 1.0, pw, 18.0, 5.0, alpha(up_down(d), 0.9));
        c.tc(&format!("{}{:.2}%", if d >= 0.0 { "+" } else { "" }, d), c.w - 14.0 - pw / 2.0, y + 1.0, 10.5, 600, WHITE, pw);
    }
}

fn next_event(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, sh: Shape) {
    let badge = super::canvas::gly(G_CAL, 0xFF3B30, 0xFFFFFF);
    let g = got(x, "calendar");
    if placeholder(c, out, def.name, &badge, "calendar", &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    let title = f.extra.get("next_title").cloned().unwrap_or_else(|| "Nothing scheduled".into());
    let when = f.extra.get("next_time").cloned().unwrap_or_default();
    let mins = f.extra.get("next_minutes").and_then(|m| m.parse::<i64>().ok());
    let soon = match mins {
        Some(m) if m <= 0 => "Now".to_string(),
        Some(m) if m < 60 => format!("in {m} min"),
        Some(m) if m < 24 * 60 => format!("in {} h {} min", m / 60, m % 60),
        _ => String::new(),
    };
    match sh {
        Shape::Mini => {
            let short = match mins {
                Some(m) if (0..60).contains(&m) => format!("{m}m"),
                Some(m) if (0..24 * 60).contains(&m) => format!("{}h", m / 60),
                _ => "—".into(),
            };
            compact_sum(c, &Sum::new(Vis::Badge(badge), short, ""), sh);
        }
        Shape::Dock => compact_sum(c, &Sum::new(Vis::Badge(badge), title, if soon.is_empty() { when } else { soon }), sh),
        Shape::Small => {
            c.badge(&badge, 14.0, 14.0, 26.0);
            c.tw("Up next", 48.0, 20.0, 10.5, 600, RED, c.w - 60.0);
            c.text(&title, 14.0, 48.0, 13.5, 600, c.text1(), Align::Left, c.w - 28.0, Font::Text);
            c.tw(&when, 14.0, 70.0, 11.0, 400, c.text2(), c.w - 28.0);
            if !soon.is_empty() {
                c.tw(&soon, 14.0, c.h - 24.0, 11.0, 600, BLUE, c.w - 28.0);
            }
        }
        _ => {
            c.badge(&badge, 16.0, 16.0, 34.0);
            c.tw("Up next", 62.0, 16.0, 11.0, 600, RED, 100.0);
            c.tw(&title, 62.0, 32.0, 15.0, 600, c.text1(), c.w - 78.0);
            c.tw(&when, 62.0, 56.0, 11.5, 400, c.text2(), c.w - 160.0);
            if !soon.is_empty() {
                let pw = c.measure(&soon, 11.0, 600, Font::Text).0 + 16.0;
                c.rr(c.w - 14.0 - pw, 54.0, pw, 20.0, 10.0, alpha(BLUE, 0.22));
                c.tc(&soon, c.w - 14.0 - pw / 2.0, 56.0, 11.0, 600, BLUE, pw);
            }
            if sh == Shape::Large {
                let mut f2 = f.clone().into_owned();
                f2.rows.retain(|r| r.label != title);
                details(c, &f2, BLUE);
            }
        }
    }
}

fn ai_all(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let src: [(&'static str, &str, Col); 6] = [
        ("claude_code", "Claude Code", CLAUDE),
        ("codex", "Codex", hex(0x9AA0A6)),
        ("anthropic", "Claude API", CLAUDE),
        ("openai", "OpenAI API", hex(0x10A37F)),
        ("openrouter", "OpenRouter", hex(0x6467F2)),
        ("cursor", "Cursor", hex(0xBBBBBB)),
    ];
    let mut rows: Vec<(&str, String, f32, Col)> = Vec::new();
    for (conn, name, col) in src {
        let f = match (x.hub.feed(conn), x.hub.err(conn)) {
            (Some(f), None) => Cow::Borrowed(f),
            _ if x.sample && (conn == "claude_code" || conn == "openai" || conn == "codex") => Cow::Owned(super::hub::sample(conn)),
            _ => continue,
        };
        let v = if f.unit == "tokens" { format!("{} today", compact(f.value.unwrap_or(0.0))) } else { headline(&f) };
        rows.push((name, v, f.frac.unwrap_or(0.0), col));
    }
    let badge = super::canvas::gly(G_AI, 0x2C2C34, 0xD97757);
    if matches!(sh, Shape::Mini | Shape::Dock) {
        out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Connect("anthropic")));
        let s = match rows.first() {
            Some(r) => Sum::new(Vis::Badge(badge), r.1.split(' ').next().unwrap_or("").to_string(), r.0),
            None => Sum::new(Vis::Badge(badge), if sh == Shape::Mini { "" } else { "AI Usage" }, "Click to connect"),
        };
        compact_sum(c, &s, sh);
        return;
    }
    let small = sh == Shape::Small;
    c.tw("AI Usage", 14.0, 12.0, 12.5, 600, c.text2(), 140.0);
    if rows.is_empty() {
        out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Connect("anthropic")));
        c.text("Use Claude Code or Codex on this PC, or connect an API key.", 14.0, 40.0, 11.0, 400, c.text3(), Align::Left, c.w - 28.0, Font::Text);
        return;
    }
    let n = if c.h > 150.0 { 6 } else { 3 };
    let rh = if c.h > 150.0 || small { 28.0 } else { 20.0 };
    for (k, (name, v, frac, col)) in rows.iter().take(n).enumerate() {
        let y = 34.0 + k as f32 * rh;
        c.circle(18.0, y + 8.0, 4.0, *col);
        if small {
            c.tw(name, 28.0, y, 11.0, 500, c.text1(), c.w - 40.0);
            c.tw(v, 28.0, y + 13.0, 10.0, 400, c.text2(), c.w - 40.0);
        } else {
            c.tw(name, 28.0, y, 12.0, 500, c.text1(), 110.0);
            c.bar(140.0, y + 5.0, c.w - 140.0 - 96.0, 5.0, if *frac > 0.0 { *frac } else { 0.5 }, *col);
            c.tr(v, c.w - 14.0, y, 11.5, 500, c.text2());
        }
    }
}

fn goal(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let goal: f64 = x.cfg.widget_settings.get("revenue-goal").and_then(|m| m.get("goal")).and_then(|v| v.parse().ok()).unwrap_or(10_000.0);
    let mut total = 0.0;
    let mut n = 0;
    for conn in ["stripe", "lemonsqueezy", "paddle", "shopify", "gumroad", "polar"] {
        let f = match (x.hub.feed(conn), x.hub.err(conn)) {
            (Some(f), None) => Cow::Borrowed(f),
            _ if x.sample && conn == "stripe" => Cow::Owned(super::hub::sample(conn)),
            _ => continue,
        };
        total += f.value.unwrap_or(0.0);
        n += 1;
    }
    let frac = (total / goal) as f32;
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Connect("revenue-goal")));
    let pct = format!("{:.0}%", (frac * 100.0).min(999.0));
    if matches!(sh, Shape::Mini | Shape::Dock) {
        let s = if sh == Shape::Mini { Sum::new(Vis::Ring(frac, GREEN, pct), "", "") } else { Sum::new(Vis::Ring(frac, GREEN, String::new()), money(total, Some("USD")), format!("{pct} of goal")) };
        compact_sum(c, &s, sh);
        return;
    }
    if sh == Shape::Small {
        let r = 30.0;
        c.ring(c.w / 2.0, 14.0 + r, r - 4.0, 7.0, frac, GREEN);
        c.tc(&pct, c.w / 2.0, 14.0 + r - 9.0, 13.0, 600, c.text1(), 60.0);
        c.tc(&money(total, Some("USD")), c.w / 2.0, 82.0, 15.0, 600, c.text1(), c.w - 16.0);
        c.tc(&format!("of {}", money(goal, Some("USD"))), c.w / 2.0, 100.0, 10.5, 400, c.text2(), c.w - 16.0);
        return;
    }
    let big = sh == Shape::Large;
    let r = if big { 56.0 } else { 30.0 };
    let (cx, cy) = (16.0 + r + 6.0, c.h / 2.0);
    c.ring(cx, cy, r, if big { 11.0 } else { 7.0 }, frac, GREEN);
    c.tc(&pct, cx, cy - if big { 14.0 } else { 9.0 }, if big { 20.0 } else { 13.0 }, 600, c.text1(), 2.0 * r);
    let tx = cx + r + 22.0;
    c.num(&money(total, Some("USD")), tx, cy - 30.0, 26.0, 600, c.text1());
    let sub = if n == 0 { "Connect a payment provider".to_string() } else { format!("of {} goal · last 30 days", money(goal, Some("USD"))) };
    c.text(&sub, tx, cy + 6.0, 11.5, 400, c.text2(), Align::Left, c.w - tx - 14.0, Font::Text);
}

fn wallpaper(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, wic: &IWICImagingFactory, sh: Shape) {
    let badge = super::canvas::gly(G_IMAGE, 0x2C6BED, 0xFFFFFF);
    let g = got(x, "wallpaper");
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    if let Got::Feed(f) = &g {
        let img = f.extra.get("image_path").and_then(|p| x.hub.wallpaper_img.as_ref().filter(|w| &w.0 == p).map(|w| w.1.clone()).or_else(|| load_image(wic, p)));
        let r = if dense { 12.0 } else { 18.0 };
        match img {
            Some(src) => c.image(&src, 0.0, 0.0, c.w, c.h, r),
            None => c.rr_grad(0.0, 0.0, c.w, c.h, r, hex(0x5B8DEF), hex(0x1C3D7A)),
        }
        if !dense {
            c.rr_grad(0.0, c.h * 0.45, c.w, c.h * 0.55, r, (0.0, 0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 0.55));
            let title = f.extra.get("title").cloned().or_else(|| f.text.clone()).unwrap_or_else(|| "Image of the day".into());
            if sh == Shape::Small {
                c.tw(&title, 12.0, c.h - 24.0, 11.5, 600, WHITE, c.w - 24.0);
            } else {
                c.tw(&title, 14.0, c.h - 30.0, 12.5, 600, WHITE, c.w - 130.0);
                let (bw, bh) = (104.0, 24.0);
                c.rr(c.w - bw - 12.0, c.h - bh - 10.0, bw, bh, 12.0, (1.0, 1.0, 1.0, 0.92));
                c.tc("Set wallpaper", c.w - 12.0 - bw / 2.0, c.h - bh - 6.0, 11.0, 600, (0.1, 0.1, 0.12, 1.0), bw);
            }
        }
        if f.extra.contains_key("image_path") && !x.sample {
            out.hits.push(((0.0, 0.0, c.w, c.h), WAct::SetWallpaper));
        }
        return;
    }
    bg(c, dense);
    placeholder(c, out, def.name, &badge, "wallpaper", &g, sh);
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
const INDIGO: Col = hex(0x8E8EFF);
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
enum Lead {
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
fn seg_time(c: &Canvas, x0: f32, y0: f32, dh: f32, d: [u32; 4], lead: Lead, on: Col, off: Col, colon: Col) {
    let dw = dh * 0.55;
    let mut sx = x0;
    for (i, v) in d.iter().enumerate() {
        if i == 0 && lead == Lead::None {
            continue;
        }
        if i == 2 {
            let cx = sx - dw * 0.25 + dw * 0.75 / 2.0;
            let r = (dw * 0.1).max(1.0);
            c.circle(cx, y0 + dh * 0.3, r, colon);
            c.circle(cx, y0 + dh * 0.7, r, colon);
            sx += dw * 0.75 - dw * 0.25;
        }
        if i == 0 && lead == Lead::Ghost {
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
    let (g, gcol) = if (6..18).contains(&h) { (G_SUN, if c.light { ORANGE } else { YELLOW }) } else { (G_MOON, if c.light { INDIGO_LIGHT } else { INDIGO }) };
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
    let lead = if hr >= 10 { Lead::Digit } else { Lead::Ghost };
    let md = format!("{} {}", &t.day[..3], n.day);
    match sh {
        Shape::Mini => {
            // No ghost here: "5:38" alone reads at a glance; "85:38" didn't.
            let (lead, nd) = if hr >= 10 { (Lead::Digit, 4) } else { (Lead::None, 3) };
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
