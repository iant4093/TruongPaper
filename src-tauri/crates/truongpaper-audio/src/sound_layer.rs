//! `scene.json` sound layer playback via `kira` 0.9 (spec section 7.3).
//!
//! Sound files are resolved through the Wallpaper Engine VFS (`WallpaperVfs`), decoded with
//! `StaticSoundData::from_cursor` and played on an `AudioManager`. Planning (mode parsing, file
//! selection, VFS reads, volume math) is pure and separate from playback so it can be tested
//! without an audio device; the player is generic over the kira backend for the same reason.

use kira::manager::backend::{Backend, DefaultBackend};
use kira::manager::{AudioManager, AudioManagerSettings};
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::tween::Tween;
use rand::seq::SliceRandom;
use rand::Rng;
use std::io::Cursor;
use truongpaper_we_formats::scene_schema::{
    resolve_bound_bool, resolve_bound_value, SceneObject, UserPropertyMap,
};
use truongpaper_we_formats::vfs::WallpaperVfs;

/// `scene.json` sound object `playbackmode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundPlaybackMode {
    /// Loop the first sound forever (Wallpaper Engine default).
    Loop,
    /// Play one randomly chosen sound from the list.
    Random,
    /// Play the first sound once.
    Single,
}

impl SoundPlaybackMode {
    /// Parses `playbackmode` case-insensitively. Missing or unknown values fall back to `Loop`.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("random") => Self::Random,
            Some("single") => Self::Single,
            _ => Self::Loop,
        }
    }

    pub fn is_loop(self) -> bool {
        self == Self::Loop
    }
}

/// Final amplitude for a layer: `layer * master`, clamped to `[0, 1]` (NaN becomes silence).
pub fn effective_volume(layer_volume: f32, master_volume: f32) -> f64 {
    let v = layer_volume * master_volume;
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 1.0) as f64
    }
}

/// Picks the sound file to play: a random entry for `Random`, otherwise the first entry.
pub fn select_sound_path<'a, R: Rng + ?Sized>(
    paths: &'a [String],
    mode: SoundPlaybackMode,
    rng: &mut R,
) -> Option<&'a str> {
    match mode {
        SoundPlaybackMode::Random => paths.choose(rng).map(String::as_str),
        SoundPlaybackMode::Loop | SoundPlaybackMode::Single => paths.first().map(String::as_str),
    }
}

/// A resolved, ready-to-decode sound layer.
#[derive(Debug, Clone)]
pub struct SoundLayerPlan {
    pub object_name: String,
    /// VFS path of the selected file.
    pub path: String,
    /// Encoded file bytes read through the VFS.
    pub bytes: Vec<u8>,
    pub mode: SoundPlaybackMode,
    /// Effective amplitude in `[0, 1]`.
    pub volume: f64,
}

/// Resolves every visible sound object in `objects` to a [`SoundLayerPlan`].
///
/// `visible` and `volume` are resolved against `user_props`. Objects without sounds, hidden
/// objects and files missing from every VFS layer are skipped.
pub fn plan_scene_sounds<R: Rng + ?Sized>(
    vfs: &WallpaperVfs,
    objects: &[SceneObject],
    user_props: &UserPropertyMap,
    master_volume: f32,
    rng: &mut R,
) -> Vec<SoundLayerPlan> {
    let mut plans = Vec::new();
    for obj in objects {
        let Some(sound_paths) = &obj.sound else {
            continue;
        };
        if !resolve_bound_bool(&obj.visible, user_props) {
            continue;
        }
        let mode = SoundPlaybackMode::parse(obj.playbackmode.as_deref());
        let Some(rel) = select_sound_path(sound_paths, mode, rng) else {
            continue;
        };
        let bytes = match vfs.read_bytes(rel) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("Sound layer '{}': cannot read {rel}: {e}", obj.name);
                continue;
            }
        };
        let layer_vol = obj
            .volume
            .as_ref()
            .map(|v| resolve_bound_value(v, user_props))
            .unwrap_or(1.0);
        plans.push(SoundLayerPlan {
            object_name: obj.name.clone(),
            path: rel.to_string(),
            bytes,
            mode,
            volume: effective_volume(layer_vol, master_volume),
        });
    }
    plans
}

/// Plays a scene's sound layers. Defaults to the cpal output backend; any kira [`Backend`]
/// (e.g. `MockBackend`) can be supplied through [`SceneSoundPlayer::from_manager`].
pub struct SceneSoundPlayer<B: Backend = DefaultBackend> {
    manager: AudioManager<B>,
    active_handles: Vec<StaticSoundHandle>,
}

impl SceneSoundPlayer<DefaultBackend> {
    /// Opens the default audio output device.
    pub fn new() -> anyhow::Result<Self> {
        let manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())?;
        Ok(Self::from_manager(manager))
    }
}

impl<B: Backend> SceneSoundPlayer<B> {
    pub fn from_manager(manager: AudioManager<B>) -> Self {
        Self {
            manager,
            active_handles: Vec::new(),
        }
    }

