mod app;
mod event;
mod style;
mod tabs;
mod ui;
mod widgets;

use std::io;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::app::{App, Msg};

pub async fn run(
    app: Arc<takusu_local_lib::app::TakusuApp>,
    tz: jiff::tz::TimeZone,
) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, app, tz).await;
    ratatui::restore();
    result
}

async fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: Arc<takusu_local_lib::app::TakusuApp>,
    tz: jiff::tz::TimeZone,
) -> io::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    tokio::spawn(event::spawn(tx.clone()));

    let mut state = App::new(app, tz);
    state.load_initial().await;

    loop {
        // Load the detail pane's comments before drawing so the selection that
        // changed on the previous key is reflected immediately (WI-5). This is
        // lazily cached per task, so it does not cause queries for other tasks.
        state.ensure_selected_comments().await;

        terminal.draw(|f| ui::draw(f, &mut state))?;

        match rx.recv().await {
            Some(Msg::Key(key)) => {
                if state.handle_key(key, terminal).await {
                    break;
                }
            }
            Some(Msg::Tick) => state.on_tick().await,
            None => break,
        }
    }
    Ok(())
}
