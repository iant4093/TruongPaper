mod common;

use std::collections::HashMap;

use common::TempDir;
use glam::{Mat4, Vec2, Vec3, Vec4};
use truongpaper_renderer::json::Value;
use truongpaper_renderer::scene_renderer::{
    downsample_spectrum, json_value_floats, layer_vertex_attributes, map_material_constants,
    plan_video_upload, reflect_texture_slots, vertex_attribute_locations,
    vertex_wants_pixel_positions, video_frame_to_rgba, VideoUpload, EMBEDDED_VIDEO_FORMAT,
    QUAD_INDICES, SCENE_COMPOSITE_WGSL, SCENE_PARTICLE_WGSL,
};
use truongpaper_renderer::{
    layer_quad_vertices, pack_uniform_block, parse_uniform_comment_metadata, preprocess_we_shader,
    texture_bind_group_layout_entries, translate_we_shader_pair, ReflectedUniformField,
    UniformValues, VideoFrame, VideoPixelFormat, WeShaderStage,
};
use truongpaper_we_formats::WeVirtualFileSystem;

const VERT: &str = r#"
uniform mat4 g_ModelViewProjectionMatrix;
uniform float g_Time;
attribute vec3 a_Position;
attribute vec2 a_TexCoord;
varying vec2 v_TexCoord;
void main() {
    gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);
    v_TexCoord = a_TexCoord;
}
"#;

const FRAG: &str = r#"
uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer"}
uniform sampler2D g_Texture2; // {"combo":"MASK","default":"util/white"}
uniform float g_Alpha;
uniform vec3 g_Color;
uniform vec2 g_PointerPosition;
uniform vec4 g_Texture0Resolution;
uniform float g_Speed; // {"material":"speed","label":"ui_speed","default":2.0}
uniform vec3 u_Tint; // {"material":"tint","default":"0.5 0.25 1"}
uniform int u_Mode; // {"material":"mode","default":3}
uniform float g_AudioSpectrum16Left[16];
varying vec2 v_TexCoord;
void main() {
    vec4 c = texSample2D(g_Texture0, v_TexCoord) * texSample2D(g_Texture2, v_TexCoord);
    float extra = g_AudioSpectrum16Left[1] + g_Speed * g_Time + float(u_Mode) + g_PointerPosition.x + g_Texture0Resolution.x;
    gl_FragColor = vec4(c.rgb * g_Color * u_Tint, c.a * g_Alpha + extra * 0.0);
}
"#;

fn f32_at(buf: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(buf[at..at + 4].try_into().unwrap())
}

fn i32_at(buf: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(buf[at..at + 4].try_into().unwrap())
}

fn field<'a>(fields: &'a [ReflectedUniformField], name: &str) -> &'a ReflectedUniformField {
    fields
        .iter()
        .find(|f| f.name.split('[').next() == Some(name))
        .unwrap_or_else(|| panic!("no uniform {name}"))
}

