//! Dev aid: translates shaders dumped via TRUONGPAPER_DUMP_FAILED_SHADERS.
use truongpaper_renderer::shader_preprocessor::translate_we_shader_pair;
fn main() {
    for a in std::env::args().skip(1) {
        let v = std::fs::read_to_string(format!("{a}.vert")).unwrap();
        let f = std::fs::read_to_string(format!("{a}.frag")).unwrap();
        match translate_we_shader_pair(&a, &v, &f) {
            Ok(_) => println!("OK {a}"),
            Err(e) => println!("ERR {a}: {}", e.chars().take(600).collect::<String>()),
        }
    }
}
