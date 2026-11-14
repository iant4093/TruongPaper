//! Which GPU draws photos and scenes (`AppSettings::render_gpu`).
//!
//! Laptops and desktops with both an integrated and a dedicated GPU default to the integrated
//! one, so wallpapers leave the dedicated GPU to games. The user may pick the dedicated GPU or
//! one specific adapter by name. Apple Silicon Macs have one GPU, so every choice lands on it.
//!
//! Web and Video wallpapers run in the system web view, which picks its own GPU.

use serde::Serialize;

/// Kind of a GPU as the user sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuKind {
    Integrated,
    Dedicated,
    Other,
}

impl GpuKind {
    pub fn from_device_type(t: wgpu::DeviceType) -> Self {
        match t {
            wgpu::DeviceType::IntegratedGpu => Self::Integrated,
            wgpu::DeviceType::DiscreteGpu => Self::Dedicated,
            _ => Self::Other,
        }
    }
}

/// One GPU the renderer can use (reported to Settings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuAdapterInfo {
    pub name: String,
    pub kind: GpuKind,
    /// Drawing the wallpapers right now.
    pub in_use: bool,
}

const ADAPTER_PREFIX: &str = "adapter:";
const MAX_ADAPTER_NAME: usize = 200;

/// Parsed `AppSettings::render_gpu`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuPreference {
    Integrated,
    Dedicated,
    Named(String),
}

impl GpuPreference {
    pub fn from_setting(raw: &str) -> Self {
        match raw.trim() {
            "dedicated" => Self::Dedicated,
            other => match other.strip_prefix(ADAPTER_PREFIX) {
                Some(name) if !name.trim().is_empty() => Self::Named(name.trim().to_string()),
                _ => Self::Integrated,
            },
        }
    }

    pub fn is_valid_setting(raw: &str) -> bool {
        match raw {
            "integrated" | "dedicated" => true,
            other => other.strip_prefix(ADAPTER_PREFIX).is_some_and(|name| {
                !name.trim().is_empty() && name.chars().count() <= MAX_ADAPTER_NAME
            }),
        }
    }

    /// The kind to fall back to when a named adapter is missing.
    fn kind_order(&self) -> [GpuKind; 3] {
        match self {
            Self::Dedicated => [GpuKind::Dedicated, GpuKind::Integrated, GpuKind::Other],
            _ => [GpuKind::Integrated, GpuKind::Dedicated, GpuKind::Other],
        }
    }
}

/// Order in which to try the candidate adapters: indices into `candidates`
/// (`(name, kind)` in the order wgpu enumerated them), best first. Software (CPU) adapters
/// must already be filtered out by the caller.
pub fn rank_adapters(candidates: &[(String, GpuKind)], pref: &GpuPreference) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::with_capacity(candidates.len());
    if let GpuPreference::Named(name) = pref {
        order.extend(
            candidates
                .iter()
                .enumerate()
                .filter(|(_, (n, _))| n.trim().eq_ignore_ascii_case(name))
                .map(|(i, _)| i),
        );
    }
    for kind in pref.kind_order() {
        for (i, (_, k)) in candidates.iter().enumerate() {
            if *k == kind && !order.contains(&i) {
                order.push(i);
            }
        }
    }
    order
}

/// One entry per GPU name (wgpu lists a GPU once per backend), in first-seen order.
pub fn unique_adapters(
    candidates: &[(String, GpuKind)],
    in_use: Option<&str>,
) -> Vec<GpuAdapterInfo> {
    let mut out: Vec<GpuAdapterInfo> = Vec::new();
    for (name, kind) in candidates {
        if out.iter().any(|a| a.name.eq_ignore_ascii_case(name)) {
            continue;
        }
        out.push(GpuAdapterInfo {
            name: name.clone(),
            kind: *kind,
            in_use: in_use.is_some_and(|u| u.eq_ignore_ascii_case(name)),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Vec<(String, GpuKind)> {
        vec![
            ("NVIDIA GeForce RTX 4080".into(), GpuKind::Dedicated),
            ("AMD Radeon(TM) Graphics".into(), GpuKind::Integrated),
            ("NVIDIA GeForce RTX 4080".into(), GpuKind::Dedicated),
            ("AMD Radeon(TM) Graphics".into(), GpuKind::Integrated),
        ]
    }

    #[test]
    fn integrated_is_the_default_and_dedicated_follows() {
        let pref = GpuPreference::from_setting("integrated");
        assert_eq!(rank_adapters(&list(), &pref), vec![1, 3, 0, 2]);
        assert_eq!(GpuPreference::from_setting(""), GpuPreference::Integrated);
        assert_eq!(
            GpuPreference::from_setting("nonsense"),
            GpuPreference::Integrated
        );
    }

    #[test]
    fn dedicated_comes_first_when_chosen() {
        let pref = GpuPreference::from_setting("dedicated");
        assert_eq!(rank_adapters(&list(), &pref), vec![0, 2, 1, 3]);
    }

    #[test]
    fn a_named_gpu_wins_and_a_missing_one_falls_back_to_integrated() {
        let pref = GpuPreference::from_setting("adapter:nvidia geforce rtx 4080");
        assert_eq!(rank_adapters(&list(), &pref), vec![0, 2, 1, 3]);
        let gone = GpuPreference::from_setting("adapter:Intel Arc A770");
        assert_eq!(rank_adapters(&list(), &gone), vec![1, 3, 0, 2]);
    }

    #[test]
    fn a_single_gpu_mac_uses_it_for_every_choice() {
        let mac = vec![("Apple M3 Max".to_string(), GpuKind::Integrated)];
        for raw in ["integrated", "dedicated", "adapter:Something else"] {
            assert_eq!(
                rank_adapters(&mac, &GpuPreference::from_setting(raw)),
                vec![0],
                "{raw}"
            );
        }
    }

    #[test]
    fn settings_values_are_validated() {
        assert!(GpuPreference::is_valid_setting("integrated"));
        assert!(GpuPreference::is_valid_setting("dedicated"));
        assert!(GpuPreference::is_valid_setting(
            "adapter:AMD Radeon(TM) Graphics"
        ));
        assert!(!GpuPreference::is_valid_setting("adapter:"));
        assert!(!GpuPreference::is_valid_setting("adapter:   "));
        assert!(!GpuPreference::is_valid_setting("fastest"));
        assert!(!GpuPreference::is_valid_setting(&format!(
            "adapter:{}",
            "x".repeat(201)
        )));
    }

    #[test]
    fn adapters_are_listed_once_with_the_one_in_use() {
        let got = unique_adapters(&list(), Some("AMD Radeon(TM) Graphics"));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "NVIDIA GeForce RTX 4080");
        assert!(!got[0].in_use);
        assert_eq!(got[1].kind, GpuKind::Integrated);
        assert!(got[1].in_use);
    }
}
