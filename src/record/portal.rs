//! xdg-desktop-portal ScreenCast session: lets the user pick a monitor,
//! window or region and hands back a PipeWire fd + node id.

use anyhow::{Context, Result, anyhow};
use ashpd::desktop::PersistMode;
use ashpd::desktop::Session;
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use std::os::fd::OwnedFd;

pub struct PortalStream {
    pub fd: OwnedFd,
    pub node_id: u32,
    pub position: Option<(i32, i32)>,
    pub size: Option<(i32, i32)>,
    pub kind: String,
    // Dropping these ends the cast, so they live as long as the recording.
    pub _proxy: Screencast,
    pub session: Session<Screencast>,
}

fn token_path() -> std::path::PathBuf {
    crate::util::config_dir().join("restore_token")
}

pub async fn open(show_cursor: bool, reuse_selection: bool) -> Result<PortalStream> {
    let proxy = Screencast::new().await.context("ScreenCast portal unavailable")?;
    let session = proxy.create_session(Default::default()).await?;
    let token = if reuse_selection { std::fs::read_to_string(token_path()).ok() } else { None };
    let cursor = if show_cursor { CursorMode::Embedded } else { CursorMode::Hidden };
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(cursor)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_persist_mode(PersistMode::ExplicitlyRevoked)
                .set_restore_token(token.as_deref()),
        )
        .await
        .context("selecting sources")?;
    let response = proxy
        .start(&session, None, Default::default())
        .await?
        .response()
        .context("screen sharing was cancelled")?;
    let stream = response.streams().first().ok_or_else(|| anyhow!("portal returned no streams"))?.clone();
    let restore_token = response.restore_token().map(str::to_owned);
    if let Some(t) = &restore_token {
        let _ = std::fs::create_dir_all(crate::util::config_dir());
        let _ = std::fs::write(token_path(), t);
    }
    let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await?;
    let kind = match stream.source_type() {
        Some(SourceType::Window) => "window",
        Some(SourceType::Virtual) => "virtual",
        _ => "monitor",
    };
    Ok(PortalStream {
        fd,
        node_id: stream.pipe_wire_node_id(),
        position: stream.position(),
        size: stream.size(),
        kind: kind.into(),
        _proxy: proxy,
        session,
    })
}
