//! Keyframe property animations (the editor's "timeline" animations).
//!
//! A scene property can carry an `"animation"` next to its `"value"`:
//!
//! ```json
//! "alpha": { "value": 1.0, "animation": {
//!     "c0": [ { "frame": 0, "value": 0, "back": {..}, "front": {..} }, ... ],
//!     "options": { "fps": 30, "length": 150, "mode": "single", "startpaused": true } } }
//! ```
//!
//! `c0`, `c1`, `c2` are the x/y/z channels (scalars use `c0` only). While an animation exists the
//! property's live value is the animation sample, not `value` (Wallpaper Engine shows `value` only
//! in the editor). Semantics implemented here:
//! - `mode`: `"loop"` wraps, `"mirror"` ping-pongs, `"single"` plays once and holds the last key.
//! - `startpaused`: the animation waits for a script `play()`, so it holds frame 0.
//! - Between keys the value follows a cubic Bezier through each key's `front` / `back` handle
//!   (`y` offsets in value units); disabled handles are flat.
use crate::json::Value;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Key {
    frame: f32,
    value: f32,
    front: f32,
    back: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineMode {
    Loop,
    Mirror,
    Single,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    channels: Vec<Vec<Key>>,
    pub fps: f32,
    /// Length in frames.
    pub length: f32,
    pub mode: TimelineMode,
    pub start_paused: bool,
}

fn handle_y(v: Option<&Value>) -> f32 {
    match v {
        Some(h) if h.get("enabled").and_then(Value::as_bool) != Some(false) => {
            h.get("y").and_then(Value::as_f64).unwrap_or(0.0) as f32
        }
        _ => 0.0,
    }
}

impl Timeline {
    /// Parses the `animation` object of a bound property; `None` when it has no usable keys.
    pub fn parse(anim: &Value) -> Option<Self> {
        let mut channels = Vec::new();
        for c in ["c0", "c1", "c2", "c3"] {
            let Some(keys) = anim.get(c).and_then(Value::as_array) else {
                break;
            };
            let mut keys: Vec<Key> = keys
                .iter()
                .filter_map(|k| {
                    Some(Key {
                        frame: k.get("frame")?.as_f64()? as f32,
                        value: k.get("value")?.as_f64()? as f32,
                        front: handle_y(k.get("front")),
                        back: handle_y(k.get("back")),
                    })
                })
                .filter(|k| k.frame.is_finite() && k.value.is_finite())
                .collect();
            if keys.is_empty() {
                break;
            }
            keys.sort_by(|a, b| a.frame.total_cmp(&b.frame));
            channels.push(keys);
        }
        if channels.is_empty() {
            return None;
        }
        let opts = anim.get("options");
        let num = |k: &str| {
            opts.and_then(|o| o.get(k))
                .and_then(Value::as_f64)
                .map(|v| v as f32)
                .filter(|v| v.is_finite() && *v > 0.0)
        };
        let last_key = channels
            .iter()
            .filter_map(|c| c.last())
            .map(|k| k.frame)
            .fold(0.0f32, f32::max);
        let mode = match opts.and_then(|o| o.get("mode")).and_then(Value::as_str) {
            Some("single") => TimelineMode::Single,
            Some("mirror") => TimelineMode::Mirror,
            _ => TimelineMode::Loop,
        };
        Some(Self {
            channels,
            fps: num("fps").unwrap_or(30.0),
            length: num("length").unwrap_or(last_key.max(1.0)),
            mode,
            start_paused: opts
                .and_then(|o| o.get("startpaused"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// Timeline frame shown `seconds` after the scene started.
    pub fn frame_at(&self, seconds: f32) -> f32 {
        if self.start_paused || !seconds.is_finite() {
            return 0.0;
        }
        let f = seconds.max(0.0) * self.fps;
        let len = self.length.max(f32::EPSILON);
        match self.mode {
            TimelineMode::Single => f.min(len),
            TimelineMode::Loop => f.rem_euclid(len),
            TimelineMode::Mirror => {
                let p = f.rem_euclid(2.0 * len);
                if p > len {
                    2.0 * len - p
                } else {
                    p
                }
            }
        }
    }

    /// Channel values (`[x]`, `[x, y]` or `[x, y, z]`) `seconds` after the scene started.
    pub fn sample(&self, seconds: f32) -> Vec<f32> {
        let frame = self.frame_at(seconds);
        self.channels
            .iter()
            .map(|keys| sample_channel(keys, frame))
            .collect()
    }
}

fn sample_channel(keys: &[Key], frame: f32) -> f32 {
    let first = keys[0];
    if frame <= first.frame {
        return first.value;
    }
    for pair in keys.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if frame <= b.frame {
            let span = b.frame - a.frame;
            if span <= f32::EPSILON {
                return b.value;
            }
            let t = (frame - a.frame) / span;
            let u = 1.0 - t;
            let (p0, p1, p2, p3) = (a.value, a.value + a.front, b.value + b.back, b.value);
            return u * u * u * p0 + 3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t * p3;
        }
    }
    keys[keys.len() - 1].value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tl(json: &str) -> Timeline {
        Timeline::parse(&Value::parse(json).unwrap()).unwrap()
    }

    const ZOOM: &str = r#"{ "c0": [
        { "frame": 0, "value": 3 }, { "frame": 300, "value": 3 }, { "frame": 450, "value": 1 } ],
        "options": { "fps": 30, "length": 450, "mode": "single" } }"#;

    #[test]
    fn single_mode_plays_once_and_holds_the_last_key() {
        let t = tl(ZOOM);
        assert_eq!(t.sample(0.0), vec![3.0]);
        assert_eq!(t.sample(10.0), vec![3.0]);
        let mid = t.sample(12.5)[0];
        assert!(mid < 3.0 && mid > 1.0, "{mid}");
        assert_eq!(t.sample(15.0), vec![1.0]);
        assert_eq!(t.sample(600.0), vec![1.0]);
    }

    #[test]
    fn start_paused_holds_frame_zero() {
        let t = tl(
            r#"{ "c0": [ { "frame": 0, "value": 0 }, { "frame": 30, "value": 1 } ],
            "options": { "fps": 30, "length": 150, "mode": "single", "startpaused": true } }"#,
        );
        assert_eq!(t.sample(0.0), vec![0.0]);
        assert_eq!(t.sample(100.0), vec![0.0]);
    }

    #[test]
    fn loop_and_mirror_wrap() {
        let keys = r#""c0": [ { "frame": 0, "value": 0 }, { "frame": 10, "value": 10 } ]"#;
        let lp = tl(&format!(
            r#"{{ {keys}, "options": {{ "fps": 10, "length": 10, "mode": "loop" }} }}"#
        ));
        assert!((lp.sample(1.5)[0] - 5.0).abs() < 1e-3);
        let mi = tl(&format!(
            r#"{{ {keys}, "options": {{ "fps": 10, "length": 10, "mode": "mirror" }} }}"#
        ));
        assert!((mi.sample(1.5)[0] - 5.0).abs() < 1e-3);
        // Frame 18 mirrors to frame 2; flat handles ease: 3*0.8*0.2^2*10 + 0.2^3*10 = 1.04.
        assert!((mi.sample(1.8)[0] - 1.04).abs() < 1e-3);
    }

    #[test]
    fn vector_channels_and_bezier_handles() {
        let t = tl(
            r#"{ "c0": [ { "frame": 0, "value": 0 }, { "frame": 10, "value": 1 } ],
            "c1": [ { "frame": 0, "value": 5, "front": { "enabled": true, "x": 1, "y": 3 } },
                    { "frame": 10, "value": 5 } ],
            "options": { "fps": 10, "length": 10, "mode": "single" } }"#,
        );
        let v = t.sample(0.5);
        assert_eq!(v.len(), 2);
        assert!((v[0] - 0.5).abs() < 1e-3);
        assert!(v[1] > 5.0, "front handle lifts the curve: {}", v[1]);
    }

    #[test]
    fn no_keys_is_none() {
        assert!(Timeline::parse(&Value::parse(r#"{ "options": {} }"#).unwrap()).is_none());
    }
}
