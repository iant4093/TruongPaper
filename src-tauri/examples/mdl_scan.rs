//! Reports which `.mdl` files in a directory fail to parse (mesh block and skeleton).
//! Usage: `cargo run --release --example mdl_scan -- <dir with .mdl files>`
use truongpaper_renderer::parse_mdl_skeleton;
use truongpaper_we_formats::{parse_mdl, parse_static_mdl};

fn main() {
    let dir = std::env::args().nth(1).expect("usage: mdl_scan <dir>");
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .expect("read_dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mdl"))
        .collect();
    names.sort();
    let (mut ok, mut bad) = (0, 0);
    for p in names {
        let bytes = std::fs::read(&p).unwrap();
        let name = p.file_name().unwrap().to_string_lossy();
        match parse_mdl(&bytes, [0.0, 0.0]) {
            Err(e) => {
                // Static 3D models (no skeleton) use a different layout.
                if let Ok(m) = parse_static_mdl(&bytes) {
                    ok += 1;
                    println!(
                        "ok {name}: static {} v={} idx={}",
                        m.version,
                        m.vertices.len(),
                        m.indices.len()
                    );
                    continue;
                }
                bad += 1;
                println!("FAIL mesh {name}: {e}");
            }
            Ok(m) => {
                let sk = if m.section("MDLS").is_some() {
                    match parse_mdl_skeleton(&bytes, &m) {
                        Ok(s) => format!("bones={} anims={}", s.bones.len(), s.animations.len()),
                        Err(e) => format!("SKELETON FAIL: {e}"),
                    }
                } else {
                    "no MDLS".into()
                };
                if sk.contains("FAIL") {
                    bad += 1;
                } else {
                    ok += 1;
                }
                println!(
                    "ok {name}: {} v={} idx={} ext={} {sk}",
                    m.version,
                    m.vertices.len(),
                    m.indices.len(),
                    m.extended_layout
                );
            }
        }
    }
    println!("ok={ok} bad={bad}");
}
