//! Live smoke check: deliver QR without logging in or printing the authentication challenge.
//! Pass the bundled DepotDownloader path as the only argument. No credentials are entered.
use anyhow::{bail, Context};
use std::{path::PathBuf, time::Duration};
use tokio::sync::mpsc;
use truongpaper_steam::{qr_login_args, run_depot_downloader_sidecar, SidecarEvent};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let binary = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("Pass the bundled DepotDownloader path")?,
    );
    let binary = binary.canonicalize()?;
    let working_dir = std::env::temp_dir().join("truongpaper-steam-qr-probe");
    let (_code_tx, codes) = mpsc::channel(1);
    let (events, mut received) = mpsc::channel(16);
    let task = tokio::spawn(async move {
        run_depot_downloader_sidecar(
            &binary,
            &working_dir,
            &qr_login_args(),
            Some(codes),
            None,
            None,
            events,
        )
        .await
    });
    let result = tokio::time::timeout(Duration::from_secs(45), async {
        while let Some(event) = received.recv().await {
            if let SidecarEvent::QrMatrix(matrix) = event {
                let width = matrix.first().map(Vec::len).unwrap_or(0);
                anyhow::ensure!(
                    width >= 21
                        && matrix.len().abs_diff(width) <= 1
                        && matrix.iter().all(|row| row.len() == width),
                    "Invalid QR dimensions"
                );
                println!(
                    "Live Steam QR delivered: {} x {} pixels. Challenge is not printed or saved.",
                    width,
                    matrix.len()
                );
                return Ok(());
            }
        }
        bail!("Downloader exited before delivering a QR")
    })
    .await;
    task.abort();
    let _ = task.await;
    result.context("Timed out waiting for Steam QR")?
}
