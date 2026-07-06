use std::time::Duration;

use tokio::io::AsyncBufReadExt;
use tracing::{error, info};

use velo_core::builder::VeloAgentBuilder;
use velo_core::config::VeloConfig;
use velo_core::events::event_names;
use velo_core::init_tracing;

/// Standalone REPL entry point.
pub async fn run() -> anyhow::Result<()> {
    init_tracing();

    let config = VeloConfig::load().map_err(|e| {
        eprintln!("Configuration error: {e}");
        e
    })?;

    info!("Starting Velo agent (standalone / headless REPL)");

    let (event_tx, mut event_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, serde_json::Value)>();

    tokio::spawn(async move {
        while let Some((name, payload)) = event_rx.recv().await {
            println!("[EVENT] {name}: {payload}");
        }
    });

    let handle = VeloAgentBuilder::new(config.clone())
        .with_event_channel(event_tx.clone())
        .build()
        .await?;

    start_clipboard_observer(config, event_tx, handle.clone());

    println!("Velo is ready. Type a command and press Enter (or 'quit' to exit).");
    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin);

    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        let text = line.trim().to_string();
        if text.is_empty() {
            continue;
        }
        if text == "quit" || text == "exit" {
            break;
        }

        match handle.run(&text).await {
            Ok(response) => println!("{response}"),
            Err(e) => error!("Agent error: {e}"),
        }
    }

    info!("Velo shutting down");
    Ok(())
}

/// Spawn a background task that polls the OS clipboard every N ms.
/// When content changes, submits it to the agent for background analysis.
fn start_clipboard_observer(
    config: VeloConfig,
    event_tx: tokio::sync::mpsc::UnboundedSender<(String, serde_json::Value)>,
    _handle: velo_core::handle::VeloAgentHandle,
) {
    use arboard::Clipboard;
    use serde_json::json;
    use velo_core::events::SuggestionReady;

    let interval = Duration::from_millis(config.clipboard_poll_ms);

    tokio::spawn(async move {
        let mut last = String::new();

        loop {
            tokio::time::sleep(interval).await;

            let current = tokio::task::spawn_blocking(|| {
                Clipboard::new()
                    .and_then(|mut cb| cb.get_text())
                    .unwrap_or_default()
            })
            .await
            .unwrap_or_default();

            if current == last || current.trim().is_empty() {
                continue;
            }

            let lower = current.to_lowercase();
            let looks_like_error = lower.contains("error")
                || lower.contains("exception")
                || lower.contains("panic")
                || lower.contains("failed")
                || lower.contains("traceback")
                || lower.contains("segfault")
                || lower.contains("killed");

            if looks_like_error && current.len() > 64 {
                info!("Clipboard change detected — looks like an error log, priming suggestion");

                let snippet = &current[..current.len().min(120)];
                let suggestion = SuggestionReady {
                    id: uuid::Uuid::new_v4(),
                    headline: "Error detected in clipboard — click to analyze".into(),
                    body: format!(
                        "Velo detected an error log in your clipboard:\n\n```\n{snippet}\n```\n\nClick to let Velo diagnose and suggest a fix."
                    ),
                    trigger_snippet: snippet.to_string(),
                };

                let _ = event_tx.send((event_names::SUGGESTION_READY.into(), json!(suggestion)));
            }

            last = current;
        }
    });
}
