//! Animation system: creature art lives in `art/<stage>/<clip>.txt` (embedded into the
//! binary at build time), each file a clip of one-or-more frames separated by `---`.
//! Nothing here does terminal I/O — it just parses art and resolves which frame to draw.
//!
//! Clip file format:
//!   # fps=3 loop=pingpong     (optional; defaults fps=2 loop=loop)
//!   <frame 1 lines>
//!   ---
//!   <frame 2 lines>
//!
//! Adding art: drop a new `art/<stage>/<clip>.txt` (or a new stage folder + a Stage entry
//! in model.rs). The validation tests below guard size/width so nothing ever reflows or
//! overflows the fixed card.

use std::collections::HashMap;
use std::sync::OnceLock;

use include_dir::{include_dir, Dir};
use unicode_width::UnicodeWidthStr;

static ART: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/art");

/// One frame of art. `w`/`h` are its display size; lines keep their significant leading
/// whitespace (trailing is cosmetic — it renders as blank cells).
pub struct Frame {
    pub lines: Vec<String>,
    pub w: u16,
    pub h: u16,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LoopMode {
    Loop,
    Once,
    PingPong,
}

pub struct Clip {
    pub frames: Vec<Frame>,
    pub fps: u8,
    pub loop_mode: LoopMode,
}

/// Which animation to play. Every stage must have `Idle`; the rest fall back to it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClipKind {
    Idle,
    Blink,
    Sad,
    Happy,
    Celebrate,
}

impl ClipKind {
    fn file(self) -> &'static str {
        match self {
            ClipKind::Idle => "idle",
            ClipKind::Blink => "blink",
            ClipKind::Sad => "sad",
            ClipKind::Happy => "happy",
            ClipKind::Celebrate => "celebrate",
        }
    }
    const ALL: [ClipKind; 5] = [
        ClipKind::Idle,
        ClipKind::Blink,
        ClipKind::Sad,
        ClipKind::Happy,
        ClipKind::Celebrate,
    ];
}

pub struct Sprite {
    clips: HashMap<ClipKind, Clip>,
}

impl Sprite {
    /// The requested clip, or Idle if this stage doesn't define it.
    pub fn clip(&self, kind: ClipKind) -> &Clip {
        self.clips
            .get(&kind)
            .or_else(|| self.clips.get(&ClipKind::Idle))
            .expect("every stage has an idle clip")
    }
    pub fn has(&self, kind: ClipKind) -> bool {
        self.clips.contains_key(&kind)
    }
}

fn make_frame(mut lines: Vec<String>) -> Frame {
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let w = lines.iter().map(|l| UnicodeWidthStr::width(l.as_str())).max().unwrap_or(0) as u16;
    let h = lines.len() as u16;
    Frame { lines, w, h }
}

fn parse_clip(text: &str) -> Clip {
    let mut fps = 2u8;
    let mut loop_mode = LoopMode::Loop;
    let mut frames = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(opts) = line.strip_prefix('#') {
            for tok in opts.split_whitespace() {
                if let Some(v) = tok.strip_prefix("fps=") {
                    if let Ok(n) = v.parse::<u8>() {
                        fps = n.max(1);
                    }
                } else if let Some(v) = tok.strip_prefix("loop=") {
                    loop_mode = match v {
                        "once" => LoopMode::Once,
                        "pingpong" => LoopMode::PingPong,
                        _ => LoopMode::Loop,
                    };
                }
            }
            continue;
        }
        if line.trim_end() == "---" {
            frames.push(make_frame(std::mem::take(&mut cur)));
            continue;
        }
        cur.push(line.to_string());
    }
    if !cur.is_empty() || frames.is_empty() {
        frames.push(make_frame(cur));
    }
    Clip { frames, fps, loop_mode }
}

fn load_sprites() -> HashMap<String, Sprite> {
    let mut map = HashMap::new();
    for dir in ART.dirs() {
        let Some(name) = dir.path().file_name().and_then(|n| n.to_str()) else { continue };
        let mut clips = HashMap::new();
        for kind in ClipKind::ALL {
            if let Some(file) = ART.get_file(format!("{name}/{}.txt", kind.file())) {
                if let Some(text) = file.contents_utf8() {
                    clips.insert(kind, parse_clip(text));
                }
            }
        }
        if clips.contains_key(&ClipKind::Idle) {
            map.insert(name.to_string(), Sprite { clips });
        }
    }
    map
}

