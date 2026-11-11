//! `DepotDownloader` v3.4.0 sidecar runner.
//!
//! Stream handling is generic over `AsyncRead`/`AsyncWrite` so it is unit-tested with in-memory
//! buffers. Only [`run_depot_downloader_sidecar`] spawns a real process.
//!
//! `stderr` is read as raw byte chunks: `ConsoleAuthenticator` writes its 2FA prompt WITHOUT a
//! trailing newline, so a line reader would deadlock.

use crate::qr::{is_qr_line, parse_ascii_qr_to_matrix, QR_HEADER};
use serde::Serialize;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

/// Events emitted to the UI layer (`steam-auth://qr-matrix`, `steam-auth://need-2fa`, ...).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum SidecarEvent {
    QrMatrix(Vec<Vec<bool>>),
    Need2Fa { kind: &'static str, prompt: String },
    LoggedInUsername(String),
    Authenticated,
    ProgressPercent(f32),
    Completed,
}

/// Marker present in every `DepotDownloader` interactive prompt.
const PROMPT_MARKER: &str = "Please enter";
/// Max bytes of stderr retained between chunks for split-prompt detection.
const SCAN_WINDOW: usize = 1024;

/// A detected 2FA prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoFaPrompt {
    /// `"email"` or `"totp"`.
    pub kind: &'static str,
    pub prompt: String,
}

/// Detects a `DepotDownloader` 2FA prompt in a raw stderr byte buffer.
pub fn detect_2fa_prompt(buf: &[u8]) -> Option<TwoFaPrompt> {
    let text = String::from_utf8_lossy(buf);
    let idx = text.find(PROMPT_MARKER)?;
    // Keep the "STEAM GUARD!" prefix when present.
    let start = text[..idx].rfind("STEAM GUARD").unwrap_or(idx);
    let prompt = text[start..].trim().to_string();
    let kind = if prompt.to_ascii_lowercase().contains("email") {
        "email"
    } else {
        "totp"
    };
    Some(TwoFaPrompt { kind, prompt })
}

/// Accumulates stderr chunks so a prompt split across two reads is still detected.
#[derive(Debug, Default)]
pub struct StderrPromptScanner {
    window: Vec<u8>,
}

impl StderrPromptScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds a raw chunk. Returns the prompt once; the window is cleared after a hit.
    pub fn push(&mut self, chunk: &[u8]) -> Option<TwoFaPrompt> {
        self.window.extend_from_slice(chunk);
        if let Some(p) = detect_2fa_prompt(&self.window) {
            self.window.clear();
            return Some(p);
        }
        if self.window.len() > SCAN_WINDOW {
            let cut = self.window.len() - SCAN_WINDOW;
            self.window.drain(..cut);
        }
        None
    }
}

/// Line-oriented stdout state machine (QR block, saved username, progress percent).
#[derive(Debug, Default)]
pub struct StdoutParser {
    in_qr: bool,
    qr_lines: Vec<String>,
}

impl StdoutParser {
    pub fn new() -> Self {
        Self::default()
    }

    fn flush_qr(&mut self, out: &mut Vec<SidecarEvent>) {
        if !self.qr_lines.is_empty() {
            out.push(SidecarEvent::QrMatrix(parse_ascii_qr_to_matrix(
                &self.qr_lines,
            )));
        }
        self.qr_lines.clear();
        self.in_qr = false;
    }

    /// Feeds one stdout line (without newline).
    pub fn feed_line(&mut self, raw: &str) -> Vec<SidecarEvent> {
        let line = raw.trim_end_matches('\r');
        let mut out = Vec::new();

        if line.contains(QR_HEADER) {
            self.in_qr = true;
            self.qr_lines.clear();
            return out;
        }
        if self.in_qr {
            if is_qr_line(line) {
                self.qr_lines.push(line.to_string());
                // DepotDownloader 3.4 prints a square QR and immediately waits for
                // authentication, without a blank terminator or closing stdout.
                // Each character is two vertical pixels (half blocks); the full
                // block renderer uses two characters per module horizontally.
                let width = self.qr_lines[0].chars().count();
                if self.qr_lines.len() * 2 >= width {
                    self.flush_qr(&mut out);
                }
                return out;
            } else if !self.qr_lines.is_empty() {
                self.flush_qr(&mut out);
            }
        }

        // "Success! Next time you can login with -username <AccountName> -remember-password ..."
        const LOGIN_MARK: &str = "login with -username ";
        if let Some(idx) = line.find(LOGIN_MARK) {
            if let Some(user) = line[idx + LOGIN_MARK.len()..].split_whitespace().next() {
                out.push(SidecarEvent::LoggedInUsername(user.to_string()));
            }
        }

        // " 42.68% ..." progress
        if let Some(pct_idx) = line.find('%') {
            if let Some(num) = line[..pct_idx].split_whitespace().last() {
                if let Ok(pct) = num.parse::<f32>() {
                    out.push(SidecarEvent::ProgressPercent(pct));
                }
            }
        }
        out
    }

