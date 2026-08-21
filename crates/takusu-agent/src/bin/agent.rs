use clap::Parser;
use std::process;
use takusu_agent::AgentConfig;
use takusu_client::Client;

#[derive(Parser)]
#[command(name = "takusu-agent", about = "takusu voice assistant agent")]
struct Cli {
    /// Text input for a single agent turn. If omitted, run push-to-talk audio mode.
    #[arg(long, short = 't')]
    text: Option<String>,

    /// Disable TTS in audio mode.
    #[arg(long)]
    no_tts: bool,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let cli = Cli::parse();
    let config = AgentConfig::load()?;
    let client = Client::new(&config.server.url, &config.server.token);
    let session = takusu_agent::runner::build_session(&config, client)?;

    if let Some(text) = cli.text {
        let result = takusu_agent::runner::run_text(&session, &text).await?;

        println!("{}", result.text);
        if !result.changes.is_empty() {
            let changes = serde_json::to_string_pretty(&result.changes)?;
            eprintln!("{changes}");
        }
        if result.schedule_dirty {
            eprintln!("schedule dirty: true");
        }
    } else {
        #[cfg(feature = "audio-device")]
        {
            takusu_agent::runner::run_audio(session, cli.no_tts).await?;
        }
        #[cfg(not(feature = "audio-device"))]
        {
            return Err("audio support is not enabled".into());
        }
    }

    Ok(())
}