#[test]
fn pack_uniform_block_writes_builtins_and_constants_at_reflected_offsets() {
    let t = TempDir::new("sr-pack");
    t.write("shaders/test.vert", VERT);
    t.write("shaders/test.frag", FRAG);
    let vfs = WeVirtualFileSystem::mount(t.path().to_path_buf(), t.path().join("core"));
    let combos = HashMap::new();
    let bound = [true, false, true, false, false, false, false, false];
    let vert = preprocess_we_shader(
        &vfs,
        "shaders/test.vert",
        WeShaderStage::Vertex,
        &combos,
        None,
        &bound,
    )
    .unwrap();
    let frag = preprocess_we_shader(
        &vfs,
        "shaders/test.frag",
        WeShaderStage::Fragment,
        &combos,
        None,
        &bound,
    )
    .unwrap();
    let tr = translate_we_shader_pair("test", &vert, &frag).expect("translates");

    // Texture slots and attributes as the renderer reflects them.
    assert_eq!(reflect_texture_slots(&[&vert, &frag]), vec![0, 2]);
    let attrs = vertex_attribute_locations(&vert);
    assert_eq!(
        attrs
            .iter()
            .map(|a| (a.location, a.name.as_str()))
            .collect::<Vec<_>>(),
        [(0, "a_Position"), (1, "a_TexCoord")]
    );
    let va = layer_vertex_attributes(&attrs);
    assert_eq!(va.len(), 2);
    assert_eq!(
        (va[0].offset, va[0].format),
        (0, wgpu::VertexFormat::Float32x3)
    );
    assert_eq!(
        (va[1].offset, va[1].format),
        (12, wgpu::VertexFormat::Float32x2)
    );

    let metadata = parse_uniform_comment_metadata(&format!("{vert}\n{frag}"));
    let material = HashMap::from([("speed".to_string(), Value::Number(4.0))]);
    let constants = map_material_constants(&metadata, &material);
    assert_eq!(constants["g_Speed"], vec![4.0]);
    assert_eq!(constants["u_Tint"], vec![0.5, 0.25, 1.0]);
    assert_eq!(constants["u_Mode"], vec![3.0]);
    assert!(!constants.contains_key("g_Texture0"));

    let mvp = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)) * Mat4::from_scale(Vec3::splat(0.5));
    let mut audio = [0.0f32; 64];
    for (i, a) in audio.iter_mut().enumerate() {
        *a = i as f32;
    }
    let mut values = UniformValues {
        time: 1.5,
        alpha: 0.25,
        color: Vec3::new(0.1, 0.2, 0.3),
        pointer: Vec2::new(0.75, 0.125),
        audio_left: audio,
        constants,
        ..UniformValues::default()
    };
    values.matrices.model_view_projection = mvp;
    values.texture_resolutions[0] = Vec4::new(512.0, 256.0, 300.0, 200.0);

    let buf = pack_uniform_block(&tr.uniform_fields, tr.uniform_buffer_size, &values);
    assert_eq!(buf.len(), tr.uniform_buffer_size);
    let f = |n| field(&tr.uniform_fields, n).byte_offset;

    let m = f("g_ModelViewProjectionMatrix");
    let cols = mvp.to_cols_array();
    for (i, c) in cols.iter().enumerate() {
        assert_eq!(f32_at(&buf, m + 4 * i), *c);
    }
    assert_eq!(f32_at(&buf, f("g_Time")), 1.5);
    assert_eq!(f32_at(&buf, f("g_Alpha")), 0.25);
    let c = f("g_Color");
    assert_eq!(
        [f32_at(&buf, c), f32_at(&buf, c + 4), f32_at(&buf, c + 8)],
        [0.1, 0.2, 0.3]
    );
    let p = f("g_PointerPosition");
    assert_eq!([f32_at(&buf, p), f32_at(&buf, p + 4)], [0.75, 0.125]);
    let r = f("g_Texture0Resolution");
    assert_eq!(
        [
            f32_at(&buf, r),
            f32_at(&buf, r + 4),
            f32_at(&buf, r + 8),
            f32_at(&buf, r + 12)
        ],
        [512.0, 256.0, 300.0, 200.0]
    );
    assert_eq!(f32_at(&buf, f("g_Speed")), 4.0);
    let tint = f("u_Tint");
    assert_eq!(
        [
            f32_at(&buf, tint),
            f32_at(&buf, tint + 4),
            f32_at(&buf, tint + 8)
        ],
        [0.5, 0.25, 1.0]
    );
    assert_eq!(i32_at(&buf, f("u_Mode")), 3);
    // float[16] uses a 16-byte element stride; band i = mean of bins 4i..4i+3.
    let a = f("g_AudioSpectrum16Left");
    assert_eq!(
        field(&tr.uniform_fields, "g_AudioSpectrum16Left").byte_size,
        256
    );
    for i in 0..16 {
        let expected = (4 * i) as f32 + 1.5;
        assert_eq!(f32_at(&buf, a + 16 * i), expected, "band {i}");
        assert_eq!(f32_at(&buf, a + 16 * i + 4), 0.0);
    }
}

