//! Live GGNetwork download of one Workshop item into a temp folder.
//! `cargo run -p truongpaper-steam --example guest_download_probe -- <workshop id>`
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let id = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "3836199874".into());
    let root = std::env::temp_dir().join(format!("tp-guest-probe-{}", std::process::id()));
    let dest = root.join(&id);
    let start = std::time::Instant::now();
    truongpaper_steam::guest_download::download_guest_workshop(&id, &dest, |p| {
        eprint!("\r{p:.0}%   ");
    })
    .await?;
    let files = std::fs::read_dir(&dest)?.count();
    println!(
        "installed {} ({files} entries) in {:?}",
        dest.display(),
        start.elapsed()
    );
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
