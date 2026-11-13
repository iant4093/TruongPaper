//! `truongpaper-renderer`: Multi-monitor `wgpu` 23.0 compositor, Kawase blur, Liquid Glass
//! transitions, Wallpaper Engine `Scene` renderer, shader preprocessor (`naga`), and particle simulator.
//! Complete specification: `docs/01-SYSTEM-ARCHITECTURE.md` (Section 3) &
//! `docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` (Sections 4, 5, 7.2).

pub mod compositor;
pub mod diagnostics;
pub mod effect_chain;
pub mod json;
pub mod particles;
pub mod puppet;
pub mod scene_assets;
pub mod scene_graph;
pub mod scene_math;
pub mod scene_renderer;
pub mod shader_preprocessor;
pub mod text;
pub mod timeline;
pub mod video_runtime;
pub mod web_runtime;

pub use compositor::{
    clamp_transition_duration, downscale_for_display, fitted_layer_sample, load_rgba_image,
    next_view_id, photo_display_target, transition_progress, upload_rgba_texture, BlurUniforms,
    CompositorPipeline, CompositorUniforms, FitMode, FittedSample, RgbaImageData, TransitionMode,
    DEFAULT_TRANSITION_DURATION_SECS, MAX_PHOTO_TEXTURE_DIMENSION, MAX_TRANSITION_DURATION_SECS,
    MIN_TRANSITION_DURATION_SECS, PHOTO_DISPLAY_HEADROOM,
};
pub use diagnostics::{DiagnosticsLog, MAX_DIAGNOSTICS};
pub use effect_chain::{
    fbo_size, fbo_texture_format, BlendMode, BloomPassPlan, BloomPlan, BloomStage, EffectChainPlan,
    EffectPassPlan, EffectStep, FboDesc, RenderTargetId, TextureBinding, RT_4_FRAME_BUFFER,
    RT_8_FRAME_BUFFER, RT_BLOOM, RT_FULL_FRAME_BUFFER, RT_SCENE,
};
pub use particles::{
    parse_vec3_str, ParticleControlPointSpec, ParticleEmitterSpec, ParticleFile, ParticleOverrides,
    ParticleRendererSpec, ParticleSpecError, ParticleSystemInstance, RopeParticleVertex,
    SingleParticle, SpriteParticleVertex,
};
pub use puppet::{
    parse_mdl_skeleton, puppet_combos, PuppetAnimation, PuppetAnimationMode, PuppetAnimator,
    PuppetBone, PuppetDeformer, PuppetError, PuppetFrame, PuppetSkeleton,
};
pub use scene_assets::{
    EffectBindSlot, EffectFboSpec, EffectFile, EffectPassSpec, MaterialFile, MaterialPassSpec,
    ModelJsonFile, SceneAssetError,
};
pub use scene_graph::{
    camera_shake_offset, CameraRuntime, CameraState, PropertyScriptSlot, SceneGraph, SceneLayer,
    SceneLayerKind, SceneLoadError, SceneProjection, ScriptProperty,
};
pub use scene_math::{
    alignment_offset, build_pass_matrices, layer_model_matrix, layer_pivot_matrix,
    parallax_eye_offset, scene_orthographic_projection, scene_view_matrix, LayerTransform,
    PassMatrices,
};
pub use scene_renderer::{
    composition_capture_matrix, layer_local_projection, layer_quad_vertices, pack_uniform_block,
    texture_bind_group_layout_entries, FrameUniformInputs, LayerVertex, SceneRenderer,
    UniformValues,
};
pub use shader_preprocessor::{
    apply_fragment_texcoord_compatibility, apply_linked_varying_compatibility, builtin_uniform,
    compile_we_shader_pair, naga_module_to_wgsl, parse_uniform_comment_metadata,
    preprocess_we_shader, translate_we_shader_pair, BuiltinUniform, CompiledWeShaderPair,
    ReflectedUniformField, TranslatedWeShaderPair, UniformCommentMetadata, WeShaderStage,
    WE_BUILTIN_UNIFORMS,
};
pub use text::{HAlign, TextStyle, VAlign};
pub use video_runtime::{
    clamp_playback_speed, clamp_volume, embedded_mp4_cache_path, extract_embedded_tex_mp4,
    is_mp4_container, tightly_packed_rows, VideoFrame, VideoFrameSource, VideoLoopClock,
    VideoPixelFormat, VideoPlaybackSettings, VideoTexture, MAX_PLAYBACK_SPEED, MIN_PLAYBACK_SPEED,
};
pub use web_runtime::{
    audio_dispatch_js, cursor_event_js, encode_uri_component, pack_web_audio_frame,
    properties_dispatch_js, video_adjustments_js, video_host_url, video_set_paused_js,
    we_property_payload, web_wallpaper_url, wpe_origin, WallpaperRuntimeKind, WebCursorEventKind,
    WEB_AUDIO_FRAME_LEN,
};

pub const KAWASE_BLUR_WGSL: &str = include_str!("shaders/kawase_blur.wgsl");
pub const COMPOSITOR_WGSL: &str = include_str!("shaders/compositor.wgsl");
pub const BLOOM_WGSL: &str = include_str!("shaders/bloom.wgsl");