    /// Starts all sound layers using their scene default values (no user-property overrides).
    pub fn start_scene_sounds(
        &mut self,
        vfs: &WallpaperVfs,
        objects: &[SceneObject],
        master_volume: f32,
    ) -> usize {
        self.start_scene_sounds_with(
            vfs,
            objects,
            &UserPropertyMap::new(),
            master_volume,
            &mut rand::thread_rng(),
        )
    }

    /// Starts all sound layers with user-property overrides and an explicit RNG.
    /// Returns the number of sounds that started.
    pub fn start_scene_sounds_with<R: Rng + ?Sized>(
        &mut self,
        vfs: &WallpaperVfs,
        objects: &[SceneObject],
        user_props: &UserPropertyMap,
        master_volume: f32,
        rng: &mut R,
    ) -> usize {
        let plans = plan_scene_sounds(vfs, objects, user_props, master_volume, rng);
        self.play_plans(plans)
    }

    /// Decodes and plays each plan. Returns the number of sounds that started.
    pub fn play_plans(&mut self, plans: Vec<SoundLayerPlan>) -> usize {
        let mut started = 0;
        for plan in plans {
            let mut sound_data = match StaticSoundData::from_cursor(Cursor::new(plan.bytes)) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(
                        "Sound layer '{}': cannot decode {}: {e}",
                        plan.object_name,
                        plan.path
                    );
                    continue;
                }
            };
            if plan.mode.is_loop() {
                sound_data = sound_data.loop_region(..);
            }
            sound_data = sound_data.volume(plan.volume);
            match self.manager.play(sound_data) {
                Ok(handle) => {
                    self.active_handles.push(handle);
                    started += 1;
                }
                Err(e) => tracing::warn!("Sound layer '{}': play failed: {e}", plan.object_name),
            }
        }
        started
    }

    pub fn set_paused(&mut self, paused: bool) {
        for handle in &mut self.active_handles {
            if paused {
                handle.pause(Tween::default());
            } else {
                handle.resume(Tween::default());
            }
        }
    }

    /// Stops and releases every active sound.
    pub fn stop_all(&mut self) {
        for handle in &mut self.active_handles {
            handle.stop(Tween::default());
        }
        self.active_handles.clear();
    }

    pub fn active_sound_count(&self) -> usize {
        self.active_handles.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kira::manager::backend::mock::MockBackend;
    use kira::sound::PlaybackState;
    use rand::rngs::StdRng;
    use rand::SeedableRng;
    use std::fs;
    use std::path::PathBuf;
    use truongpaper_we_formats::scene_schema::{parse_scene_json, parse_user_properties};
    use truongpaper_we_formats::vfs::WeVirtualFileSystem;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "truongpaper-sound-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Minimal 16-bit mono PCM WAV.
    fn wav_bytes(frames: usize) -> Vec<u8> {
        let sample_rate = 8_000u32;
        let data_len = (frames * 2) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend((36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes()); // PCM
        b.extend(1u16.to_le_bytes()); // mono
        b.extend(sample_rate.to_le_bytes());
        b.extend((sample_rate * 2).to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend(data_len.to_le_bytes());
        for i in 0..frames {
            b.extend(((i as i16 % 64) * 256).to_le_bytes());
        }
        b
    }

    const SCENE: &str = r#"{
        "general": {},
        "objects": [
            { "name": "Ambience", "sound": ["sounds/a.wav"], "volume": { "user": "vol", "value": 0.5 } },
            { "name": "Rain", "sound": ["sounds/r1.wav", "sounds/r2.wav", "sounds/r3.wav"], "playbackmode": "random" },
            { "name": "Bell", "sound": ["sounds/missing.wav"], "playbackmode": "single" },
            { "name": "Hidden", "sound": ["sounds/a.wav"], "visible": { "user": "show_hidden", "value": false } },
            { "name": "Image", "image": "models/bg.json" },
            { "name": "Empty", "sound": [] }
        ]
    }"#;

    fn mount_sounds(tmp: &TempDir) -> WallpaperVfs {
        let sounds = tmp.0.join("wall").join("sounds");
        fs::create_dir_all(&sounds).unwrap();
        for name in ["a.wav", "r1.wav", "r2.wav", "r3.wav"] {
            fs::write(sounds.join(name), wav_bytes(64)).unwrap();
        }
        WeVirtualFileSystem::mount(tmp.0.join("wall"), tmp.0.join("core"))
    }

    #[test]
    fn playback_mode_parsing() {
        assert_eq!(SoundPlaybackMode::parse(None), SoundPlaybackMode::Loop);
        assert_eq!(
            SoundPlaybackMode::parse(Some("loop")),
            SoundPlaybackMode::Loop
        );
        assert_eq!(
            SoundPlaybackMode::parse(Some("random")),
            SoundPlaybackMode::Random
        );
        assert_eq!(
            SoundPlaybackMode::parse(Some(" Single ")),
            SoundPlaybackMode::Single
        );
        assert_eq!(
            SoundPlaybackMode::parse(Some("bogus")),
            SoundPlaybackMode::Loop
        );
        assert!(SoundPlaybackMode::Loop.is_loop());
        assert!(!SoundPlaybackMode::Random.is_loop());
        assert!(!SoundPlaybackMode::Single.is_loop());
    }

    #[test]
    fn volume_is_clamped() {
        assert_eq!(effective_volume(0.5, 0.5), 0.25);
        assert_eq!(effective_volume(2.0, 1.0), 1.0);
        assert_eq!(effective_volume(-1.0, 1.0), 0.0);
        assert_eq!(effective_volume(f32::NAN, 1.0), 0.0);
        assert_eq!(effective_volume(1.0, 0.0), 0.0);
    }

    #[test]
    fn random_selection_is_seeded_and_covers_all_entries() {
        let paths: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let pick = |seed| {
            let mut rng = StdRng::seed_from_u64(seed);
            (0..20)
                .map(|_| select_sound_path(&paths, SoundPlaybackMode::Random, &mut rng).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(pick(7), pick(7), "same seed must give the same sequence");
        let mut seen: Vec<&str> = pick(7);
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen, vec!["a", "b", "c"]);

        let mut rng = StdRng::seed_from_u64(1);
        for mode in [SoundPlaybackMode::Loop, SoundPlaybackMode::Single] {
            assert_eq!(select_sound_path(&paths, mode, &mut rng), Some("a"));
        }
        assert_eq!(
            select_sound_path(&[], SoundPlaybackMode::Random, &mut rng),
            None
        );
    }

    #[test]
    fn plans_resolve_files_through_vfs() {
        let tmp = TempDir::new("plan");
        let vfs = mount_sounds(&tmp);
        let scene = parse_scene_json(SCENE).unwrap();
        let mut rng = StdRng::seed_from_u64(42);

        let plans = plan_scene_sounds(&vfs, &scene.objects, &UserPropertyMap::new(), 0.8, &mut rng);
        let names: Vec<&str> = plans.iter().map(|p| p.object_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Ambience", "Rain"],
            "missing/hidden/empty layers are skipped"
        );

        let ambience = &plans[0];
        assert_eq!(ambience.path, "sounds/a.wav");
        assert_eq!(ambience.mode, SoundPlaybackMode::Loop);
        assert_eq!(ambience.bytes, wav_bytes(64));
        assert!((ambience.volume - 0.4).abs() < 1e-6);

        let rain = &plans[1];
        assert_eq!(rain.mode, SoundPlaybackMode::Random);
        assert!(["sounds/r1.wav", "sounds/r2.wav", "sounds/r3.wav"].contains(&rain.path.as_str()));
        assert_eq!(rain.volume, 0.8f32 as f64);

        let props = parse_user_properties(r#"{ "vol": 2.0, "show_hidden": true }"#).unwrap();
        let plans = plan_scene_sounds(&vfs, &scene.objects, &props, 1.0, &mut rng);
        let names: Vec<&str> = plans.iter().map(|p| p.object_name.as_str()).collect();
        assert_eq!(names, vec!["Ambience", "Rain", "Hidden"]);
        assert_eq!(plans[0].volume, 1.0, "user override is clamped");
    }

    #[test]
    fn plans_reject_path_traversal() {
        let tmp = TempDir::new("traversal");
        let vfs = mount_sounds(&tmp);
        fs::write(tmp.0.join("secret.wav"), wav_bytes(8)).unwrap();
        let scene = parse_scene_json(
            r#"{ "general": {}, "objects": [ { "name": "x", "sound": ["../secret.wav"] } ] }"#,
        )
        .unwrap();
        let mut rng = StdRng::seed_from_u64(0);
        assert!(
            plan_scene_sounds(&vfs, &scene.objects, &UserPropertyMap::new(), 1.0, &mut rng)
                .is_empty()
        );
    }

    #[test]
    fn mock_backend_plays_pauses_and_stops() {
        let tmp = TempDir::new("mock");
        let vfs = mount_sounds(&tmp);
        let scene = parse_scene_json(SCENE).unwrap();
        let manager = AudioManager::<MockBackend>::new(AudioManagerSettings::default()).unwrap();
        let mut player = SceneSoundPlayer::from_manager(manager);
        let mut rng = StdRng::seed_from_u64(3);

        let started = player.start_scene_sounds_with(
            &vfs,
            &scene.objects,
            &UserPropertyMap::new(),
            1.0,
            &mut rng,
        );
        assert_eq!(started, 2);
        assert_eq!(player.active_sound_count(), 2);
        assert!(player
            .active_handles
            .iter()
            .all(|h| h.state() == PlaybackState::Playing));

        player.set_paused(true);
        player.set_paused(false);

        let bad = SoundLayerPlan {
            object_name: "bad".into(),
            path: "x.wav".into(),
            bytes: b"not audio".to_vec(),
            mode: SoundPlaybackMode::Single,
            volume: 1.0,
        };
        assert_eq!(player.play_plans(vec![bad]), 0);

        player.stop_all();
        assert_eq!(player.active_sound_count(), 0);
    }
}
