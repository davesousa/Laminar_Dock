//! `laminar.exe --widget-sheet out.png [light] [scale=2] [empty] [filter]`: paint every
//! widget at every size (three dock widths, small, medium and large cards) into one PNG,
//! with sample data. A quick way to review layouts without running the dock.
//!
//! - `light`: light theme (dark is the default)
//! - `scale=2`: render at 2x (like a 200% display) to check fine detail
//! - `empty`: no sample data, so connector widgets show their waiting state
//! - `filter`: a widget id fragment or category name ("clocks"), or a comma-separated
//!   list of exact ids ("stripe,stripe-chart")
//! - `state=setup|error`: force every connector face into its setup or error state
//!   (setup also clears the weather location, so the weather faces show "Set location")
//! - `battery=charging|plugged|none`: the battery sample charging, plugged in and not
//!   charging, or a PC with no battery
//! - `tray=N`: N sample tray apps for Background Apps (0 shows its empty state)
//! - `update=pending|restart|current|unknown|none`: the Windows Update status (none = not
//!   checked yet)
//!
//! The sheet never reads the user's settings or saved widget data: screen time, notes and
//! app names are made up (or empty with `empty`).

use std::collections::HashMap;

use windows::core::{Interface, Result, HSTRING};
use windows::Win32::Foundation::{GENERIC_WRITE, HWND};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows_numerics::Matrix3x2;

use super::canvas::Canvas;
use super::catalog::{self, Size};
use super::{card_size, dock_width, Ctx, Hub, Now};

