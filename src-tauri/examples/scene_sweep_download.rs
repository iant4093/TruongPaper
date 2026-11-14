//! Browses the most-subscribed Scene wallpapers, keeps those the GGNetwork guest mirror can
//! serve, and downloads up to N of them.
//!
//! `cargo run --example scene_sweep_download -- <out dir> [count=12] [pages=3]`
//! Each wallpaper lands in `<out dir>/<id>/` with its Steam preview as `steam_preview.jpg`.
use truongpaper_steam::browse::{WorkshopBrowseQuery, WorkshopSort};
use truongpaper_steam::guest_download::{
    download_guest_workshop, guest_availability, GuestAvailability,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(&args[1]);
    let want: usize = args.get(2).map_or(12, |v| v.parse().unwrap());
    let pages: u32 = args.get(3).map_or(3, |v| v.parse().unwrap());
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 TruongPaper-scene-sweep")
        .build()?;
    let mut done = 0;
    for page in 1..=pages {
        let browsed = truongpaper_steam::browse_workshop(
            &client,
            &WorkshopBrowseQuery {
                sort: WorkshopSort::MostSubscribed,
                page,
                days: -1,
                tags: vec!["Scene".into()],
                ..Default::default()
            },
        )
        .await?;
        for chunk in browsed.items.chunks(30) {
            let ids: Vec<String> = chunk
                .iter()
                .map(|i| i.preview.workshop_id.clone())
                .collect();
            let avail = guest_availability(&ids).await?;
            for item in chunk {
                let id = &item.preview.workshop_id;
                if done >= want || avail.get(id) != Some(&GuestAvailability::Available) {
                    continue;
                }
                let dir = out.join(id);
                if dir.join("project.json").exists() {
                    done += 1;
                    continue;
                }
                println!("downloading {id} {:?}", item.preview.title);
                match download_guest_workshop(id, &dir, |_| {}).await {
                    Ok(()) => {
                        done += 1;
                        if let Ok(resp) = client.get(&item.preview.preview_url).send().await {
                            if let Ok(bytes) = resp.bytes().await {
                                let _ = std::fs::write(dir.join("steam_preview.jpg"), bytes);
                            }
                        }
                        std::fs::write(dir.join("title.txt"), &item.preview.title)?;
                    }
                    Err(e) => println!("  failed: {e}"),
                }
            }
        }
        if done >= want {
            break;
        }
    }
    println!("{done} wallpapers ready in {}", out.display());
    Ok(())
}
