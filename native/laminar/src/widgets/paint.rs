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
    /// Dock caption: the mode when idle, "Ends 3:42 PM" / "Since 2:14 PM" while running.
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
                dock_caption: if running { format!("Since {since}") } else if used { "Paused".into() } else { "Stopwatch".into() },
                caption: if running { format!("Since {since}") } else { state.into() },
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
            c.text_base(&v.dock_caption, x0, 44.0 * kk, style::FOOT.size * kk, style::FOOT.weight, c.text2(), Align::Left, right - x0);
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

/// The transport row as bare glyphs (prev, play/pause, next) at the given centres.
#[allow(clippy::too_many_arguments)]
fn controls(c: &Canvas, out: &mut Painted, xs: [f32; 3], cy: f32, sizes: [f32; 3], playing: bool) {
    let play = if playing { G_PAUSE_FILL } else { G_PLAY_FILL };
    for (i, (g, a)) in [(G_PREV_FILL, WAct::MediaPrev), (play, WAct::MediaPlay), (G_NEXT_FILL, WAct::MediaNext)].into_iter().enumerate() {
        transport_glyph(c, g, xs[i], cy, sizes[i], c.text1());
        let hd = style::HIT_MIN;
        out.hits.push(((xs[i] - hd / 2.0, cy - hd / 2.0, hd, hd), a));
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
    /// The supporting line under the artist: the album, else the source app, else "Paused"
    /// when paused. Empty while playing with neither: "Now playing" would only repeat the
    /// widget's name.
    fn third(&self) -> String {
        if !self.album.is_empty() {
            self.album.clone()
        } else if !self.source.is_empty() {
            self.source.clone()
        } else if self.playing {
            String::new()
        } else {
            "Paused".into()
        }
    }
    /// "Playing in Spotify" / "Paused in Spotify"; without a source, "Paused" or nothing.
    fn where_line(&self) -> String {
        let verb = if self.playing { "Playing" } else { "Paused" };
        if self.source.is_empty() {
            if self.playing {
                String::new()
            } else {
                "Paused".into()
            }
        } else {
            format!("{verb} in {}", self.source)
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
fn timeline(_m: &Track) -> Option<(f32, f32)> {
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
/// Only a title too long for two lines is cut, at a word boundary.
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
    let (lines, fs) = wrap_title(c, &t.title, role, w, 2);
    if lines.len() == 2 && lines.iter().all(|l| c.measure(l, fs, style::TITLE.weight, Font::Text).0 <= w) {
        let pitch = (fs * 1.25).round();
        let b0 = (c.h + c.cap_h(fs) - pitch) / 2.0;
        for (i, l) in lines.iter().enumerate() {
            c.text_base(l, x0, b0 + i as f32 * pitch, fs, style::TITLE.weight, c.text1(), Align::Left, w);
        }
        return;
    }
    let cut = word_cut(c, &t.title, min, style::TITLE.weight, w);
    c.text_base(&cut, x0, 25.0 * kk, min, style::TITLE.weight, c.text1(), Align::Left, w);
    c.text_base(&t.artist, x0, 39.0 * kk, style::FOOT.size * kk, style::FOOT.weight, c.text2(), Align::Left, w);
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
            if t.idle {
                let p = style::BADGE_DOCK * kk;
                c.rr(m, (c.h - p) / 2.0, p, p, p * style::BADGE_R, c.plate());
                c.glyph(G_MUSIC, m + p / 2.0, c.h / 2.0, p * style::GLYPH_IN, c.text3());
                let x0 = 52.0 * kk;
                c.text_base("Not playing", x0, 30.0 * kk, style::TITLE.size * kk, style::TITLE.weight, c.text1(), Align::Left, c.w - m - x0);
                let hint = "Play in any app";
                let fs = fit_down(c, hint, style::FOOT.size * kk, style::FOOT.min * kk, style::FOOT.weight, c.w - m - x0);
                c.text_base(hint, x0, 44.0 * kk, fs, style::FOOT.weight, c.text2(), Align::Left, c.w - m - x0);
                return;
            }
            let s = 42.0 * kk;
            let a0 = 6.0 * kk;
            if k == MediaKind::Disc {
                record(c, x, wic, &t, a0 + s / 2.0, c.h / 2.0, s, true);
            } else {
                art_note(c, x, wic, a0, a0, s, style::R_MIN * kk, t.playing || wide);
            }
            let x0 = 56.0 * kk;
            let pd = style::BTN_SECONDARY_DOCK * kk;
            // The slot is only as wide as what it holds, so the title keeps its room.
            let used = match (k, timeline(&t)) {
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
                MediaKind::Bar => match timeline(&t) {
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
                    // Visual only: the record, centred on its card with at least the
                    // Small margin of glass around it.
                    let d = 92.0;
                    let (cx, cy) = (c.w / 2.0, c.h / 2.0);
                    record(c, x, wic, &t, cx, cy, d, true);
                    if !t.idle && !t.playing {
                        paused_overlay(c, cx, cy, 34.0);
                    }
                }
                MediaKind::Full => {
                    art_or_idle(c, x, wic, &t, m, m, 40.0, r);
                    if t.idle {
                        card_track_text(c, &Track { artist: "Play in any app".into(), ..t.clone() }, m, inner, 70.0, 84.0, style::FOOT.size, None);
                        return;
                    }
                    card_track_text(c, &t, m, inner, 70.0, 84.0, style::FOOT.size, None);
                    // Prev's left ink edge on the margin, next's right ink edge on the other.
                    controls(c, out, [m + 7.0, c.w / 2.0, c.w - m - 7.0], 98.0, [14.0, 18.0, 14.0], t.playing);
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
                    // Bar: art and button on top, the track and its progress below.
                    art_or_idle(c, x, wic, &t, m, m, 40.0, r);
                    let p = style::BTN_SECONDARY;
                    if t.idle {
                        card_track_text(c, &Track { artist: "Play in any app".into(), ..t.clone() }, m, inner, 74.0, 88.0, style::FOOT.size, None);
                        return;
                    }
                    play_plate(c, out, c.w - m - p / 2.0, m + p / 2.0, p, t.playing);
                    match timeline(&t) {
                        Some((pos, len)) => {
                            card_track_text(c, &t, m, inner, 74.0, 88.0, style::FOOT.size, None);
                            c.bar(m, 102.0, inner, style::METER_H_COMPACT, (pos / len.max(1.0)).clamp(0.0, 1.0), c.text1());
                        }
                        None => {
                            // Bottom-anchored: with no "where" line the track sits on the
                            // bottom margin instead of leaving it bare.
                            let wl = t.where_line();
                            let (tb, ab) = if wl.is_empty() { (86.0, 106.0) } else { (74.0, 88.0) };
                            let tw = inner - LEVEL_W - style::SP[2];
                            let fs = fit_down(c, &t.title, style::TITLE.size, style::TITLE.min, style::TITLE.weight, tw);
                            let w = c.text_base(&t.title, m, tb, fs, style::TITLE.weight, c.text1(), Align::Left, tw);
                            level_glyph(c, m + w + style::SP[2], tb, t.playing, 1.0);
                            let asz = if wl.is_empty() { style::CAPTION.size } else { style::FOOT.size };
                            c.text_base(&t.artist, m, ab, asz, 400, c.text2(), Align::Left, inner);
                            if !wl.is_empty() {
                                c.text_base(&wl, m, 106.0, style::FOOT.size, 400, c.text3(), Align::Left, inner);
                            }
                        }
                    }
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
                    art_or_idle(c, x, wic, &t, m, m, 96.0, r);
                    let tx = m + 96.0 + style::SP[5];
                    let third = if t.idle { String::new() } else { t.third() };
                    let t2 = if t.idle { Track { artist: "Play in any app".into(), ..t.clone() } } else { t.clone() };
                    large_track_text(c, &t2, tx, c.w - m - tx, 20.0, &third, 2);
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
                        c.text_base(&t.where_line(), m, c.h - m, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, pcx - pd / 2.0 - style::SP[3] - m);
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
                    if timeline(&t).is_none() {
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
            controls(c, out, [104.0, 144.0, 184.0], 66.0, [16.0, 20.0, 16.0], t.playing);
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
            // Two lines centre on the art; a third (album, app or "Paused") goes below.
            let third = t.third();
            if third.is_empty() {
                card_track_text(c, t, tx, w, 44.0, 60.0, style::CAPTION.size, None);
            } else {
                card_track_text(c, t, tx, w, 36.0, 52.0, style::CAPTION.size, Some((68.0, third)));
            }
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
            let (p, cy) = (style::BTN_SECONDARY_SMALL, 66.0);
            play_plate(c, out, tx + p / 2.0, cy, p, t.playing);
            let ncx = tx + p * 1.5 + style::SP[3];
            round_button(c, out, ncx, cy, p, c.plate(), G_NEXT_FILL, c.text1(), WAct::MediaNext);
        }
        _ => {
            let s = if timeline(t).is_some() { 40.0 } else { 32.0 };
            art_or_idle(c, x, wic, t, m, m, s, style::R_MIN + 2.0);
            let tx = m + s + style::SP[5];
            let pd = style::BTN_SECONDARY;
            let right = c.w - m - pd - style::SP[5];
            if t.idle {
                card_track_text(c, &idle_t(), tx, c.w - m - tx, style::HEADER_BASE, 47.0, style::CAPTION.size, None);
                return;
            }
            match timeline(t) {
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
                    controls(c, out, [mid - 64.0, mid, mid + 64.0], cy, [12.0, 14.0, 12.0], t.playing);
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
            c.rr(m, (c.h - p) / 2.0, p, p, p * style::BADGE_R, if muted { RED } else { c.plate() });
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
            c.rr(m, m, p, p, p * style::BADGE_R, if muted { RED } else { c.plate() });
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
            c.rr(m, m, p, p, p * style::BADGE_R, if muted { RED } else { c.plate() });
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
                let (tf, ink, sub) = if muted { (RED, WHITE, alpha(WHITE, 0.8)) } else { (c.plate(), c.text1(), c.text2()) };
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
    let (fill, g, ink) = if muted { (RED, G_MIC_OFF, WHITE) } else { (c.plate(), G_MIC, c.text1()) };
    let state_col = if muted { RED } else { c.text2() };
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
            c.text_base("Microphone", tx, 46.0, style::VALUE.size, style::VALUE.weight, c.text1(), Align::Left, c.w - m - tx);
            c.text_base(state, tx, 64.0, style::CAPTION.size, style::CAPTION.weight, state_col, Align::Left, c.w - m - tx);
        }
        Shape::Large => {
            // A state tile, then two facts.
            let m = style::M_CARD;
            let i = 6.0;
            let (tw, th) = (c.w - 2.0 * i, 112.0);
            c.rr(i, i, tw, th, style::concentric(style::CARD_R, i), c.plate());
            let d = 56.0;
            let (cx, cy) = (i + style::SP[6] + d / 2.0, i + th / 2.0);
            c.circle(cx, cy, d / 2.0, if muted { RED } else { c.plate() });
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