    /// Call at EOF to emit a QR block that was still being accumulated.
    pub fn finish(&mut self) -> Vec<SidecarEvent> {
        let mut out = Vec::new();
        self.flush_qr(&mut out);
        out
    }
}

/// Translate known failures into user-facing text. Raw output can contain
/// Steam account names or authentication challenges, so never return it to the UI.
fn download_failure(output: &str) -> Option<&'static str> {
    let output = output.to_ascii_lowercase();
    if output.contains("is not available from this account") {
        Some("Steam says this account does not have access to Wallpaper Engine. Use a Steam account that owns it.")
    } else if output.contains("enter account password for")
        || output.contains("access token was rejected")
        || output.contains("unable to login to steam3: expired")
        || output.contains("unable to login to steam3: invalidpassword")
    {
        Some("Sign in to Steam again. The saved Steam session is missing or expired.")
    } else if output.contains("read-only file system")
        || output.contains("unauthorizedaccessexception")
        || output.contains("failed to save account settings")
        || output.contains("unable to create install directories")
    {
        Some("The Steam downloader could not write its files. Check that TruongPaper can write to its app data folder.")
    } else if output.contains("unable to locate manifest id for published file")
        || output.contains("failed to get published file details")
    {
        Some("Steam could not find downloadable files for this Workshop item.")
    } else if output.contains("could not connect to steam after") {
        Some("The downloader could not connect to Steam. Try again when Steam is reachable.")
    } else {
        None
    }
}

/// Reads `stdout` line by line and forwards parsed events.
pub async fn pump_stdout<R: AsyncRead + Unpin>(
    stdout: R,
    event_tx: &mpsc::Sender<SidecarEvent>,
) -> std::io::Result<Option<&'static str>> {
    let mut lines = BufReader::new(stdout).lines();
    let mut parser = StdoutParser::new();
    let mut failure = None;
    while let Some(line) = lines.next_line().await? {
        failure = failure.or_else(|| download_failure(&line));
        // The license list arrives after the account is logged in. A bare
        // "Done!" can also mean the transport connected, especially on retry.
        if failure.is_none()
            && line
                .trim()
                .strip_prefix("Got ")
                .and_then(|line| line.strip_suffix(" licenses for account!"))
                .is_some_and(|count| count.parse::<u32>().is_ok())
        {
            let _ = event_tx.send(SidecarEvent::Authenticated).await;
        }
        for ev in parser.feed_line(&line) {
            let _ = event_tx.send(ev).await;
        }
    }
    for ev in parser.finish() {
        let _ = event_tx.send(ev).await;
    }
    Ok(failure)
}

/// Reads `stderr` in raw chunks. On a 2FA prompt, emits `Need2Fa`, waits for a code on
/// `two_fa_rx`, and writes `"<code>\n"` to `stdin`.
pub async fn pump_stderr<R, W>(
    mut stderr: R,
    stdin: &mut W,
    two_fa_rx: &mut mpsc::Receiver<String>,
    event_tx: &mpsc::Sender<SidecarEvent>,
) -> Option<&'static str>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut scanner = StderrPromptScanner::new();
    let mut failure = None;
    let mut buf = [0u8; 1024];
    loop {
        let n = match stderr.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let prompt = scanner.push(&buf[..n]);
        failure = failure.or_else(|| download_failure(&String::from_utf8_lossy(&scanner.window)));
        let Some(prompt) = prompt else { continue };
        let _ = event_tx
            .send(SidecarEvent::Need2Fa {
                kind: prompt.kind,
                prompt: prompt.prompt,
            })
            .await;

        // Wait for the code, but stop if the process closes stderr (exited) meanwhile.
        let code = loop {
            tokio::select! {
                biased;
                code = two_fa_rx.recv() => break code,
                r = stderr.read(&mut buf) => {
                    if matches!(r, Ok(0) | Err(_)) {
                        break None;
                    }
                }
            }
        };
        match code {
            Some(code) => {
                let line = format!("{}\n", code.trim());
                let _ = stdin.write_all(line.as_bytes()).await;
                let _ = stdin.flush().await;
            }
            None => break,
        }
    }
    failure
}

