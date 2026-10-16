//! `--debug-audio`: live analysis view in the terminal (no window, no GPU).
//!
//! On a TTY it redraws a full-screen dashboard ~30 times per second; when
//! stdout is redirected it prints one compact line every 100 ms instead.

use std::fmt::Write as _;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use crate::audio::AudioEngine;
use crate::snapshot::Snapshot;

const EIGHTHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const PARTIAL: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
const CHART_ROWS: usize = 12;

pub fn run(engine: &AudioEngine) -> anyhow::Result<()> {
    let tty = std::io::stdout().is_terminal();
    let period = Duration::from_millis(if tty { 33 } else { 100 });
    let mut out = std::io::stdout().lock();
    let mut snap = Snapshot::default();
    let mut screen = String::new();
    let started = Instant::now();
    let (mut rate_t, mut rate_seq, mut rate) = (Instant::now(), 0u64, 0.0f32);

    if tty {
        write!(out, "\x1b[2J")?;
    }
    loop {
        engine.snapshot().read_into(&mut snap);
        if rate_t.elapsed() >= Duration::from_secs(1) {
            rate = (snap.seq - rate_seq) as f32 / rate_t.elapsed().as_secs_f32();
            rate_seq = snap.seq;
            rate_t = Instant::now();
        }
        screen.clear();
        if tty {
            dashboard(&mut screen, &snap, &engine.status().to_string(), rate)?;
            write!(out, "\x1b[H{screen}\x1b[J")?;
        } else {
            line(&mut screen, &snap, started.elapsed().as_secs_f32())?;
            writeln!(out, "{screen}")?;
        }
        out.flush()?;
        std::thread::sleep(period);
    }
}

fn dashboard(s: &mut String, snap: &Snapshot, status: &str, rate: f32) -> std::fmt::Result {
    writeln!(s, "miz — audio debug    (Ctrl+C to quit)\x1b[K")?;
    writeln!(s, "source: {status}\x1b[K")?;
    let age = snap.published_at.map_or_else(
        || "never".into(),
        |t| format!("{:.0} ms ago", t.elapsed().as_secs_f32() * 1e3),
    );
    writeln!(
        s,
        "gain ref {:6.1} dB   rms {:6.1} dBFS   wave gain x{:5.1}   analyses {:4.0}/s   last {age}\x1b[K",
        snap.gain_db, snap.rms_db, snap.wave_gain, rate
    )?;
    writeln!(s, "\x1b[K")?;
    for (name, v) in [
        ("bass", snap.bass),
        ("low-mid", snap.low_mid),
        ("high-mid", snap.high_mid),
        ("treble", snap.treble),
        ("loudness", snap.loudness),
        ("width", snap.width),
        ("idle", snap.idle),
    ] {
        writeln!(s, "{name:>9} {} {v:4.2}\x1b[K", meter(v, 40))?;
    }
    let dot = if snap.beat > 0.5 { '●' } else { '○' };
    writeln!(
        s,
        "{:>9} {} {:4.2} {dot}  beats {:5}  flux {:.3} / thr {:.3}\x1b[K",
        "beat",
        meter(snap.beat, 40),
        snap.beat,
        snap.beat_count,
        snap.flux,
        snap.flux_threshold
    )?;
    writeln!(s, "\x1b[K")?;
    writeln!(s, "bands ({}), 30 Hz … 16 kHz:\x1b[K", snap.band_count)?;
    let bands = snap.bands();
    for row in 0..CHART_ROWS {
        s.push_str("  ");
        let base = (CHART_ROWS - 1 - row) as f32;
        for &b in bands {
            let eighths = ((b * CHART_ROWS as f32 - base) * 8.0).clamp(0.0, 8.0) as usize;
            s.push(EIGHTHS[eighths]);
        }
        s.push_str("\x1b[K\n");
    }
    Ok(())
}

fn line(s: &mut String, snap: &Snapshot, t: f32) -> std::fmt::Result {
    write!(
        s,
        "t={t:6.2} gain={:6.1} rms={:6.1} bass={:.2} lm={:.2} hm={:.2} tr={:.2} loud={:.2} \
         width={:.2} beat={:.2} beats={:4} flux={:.3}/{:.3} idle={:.2} |",
        snap.gain_db,
        snap.rms_db,
        snap.bass,
        snap.low_mid,
        snap.high_mid,
        snap.treble,
        snap.loudness,
        snap.width,
        snap.beat,
        snap.beat_count,
        snap.flux,
        snap.flux_threshold,
        snap.idle
    )?;
    for &b in snap.bands() {
        s.push(EIGHTHS[(b * 8.0).round().clamp(0.0, 8.0) as usize]);
    }
    s.push('|');
    Ok(())
}

/// Horizontal bar of `width` cells with 1/8-cell resolution.
fn meter(v: f32, width: usize) -> String {
    let eighths = (v.clamp(0.0, 1.0) * (width * 8) as f32).round() as usize;
    let (full, part) = (eighths / 8, eighths % 8);
    let mut m = "█".repeat(full);
    if full < width {
        m.push(PARTIAL[part]);
        m.push_str(&"·".repeat(width - full - 1));
    }
    m
}