#[test]
fn pack_uniform_block_handles_mat3_unknown_and_out_of_range_fields() {
    let fields = vec![
        ReflectedUniformField {
            name: "u_Basis".into(),
            glsl_type: "mat3".into(),
            byte_offset: 0,
            byte_size: 48,
        },
        ReflectedUniformField {
            name: "u_Unknown".into(),
            glsl_type: "vec4".into(),
            byte_offset: 48,
            byte_size: 16,
        },
        ReflectedUniformField {
            name: "g_Time".into(),
            glsl_type: "float".into(),
            byte_offset: 1000,
            byte_size: 4,
        },
    ];
    let mut values = UniformValues::default();
    values
        .constants
        .insert("u_Basis".into(), (1..=9).map(|n| n as f32).collect());
    let buf = pack_uniform_block(&fields, 64, &values);
    assert_eq!(buf.len(), 64);
    // Columns padded to vec4.
    assert_eq!(
        [
            f32_at(&buf, 0),
            f32_at(&buf, 4),
            f32_at(&buf, 8),
            f32_at(&buf, 12)
        ],
        [1.0, 2.0, 3.0, 0.0]
    );
    assert_eq!(f32_at(&buf, 16), 4.0);
    assert_eq!(f32_at(&buf, 32), 7.0);
    assert!(buf[48..64].iter().all(|b| *b == 0));
}

#[test]
fn layer_quad_vertices_apply_uv_scale() {
    let v = layer_quad_vertices(Vec2::new(200.0, 100.0), [0.5, 0.25]);
    assert_eq!(v[0].position, [-100.0, -50.0, 0.0]);
    assert_eq!(v[0].texcoord, [0.0, 0.25]);
    assert_eq!(v[1].position, [100.0, -50.0, 0.0]);
    assert_eq!(v[1].texcoord, [0.5, 0.25]);
    assert_eq!(v[2].position, [-100.0, 50.0, 0.0]);
    assert_eq!(v[2].texcoord, [0.0, 0.0]);
    assert_eq!(v[3].position, [100.0, 50.0, 0.0]);
    assert_eq!(v[3].texcoord, [0.5, 0.0]);
    assert_eq!(std::mem::size_of_val(&v), 80);
    assert_eq!(QUAD_INDICES, [0, 1, 2, 2, 1, 3]);
}

#[test]
fn texture_bind_group_layout_entries_use_n_and_n_plus_8() {
    let entries = texture_bind_group_layout_entries(&[0, 2]);
    let bindings: Vec<u32> = entries.iter().map(|e| e.binding).collect();
    assert_eq!(bindings, [0, 8, 2, 10]);
    assert!(matches!(
        entries[0].ty,
        wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false
        }
    ));
    assert!(matches!(
        entries[1].ty,
        wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
    ));
    assert!(entries
        .iter()
        .all(|e| e.visibility.contains(wgpu::ShaderStages::FRAGMENT)));
    assert!(texture_bind_group_layout_entries(&[]).is_empty());
}

