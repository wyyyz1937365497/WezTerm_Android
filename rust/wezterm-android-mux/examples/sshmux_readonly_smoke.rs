use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};
use wezterm_android_mux::{AndroidMuxSession, MuxEndpoint, MuxEvent};
use wezterm_term::TerminalSize;

fn required(name: &str) -> anyhow::Result<String> {
    std::env::var(name).map_err(|_| anyhow::anyhow!("missing required environment variable {name}"))
}

fn optional_usize(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn enabled(name: &str) -> bool {
    std::env::var(name).as_deref() == Ok("1")
}

fn main() -> anyhow::Result<()> {
    let host = required("WEZTERM_ANDROID_MUX_HOST")?;
    let user = required("WEZTERM_ANDROID_MUX_USER")?;
    let files_dir = PathBuf::from(required("WEZTERM_ANDROID_MUX_FILES_DIR")?);
    let identity = PathBuf::from(required("WEZTERM_ANDROID_MUX_IDENTITY")?);
    let remote_wezterm = required("WEZTERM_ANDROID_REMOTE_WEZTERM")?;
    let port = std::env::var("WEZTERM_ANDROID_MUX_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(22);
    let endpoint = MuxEndpoint::new(host, user, port, files_dir, Some(identity), remote_wezterm)?;
    let session = AndroidMuxSession::start(
        endpoint,
        TerminalSize {
            rows: optional_usize("WEZTERM_ANDROID_MUX_ROWS", 24),
            cols: optional_usize("WEZTERM_ANDROID_MUX_COLS", 80),
            pixel_width: optional_usize("WEZTERM_ANDROID_MUX_PIXEL_WIDTH", 800),
            pixel_height: optional_usize("WEZTERM_ANDROID_MUX_PIXEL_HEIGHT", 480),
            dpi: optional_usize("WEZTERM_ANDROID_MUX_DPI", 160) as u32,
        },
        None,
    )?;

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut attached = false;
    while Instant::now() < deadline {
        while let Some(event) = session.try_next_event() {
            println!("event={event:?}");
            match event {
                MuxEvent::Attached { .. } => attached = true,
                MuxEvent::Error { message } => anyhow::bail!(message),
                _ => {}
            }
        }
        if attached {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    anyhow::ensure!(attached, "timed out waiting for SSHMUX attach");

    let tabs = session.tabs()?;
    println!("attached_tabs={tabs:?}");
    anyhow::ensure!(!tabs.is_empty(), "remote mux returned no tabs");

    let snapshot_deadline = Instant::now() + Duration::from_secs(5);
    let mut occupied = 0;
    while Instant::now() < snapshot_deadline {
        if let Some(snapshot) = session.snapshot()? {
            occupied = snapshot.terminal.cells.len();
            if occupied > 0 {
                println!(
                    "snapshot={}x{} occupied={occupied}",
                    snapshot.terminal.columns, snapshot.terminal.rows
                );
                break;
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    anyhow::ensure!(occupied > 0, "SSHMUX snapshot stayed empty");

    if enabled("WEZTERM_ANDROID_MUX_EXERCISE_EPHEMERAL_TAB") {
        let original_pane = tabs
            .iter()
            .find(|tab| tab.active)
            .map(|tab| tab.pane_id)
            .ok_or_else(|| anyhow::anyhow!("initial mux snapshot has no active tab"))?;
        session.spawn_tab()?;
        let spawned = session.tabs()?;
        anyhow::ensure!(
            spawned.len() == tabs.len() + 1,
            "expected exactly one newly spawned tab"
        );
        let spawned_pane = spawned
            .iter()
            .find(|tab| tab.active)
            .map(|tab| tab.pane_id)
            .ok_or_else(|| anyhow::anyhow!("spawned mux snapshot has no active tab"))?;
        anyhow::ensure!(spawned_pane != original_pane, "new tab was not activated");

        session.activate_relative(-1)?;
        anyhow::ensure!(
            session
                .tabs()?
                .iter()
                .any(|tab| tab.active && tab.pane_id == original_pane),
            "previous-tab action did not activate the original tab"
        );
        session.activate_relative(1)?;
        anyhow::ensure!(
            session
                .tabs()?
                .iter()
                .any(|tab| tab.active && tab.pane_id == spawned_pane),
            "next-tab action did not return to the ephemeral tab"
        );
        session.close_active_tab()?;
        let cleaned = session.tabs()?;
        anyhow::ensure!(
            cleaned.len() == tabs.len()
                && cleaned
                    .iter()
                    .any(|tab| tab.active && tab.pane_id == original_pane),
            "ephemeral tab cleanup did not restore the original tab set"
        );
        println!("ephemeral_tab_spawn_switch_close=ok");
    }

    session.detach()?;
    println!("safe_detach=ok");
    Ok(())
}
