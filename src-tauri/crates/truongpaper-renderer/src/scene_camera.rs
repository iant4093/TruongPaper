//! Camera assets in 3D scenes.
//!
//! A perspective scene can hold `camera` objects (`{"camera": "default", "origin": ..,
//! "angles": .., "fov": .., "path": "scripts/camera_paths_<id>.json"}`). Wallpaper Engine
//! looks through the bottom-most visible one, which overrides the `scene.camera` eye/center
//! saved from the editor view and the `general.fov`. The camera looks down its local -Z with
//! +Y up, rotated by `angles` (radians, the same Euler order as layers). Camera paths are not
//! applied: the packages seen so far ship empty `paths` lists.

use glam::{EulerRot, Quat, Vec3};
use truongpaper_we_formats::{
    parse_vec3_or, resolve_bound_bool, resolve_bound_value, resolve_json_binding, SceneObject,
    UserPropertyMap,
};

use crate::json::Value;

/// A camera object found at load time: its object index and raw `fov` binding.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraObjectRef {
    pub object_index: usize,
    pub fov: Option<Value>,
}

/// Resolved view of a camera object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraView {
    pub eye: Vec3,
    pub center: Vec3,
    pub up: Vec3,
    pub fov_degrees: Option<f32>,
}

/// Collects every object with a `camera` key from the raw scene JSON, in scene order.
pub fn collect_camera_objects(raw_scene: &Value) -> Vec<CameraObjectRef> {
    let Some(objects) = raw_scene.get("objects").and_then(Value::as_array) else {
        return Vec::new();
    };
    objects
        .iter()
        .enumerate()
        .filter(|(_, o)| o.get("camera").is_some_and(|c| !c.is_null()))
        .map(|(object_index, o)| CameraObjectRef {
            object_index,
            fov: o.get("fov").cloned(),
        })
        .collect()
}

/// Eye, look-at point and up vector for a camera at `origin` rotated by `angles`.
pub fn camera_basis(origin: Vec3, angles: Vec3) -> (Vec3, Vec3, Vec3) {
    let rot = Quat::from_euler(EulerRot::ZYX, angles.z, angles.y, angles.x);
    (origin, origin + rot * Vec3::NEG_Z, rot * Vec3::Y)
}

fn fov_value(v: &Value, props: &UserPropertyMap) -> Option<f32> {
    let raw: serde_json::Value = serde_json::from_str(&v.to_json_string()).ok()?;
    match resolve_json_binding(&raw, props) {
        serde_json::Value::Number(n) => n.as_f64().map(|n| n as f32),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// The camera Wallpaper Engine uses: the last (bottom-most) visible camera object.
pub fn active_camera(
    refs: &[CameraObjectRef],
    objects: &[SceneObject],
    props: &UserPropertyMap,
) -> Option<CameraView> {
    refs.iter().rev().find_map(|r| {
        let obj = objects.get(r.object_index)?;
        if !resolve_bound_bool(&obj.visible, props) {
            return None;
        }
        let origin = parse_vec3_or(&resolve_bound_value(&obj.origin, props), Vec3::ZERO);
        let angles = parse_vec3_or(&resolve_bound_value(&obj.angles, props), Vec3::ZERO);
        let (eye, center, up) = camera_basis(origin, angles);
        Some(CameraView {
            eye,
            center,
            up,
            fov_degrees: r
                .fov
                .as_ref()
                .and_then(|f| fov_value(f, props))
                .filter(|f| *f > 1.0 && *f < 179.0),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use truongpaper_we_formats::parse_scene_json;

    const SCENE: &str = r#"{
        "camera": { "center": "0 0 -1", "eye": "0 0 0", "up": "0 1 0" },
        "general": {},
        "objects": [
            { "id": 1, "name": "a", "camera": "default", "origin": "1 2 3", "fov": 40 },
            { "id": 2, "name": "img", "image": "models/x.json", "origin": "0 0 0" },
            { "id": 3, "name": "b", "camera": "default", "origin": "0 0 6",
              "fov": { "user": "camfov", "value": 50 } },
            { "id": 4, "name": "c", "camera": "default", "origin": "9 9 9", "visible": false }
        ]
    }"#;

    #[test]
    fn bottom_most_visible_camera_wins() {
        let raw = Value::parse(SCENE).unwrap();
        let scene = parse_scene_json(SCENE).unwrap();
        let refs = collect_camera_objects(&raw);
        assert_eq!(
            refs.iter().map(|r| r.object_index).collect::<Vec<_>>(),
            [0, 2, 3]
        );
        let props = UserPropertyMap::new();
        let view = active_camera(&refs, &scene.objects, &props).unwrap();
        assert_eq!(view.eye, Vec3::new(0.0, 0.0, 6.0));
        assert!((view.center - Vec3::new(0.0, 0.0, 5.0)).length() < 1e-6);
        assert!((view.up - Vec3::Y).length() < 1e-6);
        assert_eq!(view.fov_degrees, Some(50.0));
        // Only the first camera left: it is used with its plain fov.
        let view = active_camera(&refs[..1], &scene.objects, &props).unwrap();
        assert_eq!(
            (view.eye, view.fov_degrees),
            (Vec3::new(1.0, 2.0, 3.0), Some(40.0))
        );
        assert!(active_camera(&refs[2..], &scene.objects, &props).is_none());
    }

    #[test]
    fn fov_follows_user_property() {
        let raw = Value::parse(SCENE).unwrap();
        let scene = parse_scene_json(SCENE).unwrap();
        let refs = collect_camera_objects(&raw);
        let mut props = UserPropertyMap::new();
        props.insert("camfov".into(), serde_json::json!(60));
        let view = active_camera(&refs, &scene.objects, &props).unwrap();
        assert_eq!(view.fov_degrees, Some(60.0));
    }

    #[test]
    fn camera_basis_rotates_forward_and_up() {
        let (eye, center, up) =
            camera_basis(Vec3::ZERO, Vec3::new(0.0, std::f32::consts::FRAC_PI_2, 0.0));
        assert_eq!(eye, Vec3::ZERO);
        // Yaw +90 deg about Y turns -Z into -X.
        assert!((center - Vec3::NEG_X).length() < 1e-5);
        assert!((up - Vec3::Y).length() < 1e-5);
    }

    #[test]
    fn scene_without_cameras_has_none() {
        let raw = Value::parse(r#"{"objects":[{"image":"x"}]}"#).unwrap();
        assert!(collect_camera_objects(&raw).is_empty());
    }
}