#[test]
fn helper_conversions() {
    assert_eq!(json_value_floats(&Value::Number(2.0)), Some(vec![2.0]));
    assert_eq!(json_value_floats(&Value::Bool(true)), Some(vec![1.0]));
    assert_eq!(
        json_value_floats(&Value::String("1 2 3".into())),
        Some(vec![1.0, 2.0, 3.0])
    );
    assert_eq!(
        json_value_floats(&Value::parse(r#"{ "value": "4 5" }"#).unwrap()),
        Some(vec![4.0, 5.0])
    );
    assert_eq!(json_value_floats(&Value::Null), None);
    let mut s = [0.0f32; 64];
    s[0] = 2.0;
    s[1] = 4.0;
    assert_eq!(downsample_spectrum(&s, 32)[0], 3.0);
    assert_eq!(downsample_spectrum(&s, 16)[0], 1.5);
    assert_eq!(downsample_spectrum(&s, 64).len(), 64);
}

#[test]
fn built_in_scene_wgsl_validates_with_naga() {
    for (name, src) in [
        ("composite", SCENE_COMPOSITE_WGSL),
        ("particle", SCENE_PARTICLE_WGSL),
    ] {
        let module = naga::front::wgsl::parse_str(src)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let vs = module
            .entry_points
            .iter()
            .find(|e| e.name == "vs_main")
            .expect("vs_main");
        let expected_inputs = if name == "particle" { 5 } else { 2 };
        assert_eq!(vs.function.arguments.len(), 1);
        let ty = &module.types[vs.function.arguments[0].ty];
        match &ty.inner {
            naga::TypeInner::Struct { members, .. } => assert_eq!(members.len(), expected_inputs),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn video_frames_are_swizzled_and_scaled_into_the_placeholder_size() {
    // 2x1 BGRA frame with a padded stride (12 bytes per row).
    let frame = VideoFrame {
        width: 2,
        height: 1,
        stride: 12,
        format: VideoPixelFormat::Bgra8,
        data: vec![1, 2, 3, 255, 10, 20, 30, 128, 0, 0, 0, 0],
    };
    let same = video_frame_to_rgba(&frame, 2, 1).unwrap();
    assert_eq!(same, vec![3, 2, 1, 255, 30, 20, 10, 128]);

    // Upscaled 4x2 with nearest-neighbour sampling.
    let up = video_frame_to_rgba(&frame, 4, 2).unwrap();
    assert_eq!(up.len(), 4 * 2 * 4);
    assert_eq!(&up[0..4], &[3, 2, 1, 255]);
    assert_eq!(&up[4..8], &[3, 2, 1, 255]);
    assert_eq!(&up[8..12], &[30, 20, 10, 128]);
    assert_eq!(&up[16..20], &[3, 2, 1, 255]);

    // RGBA frames are copied as-is (downscaled to 1x1 here).
    let rgba = VideoFrame {
        format: VideoPixelFormat::Rgba8,
        ..frame.clone()
    };
    assert_eq!(
        video_frame_to_rgba(&rgba, 1, 1).unwrap(),
        vec![1, 2, 3, 255]
    );

    // Invalid frames and empty targets are rejected.
    let short = VideoFrame {
        data: vec![0; 4],
        ..frame.clone()
    };
    assert!(video_frame_to_rgba(&short, 2, 1).is_none());
    assert!(video_frame_to_rgba(&frame, 0, 1).is_none());
}

#[test]
fn bgra_frames_of_the_texture_size_upload_without_conversion() {
    assert_eq!(EMBEDDED_VIDEO_FORMAT, wgpu::TextureFormat::Bgra8Unorm);
    // 2x1 BGRA frame with a padded stride (12 bytes per row).
    let frame = VideoFrame {
        width: 2,
        height: 1,
        stride: 12,
        format: VideoPixelFormat::Bgra8,
        data: vec![1, 2, 3, 255, 10, 20, 30, 128, 0, 0, 0, 0],
    };
    assert_eq!(
        plan_video_upload(&frame, 2, 1),
        Some(VideoUpload::Direct {
            data: &frame.data,
            bytes_per_row: 12
        })
    );
    // A different size is scaled into tightly packed BGRA rows.
    match plan_video_upload(&frame, 1, 1) {
        Some(VideoUpload::Converted(bgra)) => assert_eq!(bgra, vec![1, 2, 3, 255]),
        other => panic!("expected a converted upload, got {other:?}"),
    }
    // RGBA frames are swizzled to BGRA even at the same size.
    let rgba = VideoFrame {
        format: VideoPixelFormat::Rgba8,
        ..frame.clone()
    };
    match plan_video_upload(&rgba, 2, 1) {
        Some(VideoUpload::Converted(bgra)) => {
            assert_eq!(bgra, vec![3, 2, 1, 255, 30, 20, 10, 128])
        }
        other => panic!("expected a converted upload, got {other:?}"),
    }
    let short = VideoFrame {
        data: vec![0; 4],
        ..frame.clone()
    };
    assert!(plan_video_upload(&short, 2, 1).is_none());
    assert!(plan_video_upload(&frame, 2, 0).is_none());
}

#[test]
fn scalar_constant_fills_every_component_of_a_vector_uniform() {
    // e.g. a user "blur" slider (1.5) bound to the vec2 `scale` of a blur pass.
    let meta = parse_uniform_comment_metadata(
        "uniform vec2 g_Scale; // {\"material\":\"scale\",\"default\":\"1 1\"}\n",
    );
    let constants = HashMap::from([("scale".to_string(), Value::Number(1.5))]);
    let mapped = map_material_constants(&meta, &constants);
    assert_eq!(mapped["g_Scale"], vec![1.5, 1.5]);
}

/// Regression (2887099508): a composition layer captures the screen region behind its own
/// (scaled) bounds, so its layer rect corners map to the FBO corners.
#[test]
fn composition_capture_maps_the_layer_rect_onto_its_fbo() {
    use truongpaper_renderer::scene_renderer::layer_local_projection;
    use truongpaper_renderer::{build_pass_matrices, composition_capture_matrix, LayerTransform};
    let scene = Vec2::new(200.0, 100.0);
    let transform = LayerTransform {
        origin: Vec3::new(150.0, 50.0, 0.0),
        scale: Vec3::new(2.0, 1.0, 0.0),
        size: Vec2::new(25.0, 100.0),
        ..LayerTransform::default()
    };
    let m = build_pass_matrices(scene, &transform, Vec2::ZERO);
    let local = layer_local_projection(transform.size);
    let cap = composition_capture_matrix(false, local, m.model_view_projection);
    // Layer spans x 125..175 of 200 -> clip x 0.25..0.75, full height.
    for (screen, fbo) in [
        (Vec4::new(0.25, -1.0, 0.0, 1.0), Vec2::new(-1.0, -1.0)),
        (Vec4::new(0.75, 1.0, 0.0, 1.0), Vec2::new(1.0, 1.0)),
        (Vec4::new(0.5, 0.0, 0.0, 1.0), Vec2::ZERO),
    ] {
        let p = cap * screen;
        let got = Vec2::new(p.x / p.w, p.y / p.w);
        assert!((got - fbo).length() < 1e-4, "{screen:?} -> {got:?}");
        assert_eq!(p.z, 0.0);
    }
    assert_eq!(
        composition_capture_matrix(true, local, m.model_view_projection),
        Mat4::IDENTITY
    );
}

#[test]
fn depth_parallax_builtins_are_finite_and_centred() {
    use truongpaper_renderer::scene_renderer::{builtin_uniform_floats, UniformValues};
    let v = UniformValues::default();
    // At rest the parallax position is the centre; an all-zero value made the stock
    // depthparallax vertex shader normalise a zero vector and blank the layer (scene 3118949804).
    assert_eq!(
        builtin_uniform_floats("g_ParallaxPosition", &v),
        Some(vec![0.5, 0.5])
    );
    for name in [
        "g_EffectTextureProjectionMatrix",
        "g_EffectTextureProjectionMatrixInverse",
    ] {
        assert_eq!(
            builtin_uniform_floats(name, &v),
            Some(Mat4::IDENTITY.to_cols_array().to_vec()),
            "{name}"
        );
    }
}

#[test]
fn layer_composite_texture_names_parse_to_layer_ids() {
    use truongpaper_renderer::scene_renderer::parse_layer_composite;
    assert_eq!(
        parse_layer_composite("_rt_imageLayerComposite_589_a"),
        Some(589)
    );
    assert_eq!(
        parse_layer_composite("_rt_imageLayerComposite_12_b"),
        Some(12)
    );
    assert_eq!(parse_layer_composite("_rt_imageLayerComposite_x_a"), None);
    assert_eq!(parse_layer_composite("_rt_FullFrameBuffer"), None);
}

#[test]
fn custom_vertex_shaders_that_forward_positions_get_pixel_space_quads() {
    // 3141421197 "myfirstshader": `v_Position = a_Position.xy` feeds a ray-marcher that expects
    // layer pixels, projected with `g_ModelViewProjectionMatrix`.
    let custom = "uniform mat4 g_ModelViewProjectionMatrix;\nvoid main() {\n\tgl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);\n\tv_Position.xy = a_Position.xy;\n}\n";
    assert!(vertex_wants_pixel_positions(custom));
    // Stock effect passes only project the clip-space quad.
    let stock = "uniform mat4 g_ModelViewProjectionMatrix;\nvoid main() {\n\tgl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);\n\tv_TexCoord = a_TexCoord;\n}\n";
    assert!(!vertex_wants_pixel_positions(stock));
    let clip = "void main() {\n\tgl_Position = vec4(a_Position, 1.0);\n\tv_Pos = a_Position;\n}\n";
    assert!(!vertex_wants_pixel_positions(clip));
}

#[test]
fn additive_light_blend_mode_31_uses_the_backdrop_blend_path() {
    use truongpaper_renderer::scene_renderer::{is_color_blend_mode, SCENE_COMPOSITE_WGSL};
    assert!(is_color_blend_mode(31));
    assert!(SCENE_COMPOSITE_WGSL.contains("case 31: { rgb = b + l; }"));
    assert!(!is_color_blend_mode(0) && !is_color_blend_mode(30));
}