/// Drives both pipes concurrently until both reach EOF.
pub async fn drive_streams<O, E, I>(
    stdout: O,
    stderr: E,
    mut stdin: I,
    two_fa_rx: &mut mpsc::Receiver<String>,
    event_tx: &mpsc::Sender<SidecarEvent>,
) -> std::io::Result<Option<&'static str>>
where
    O: AsyncRead + Unpin,
    E: AsyncRead + Unpin,
    I: AsyncWrite + Unpin,
{
    let (out, err) = tokio::join!(
        pump_stdout(stdout, event_tx),
        pump_stderr(stderr, &mut stdin, two_fa_rx, event_tx)
    );
    Ok(out?.or(err))
}

/// Spawns `sidecar_bin` with `args`, streams events, and errors on a non-zero exit.
pub async fn run_depot_downloader_sidecar(
    sidecar_bin: &Path,
    working_dir: &Path,
    args: &[String],
    two_fa_rx: Option<mpsc::Receiver<String>>,
    event_tx: mpsc::Sender<SidecarEvent>,
) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(working_dir).await?;
    let mut cmd = Command::new(sidecar_bin);
    cmd.current_dir(working_dir)
        .args(args)
        .stdin(if two_fa_rx.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("sidecar stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("sidecar stderr was not piped"))?;

    let failure = if let (Some(stdin), Some(mut codes)) = (stdin, two_fa_rx) {
        drive_streams(stdout, stderr, stdin, &mut codes, &event_tx).await?
    } else {
        let (unused, mut codes) = mpsc::channel(1);
        drop(unused);
        drive_streams(stdout, stderr, tokio::io::sink(), &mut codes, &event_tx).await?
    };

    let status = child.wait().await?;
    if let Some(failure) = failure {
        anyhow::bail!("{failure}");
    }
    if !status.success() {
        anyhow::bail!("DepotDownloader exited with status: {status}");
    }
    let _ = event_tx.send(SidecarEvent::Completed).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOTP: &str =
        "STEAM GUARD! Please enter your 2-factor auth code from your authenticator app: ";
    const EMAIL: &str =
        "STEAM GUARD! Please enter the auth code sent to the email at i***@example.com: ";

    #[test]
    fn detects_totp_prompt_without_newline() {
        let p = detect_2fa_prompt(TOTP.as_bytes()).unwrap();
        assert_eq!(p.kind, "totp");
        assert!(p.prompt.starts_with("STEAM GUARD!"));
        assert!(!p.prompt.ends_with(' '));
    }

    #[test]
    fn detects_email_prompt() {
        let p = detect_2fa_prompt(EMAIL.as_bytes()).unwrap();
        assert_eq!(p.kind, "email");
    }

    #[test]
    fn ignores_unrelated_stderr() {
        assert_eq!(detect_2fa_prompt(b"warning: something happened\n"), None);
        assert_eq!(detect_2fa_prompt(b""), None);
        assert_eq!(detect_2fa_prompt(&[0xff, 0xfe, 0x00]), None);
    }

    #[test]
    fn scanner_detects_prompt_split_across_chunks_once() {
        let bytes = TOTP.as_bytes();
        let (a, b) = bytes.split_at(20);
        let mut s = StderrPromptScanner::new();
        assert_eq!(s.push(a), None);
        let p = s.push(b).unwrap();
        assert_eq!(p.kind, "totp");
        assert_eq!(s.push(b"more output"), None);
    }

    #[test]
    fn stdout_parser_emits_qr_matrix_username_progress() {
        let mut p = StdoutParser::new();
        let mut evs = Vec::new();
        for line in [
            "Logging in...",
            "Use the Steam Mobile App to sign in with this QR code:",
            "█▀▄ ",
            " ▄▀█\r",
            "",
            "Success! Next time you can login with -username ian_t -remember-password instead of -qr.",
            " 42.68% depots/431961/file.bin",
        ] {
            evs.extend(p.feed_line(line));
        }
        assert_eq!(
            evs,
            vec![
                SidecarEvent::QrMatrix(vec![
                    vec![true, true, false, false],
                    vec![true, false, true, false],
                    vec![false, false, true, true],
                    vec![false, true, false, true],
                ]),
                SidecarEvent::LoggedInUsername("ian_t".into()),
                SidecarEvent::ProgressPercent(42.68),
            ]
        );
    }

    #[test]
    fn stdout_parser_flushes_qr_at_eof() {
        let mut p = StdoutParser::new();
        assert!(p.feed_line(QR_HEADER).is_empty());
        assert!(p.feed_line("██  ").is_empty());
        assert_eq!(
            p.finish(),
            vec![SidecarEvent::QrMatrix(vec![
                vec![true, true, false, false],
                vec![true, true, false, false]
            ])]
        );
        assert!(p.finish().is_empty());
    }

    #[tokio::test]
    async fn qr_is_delivered_while_stdout_stays_open_waiting_for_scan() {
        let (mut producer, reader) = tokio::io::duplex(1024);
        let (events, mut received) = mpsc::channel(8);
        let pump = tokio::spawn(async move { pump_stdout(reader, &events).await });
        // Same double-width full-block framing as QRCoder.GetLineByLineGraphic(1).
        producer
            .write_all(format!("{QR_HEADER}:\n        \n  ████  \n  ████  \n        \n").as_bytes())
            .await
            .unwrap();
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), received.recv())
            .await
            .unwrap()
            .unwrap();
        let SidecarEvent::QrMatrix(matrix) = event else {
            panic!("missing QR event")
        };
        assert_eq!(matrix.len(), 8);
        assert!(matrix.iter().all(|row| row.len() == 8));
        assert_eq!(
            matrix[2],
            vec![false, false, true, true, true, true, false, false]
        );
        // The writer remains open until AFTER the QR event is received.
        drop(producer);
        pump.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn pump_stderr_replies_to_prompt_on_stdin() {
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let (code_tx, mut code_rx) = mpsc::channel(1);
        code_tx.send(" 123456 ".to_string()).await.unwrap();
        let mut stdin: Vec<u8> = Vec::new();

        pump_stderr(TOTP.as_bytes(), &mut stdin, &mut code_rx, &event_tx).await;

        assert_eq!(stdin, b"123456\n");
        match event_rx.recv().await.unwrap() {
            SidecarEvent::Need2Fa { kind, .. } => assert_eq!(kind, "totp"),
            other => panic!("unexpected event {other:?}"),
        }
    }

    #[tokio::test]
    async fn pump_stderr_stops_when_stderr_closes_while_waiting_for_code() {
        let (event_tx, _event_rx) = mpsc::channel(8);
        let (_code_tx, mut code_rx) = mpsc::channel::<String>(1); // sender kept alive, never used
        let mut stdin: Vec<u8> = Vec::new();
        pump_stderr(EMAIL.as_bytes(), &mut stdin, &mut code_rx, &event_tx).await;
        assert!(stdin.is_empty());
    }

    #[tokio::test]
    async fn confirms_login_after_the_license_list_and_reports_missing_access() {
        let (events, mut received) = mpsc::channel(8);
        let stdout = "Connecting to Steam3... Done!\n Done!\nGot 12 licenses for account!\nApp 431960 (Wallpaper Engine) is not available from this account.\n";
        let failure = pump_stdout(stdout.as_bytes(), &events)
            .await
            .unwrap()
            .unwrap();
        assert!(failure.contains("does not have access to Wallpaper Engine"));
        assert_eq!(received.try_recv().unwrap(), SidecarEvent::Authenticated);
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn failed_login_storage_does_not_confirm_a_remembered_session() {
        let (events, mut received) = mpsc::channel(8);
        let stdout =
            "Failed to save account settings: private-path\nGot 12 licenses for account!\n";
        let failure = pump_stdout(stdout.as_bytes(), &events)
            .await
            .unwrap()
            .unwrap();
        assert!(failure.contains("could not write its files"));
        assert!(!failure.contains("private-path"));
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn drive_streams_runs_both_pipes() {
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let (code_tx, mut code_rx) = mpsc::channel(1);
        code_tx.send("ABCDE".to_string()).await.unwrap();
        let stdout = " 10.00% x\n 20.50% y\n".as_bytes();
        let sink: Vec<u8> = Vec::new();

        drive_streams(stdout, TOTP.as_bytes(), sink, &mut code_rx, &event_tx)
            .await
            .unwrap();
        drop(event_tx);

        let mut got = Vec::new();
        while let Some(e) = event_rx.recv().await {
            got.push(e);
        }
        assert!(got.contains(&SidecarEvent::ProgressPercent(10.0)));
        assert!(got.contains(&SidecarEvent::ProgressPercent(20.5)));
        assert!(got
            .iter()
            .any(|e| matches!(e, SidecarEvent::Need2Fa { kind: "totp", .. })));
    }

    #[test]
    fn events_serialize_for_tauri_emit() {
        let v = serde_json::to_value(SidecarEvent::QrMatrix(vec![vec![true]])).unwrap();
        assert_eq!(v["type"], "qrMatrix");
        assert_eq!(v["data"][0][0], true);
        let v = serde_json::to_value(SidecarEvent::Completed).unwrap();
        assert_eq!(v["type"], "completed");
    }
}