pub fn run(out: &str, light: bool, filter: Option<&str>, scale: f32, empty: bool, opts: &[String]) -> Result<()> {
    let opt = |k: &str| opts.iter().find_map(|a| a.strip_prefix(k)?.strip_prefix('=').map(str::to_string));
    let state = match opt("state").as_deref() {
        Some("setup") => 1,
        Some("error") => 2,
        _ => 0,
    };
    super::paint::SHEET_STATE.store(state, std::sync::atomic::Ordering::Relaxed);
    let battery = match opt("battery").as_deref() {
        Some("charging") => 1,
        Some("plugged") => 2,
        Some("none") => 3,
        _ => 0,
    };
    super::paint::SHEET_BATTERY.store(battery, std::sync::atomic::Ordering::Relaxed);
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        crate::ui::theme::set_light(light);

        let matches = |d: &catalog::Def, f: &str| {
            if f.contains(',') {
                f.split(',').any(|id| id.trim() == d.id)
            } else {
                d.id.contains(f) || d.cats.iter().any(|c| format!("{c:?}").eq_ignore_ascii_case(f))
            }
        };
        let defs: Vec<&'static catalog::Def> = catalog::all().filter(|d| filter.map(|f| matches(d, f)).unwrap_or(true)).collect();
        let b = 54.0f32;
        let col_w = 268.0f32;
        let label_w = 150.0;
        let pad = 16.0;
        // Columns: label | dock S | dock M | dock L | small | medium | large
        let dock_cols = [Size::Small, Size::Medium, Size::Large];
        let row_h = card_size(Size::Large, col_w).1 + pad;
        let widths: Vec<f32> = defs.iter().map(|d| dock_cols.iter().map(|s| dock_width(d, *s, b) + pad).sum::<f32>()).collect();
        let dock_w = widths.iter().cloned().fold(0.0, f32::max);
        let small_w = card_size(Size::Small, col_w).0;
        let w = ((label_w + dock_w + small_w + col_w * 2.0 + pad * 4.0) * scale).ceil() as u32;
        let h = ((defs.len() as f32 * row_h + pad) * scale).ceil() as u32;

        let bmp = wic.CreateBitmap(w, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnDemand)?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            ..Default::default()
        };
        let base: &ID2D1Factory = &factory;
        let rt = base.CreateWicBitmapRenderTarget(&bmp, &props)?;
        let dc: ID2D1DeviceContext = rt.cast()?;

        let mut hub = Hub::with_screen(HWND::default(), if empty { Default::default() } else { sample_screen(&Now::get()) });
        if empty {
            // Set the fields directly: Hub::set_mic would really mute the microphone.
            hub.mic_muted = Some(true);
        } else {
            hub.output_name = Some("Speakers (Realtek(R) Audio)".into());
            hub.notes = "Groceries
Oat milk
Coffee beans
Call the dentist".into();
            // An earlier run, so the Stopwatch Large shows its idle facts.
            hub.stopwatch.last = Some(std::time::Duration::from_secs(12 * 60 + 34));
            hub.stopwatch.today = std::time::Duration::from_secs(48 * 60);
            hub.stopwatch.day = super::hub::day_key(&Now::get());
        }
        let hub = hub;
        let weather = sample_weather(&Now::get());
        let stats = laminar_sys::stats::Stats { cpu_percent: 23.0, mem_used_gb: 21.3, mem_total_gb: 32.0, disk_c_used_percent: 74.0, net_down_bps: 2_400_000.0, net_up_bps: 310_000.0 };
        let control = crate::services::ControlState {
            volume: Some(0.42),
            muted: Some(empty),
            wifi: Some(true),
            bluetooth: Some(true),
            dark: !light,
            media: (!empty).then(|| laminar_sys::media::NowPlaying {
                title: "Midnight Drive".into(),
                artist: "The Long Roads".into(),
                album: "Night Lines".into(),
                playing: true,
                app_id: "Spotify.exe".into(),
                thumbnail_png: None,
            }),
        };
        let mut cfg = crate::config::Config::default();
        if state == 1 {
            cfg.weather_city.clear();
        }
        let cfg = cfg;
        let pinned = vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string(), "e".to_string()];
        let names: HashMap<String, String> =
            if empty { HashMap::new() } else { pinned.iter().zip(["Mail", "Calendar", "Photos", "Terminal", "Notepad"]).map(|(id, n)| (id.clone(), n.to_string())).collect() };
        let n_tray = opt("tray").and_then(|v| v.parse::<u64>().ok()).unwrap_or(14);
        let tray: Vec<(u64, String)> = (1..=n_tray).map(|k| (k, format!("App {k}"))).collect();
        use laminar_sys::update::UpdateStatus;
        let update = match opt("update").as_deref() {
            Some("restart") => Some(UpdateStatus::RestartRequired),
            Some("current") => Some(UpdateStatus::UpToDate),
            Some("unknown") => Some(UpdateStatus::Unknown),
            Some("none") => None,
            _ => Some(UpdateStatus::Pending(3)),
        };
        let ctx = Ctx {
            hub: &hub,
            cfg: &cfg,
            weather: if empty || state == 1 { None } else { Some(&weather) },
            stats: Some(&stats),
            control: &control,
            pinned: &pinned,
            names: &names,
            whatsapp: Some(true),
            tray: &tray,
            hover_tray: None,
            update: update.as_ref(),
            sample: !empty,
            now: Now::get(),
        };

        dc.BeginDraw();
        dc.Clear(Some(&if light { D2D1_COLOR_F { r: 0.86, g: 0.87, b: 0.9, a: 1.0 } } else { D2D1_COLOR_F { r: 0.11, g: 0.11, b: 0.14, a: 1.0 } }));
        let ink = if light { D2D1_COLOR_F { r: 0.1, g: 0.1, b: 0.12, a: 1.0 } } else { D2D1_COLOR_F { r: 0.9, g: 0.9, b: 0.92, a: 1.0 } };
        let brush = dc.CreateSolidColorBrush(&ink, None)?;
        let icon_brush = dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.35, g: 0.55, b: 0.95, a: 0.9 }, None)?;
        let fmt = dw.CreateTextFormat(&HSTRING::from("Segoe UI Variable Text"), None, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 12.0, &HSTRING::from("en-us"))?;
        for (row, def) in defs.iter().enumerate() {
            let y = pad + row as f32 * row_h;
            dc.SetTransform(&Matrix3x2 { M11: scale, M12: 0.0, M21: 0.0, M22: scale, M31: 0.0, M32: 0.0 });
            let name: Vec<u16> = format!("{}\n{}", def.name, def.id).encode_utf16().collect();
            dc.DrawText(&name, &fmt, &D2D_RECT_F { left: pad, top: y, right: label_w, bottom: y + 60.0 }, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            let mut x = label_w;
            let mut cells: Vec<(f32, f32, f32, f32, bool)> = Vec::new();
            for s in dock_cols {
                let ww = dock_width(def, s, b);
                cells.push((x, y, ww, b, true));
                x += ww + pad;
            }
            x = label_w + dock_w + pad;
            for s in [Size::Small, Size::Medium, Size::Large] {
                let (cw, ch) = card_size(s, col_w);
                cells.push((x, y, cw, ch, false));
                x += cw + pad;
            }
            for (cx, cy, cw, ch, dense) in cells {
                dc.SetTransform(&Matrix3x2::translation(cx * scale, cy * scale));
                let canvas = Canvas::new(&dc, &factory, &dw, cw, ch, scale, light);
                let painted = super::paint::paint(&canvas, def, &ctx, dense, &wic);
                drop(canvas);
                dc.SetTransform(&Matrix3x2 { M11: scale, M12: 0.0, M21: 0.0, M22: scale, M31: cx * scale, M32: cy * scale });
                for (ix, iy, isz, _) in &painted.icons {
                    dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: *ix, top: *iy, right: ix + isz, bottom: iy + isz }, radiusX: isz * 0.22, radiusY: isz * 0.22 }, &icon_brush);
                }
            }
        }
        dc.EndDraw(None, None)?;

        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(out), GENERIC_WRITE.0)?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.unwrap();
        frame.Initialize(None)?;
        frame.WriteSource(&bmp, std::ptr::null())?;
        frame.Commit()?;
        enc.Commit()?;
        println!("{} widgets -> {out} ({w}x{h})", defs.len());
        Ok(())
    }
}