pub fn sprite(art_dir: &str) -> &'static Sprite {
    static SPRITES: OnceLock<HashMap<String, Sprite>> = OnceLock::new();
    SPRITES
        .get_or_init(load_sprites)
        .get(art_dir)
        .unwrap_or_else(|| panic!("no art for stage '{art_dir}'"))
}

/// The frame to draw for `clip` at animation tick `tick` (the TUI's ~10fps frame counter).
pub fn frame_at(clip: &Clip, tick: u64) -> &Frame {
    let n = clip.frames.len();
    if n <= 1 {
        return &clip.frames[0];
    }
    let ticks_per_frame = (10 / clip.fps.max(1)).max(1) as u64;
    let step = (tick / ticks_per_frame) as usize;
    let idx = match clip.loop_mode {
        LoopMode::Loop => step % n,
        LoopMode::Once => step.min(n - 1),
        LoopMode::PingPong => {
            let period = 2 * (n - 1);
            let p = step % period;
            if p < n { p } else { period - p }
        }
    };
    &clip.frames[idx]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BOX_H, BOX_W};

    fn all_sprites() -> &'static HashMap<String, Sprite> {
        static S: OnceLock<HashMap<String, Sprite>> = OnceLock::new();
        S.get_or_init(load_sprites)
    }

    #[test]
    fn parse_frames_and_options() {
        let c = parse_clip("# fps=4 loop=pingpong\nAB\nAB\n---\nCD\nCD");
        assert_eq!(c.frames.len(), 2);
        assert_eq!(c.fps, 4);
        assert_eq!(c.loop_mode, LoopMode::PingPong);
        assert_eq!(c.frames[0].w, 2);
        assert_eq!(c.frames[0].h, 2);
    }

    #[test]
    fn every_stage_has_idle_and_loads() {
        let s = all_sprites();
        for dir in ["egg", "blob", "sprout", "critter", "beast", "elder"] {
            let sp = s.get(dir).unwrap_or_else(|| panic!("missing sprite {dir}"));
            assert!(sp.has(ClipKind::Idle), "{dir} has no idle");
        }
    }

    #[test]
    fn frames_uniform_within_clip_and_fit_the_box() {
        for (name, sp) in all_sprites() {
            for kind in ClipKind::ALL {
                if !sp.has(kind) {
                    continue;
                }
                let clip = sp.clip(kind);
                let (w0, h0) = (clip.frames[0].w, clip.frames[0].h);
                for f in &clip.frames {
                    assert_eq!((f.w, f.h), (w0, h0), "{name}/{} reflows", kind.file());
                    assert!(f.w <= BOX_W, "{name}/{} too wide ({} > {BOX_W})", kind.file(), f.w);
                    assert!(f.h < BOX_H, "{name}/{} too tall ({} >= {BOX_H})", kind.file(), f.h);
                    // every glyph must be display-width 1 or the art misaligns
                    for line in &f.lines {
                        assert_eq!(
                            UnicodeWidthStr::width(line.as_str()),
                            line.chars().count(),
                            "{name}/{} has a wide glyph in {line:?}",
                            kind.file()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn frame_selection_modes() {
        let mk = |n: usize, lm: LoopMode| Clip {
            frames: (0..n).map(|_| make_frame(vec!["x".into()])).collect(),
            fps: 10,
            loop_mode: lm,
        };
        let loop3 = mk(3, LoopMode::Loop); // ticks_per_frame = 1
        assert!(std::ptr::eq(frame_at(&loop3, 0), &loop3.frames[0]));
        assert!(std::ptr::eq(frame_at(&loop3, 4), &loop3.frames[1])); // 4 % 3
        let once3 = mk(3, LoopMode::Once);
        assert!(std::ptr::eq(frame_at(&once3, 99), &once3.frames[2])); // clamps to last
    }
}
