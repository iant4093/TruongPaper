//! Live public-catalog smoke check, deliberately separate from headless tests.
use truongpaper_steam::{browse_workshop, WorkshopBrowseQuery};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("TruongPaper/1.0")
        .build()?;
    for query in [
        WorkshopBrowseQuery::default(),
        WorkshopBrowseQuery {
            search: "mountain".into(),
            tags: vec!["Scene".into()],
            page: 2,
            ..Default::default()
        },
    ] {
        let page = browse_workshop(&client, &query).await?;
        anyhow::ensure!(!page.items.is_empty(), "No live results");
        anyhow::ensure!(page.page == query.page, "Incorrect page");
        anyhow::ensure!(!page.tag_groups.is_empty(), "No live filters");
        println!(
            "page={} total={} results={} filter_groups={} first={:?}",
            page.page,
            page.total_items,
            page.items.len(),
            page.tag_groups.len(),
            page.items[0].preview.title
        );
    }
    Ok(())
}