/// Made-up screen time for today: a few apps and a working-day hour profile.
fn sample_screen(now: &Now) -> super::hub::ScreenTime {
    let apps = [("Visual Studio Code", 9_840u64), ("Google Chrome", 6_120), ("Slack", 2_460), ("Spotify", 1_380), ("File Explorer", 1_860)];
    let mut hours = vec![0u32; 24];
    for (h, m) in [(8, 22), (9, 51), (10, 56), (11, 48), (12, 18), (13, 44), (14, 55), (15, 38), (16, 29)] {
        if h <= now.hour as usize {
            hours[h] = m * 60;
        }
    }
    super::hub::ScreenTime { day: now.ymd(), apps: apps.iter().map(|(a, s)| (a.to_string(), *s)).collect(), hours }
}

/// Made-up weather that follows the real clock: the hourly forecast starts at the next full
/// hour and the daily forecast at today, so previews agree with the clocks beside them.
fn sample_weather(now: &Now) -> laminar_sys::weather::Weather {
    // A day that warms to 21° at 3 PM and cools to 11° before dawn; clear after dark.
    let temp_at = |h: u32| (16.0 + 5.0 * ((h as f32 - 15.0) * std::f32::consts::PI / 12.0).cos()).round();
    let code_at = |h: u32| if (7..17).contains(&h) { if h < 15 { 2 } else { 1 } } else { 0 };
    let hourly = (1..=8u32).map(|i| (now.hour + i) % 24).map(|h| (h as u8, temp_at(h), code_at(h))).collect();
    // Today and the next six dates.
    let mut dates = Vec::new();
    let (mut y, mut m, mut d) = (now.year, now.month, now.day);
    for _ in 0..7 {
        dates.push(format!("{y:04}-{m:02}-{d:02}"));
        d += 1;
        if d > super::days_in_month(y, m) {
            d = 1;
            m += 1;
            if m > 12 {
                m = 1;
                y += 1;
            }
        }
    }
    laminar_sys::weather::Weather {
        temp_c: 17.0,
        code: 2,
        description: "Partly cloudy".into(),
        high_c: 21.0,
        low_c: 11.0,
        hourly,
        feels_c: 16.0,
        humidity: 62.0,
        wind_kmh: 14.0,
        uv: 4.0,
        is_day: (7..17).contains(&now.hour),
        sunrise: "07:02".into(),
        sunset: "16:58".into(),
        // Today, then a rainy midweek dip and a clear weekend.
        daily: [(21.0, 11.0, 2), (19.0, 10.0, 3), (16.0, 9.0, 61), (13.0, 6.0, 63), (12.0, 4.0, 3), (15.0, 7.0, 1), (17.0, 9.0, 0)]
            .iter()
            .zip(dates)
            .map(|((hi, lo, code), date)| (date, *hi, *lo, *code))
            .collect(),
        now: format!("{}T{:02}:{:02}", now.ymd(), now.hour, now.minute),
    }
}
