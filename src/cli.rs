use crate::{atspi_tree, diagnostics, screenshot, server, windows};
use anyhow::{Context, Result};

pub(crate) async fn run_from_env() -> Result<()> {
    diagnostics::hydrate_session_bus_env();

    match std::env::args().nth(1).as_deref() {
        Some("mcp") => server::serve_mcp().await,
        Some("doctor") => {
            let report = diagnostics::doctor_report();
            println!(
                "{}",
                serde_json::to_string_pretty(&report)
                    .context("failed to serialize doctor report")?
            );
            Ok(())
        }
        Some("apps") => {
            let apps = atspi_tree::list_accessible_apps(50).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&apps)
                    .context("failed to serialize accessible apps")?
            );
            Ok(())
        }
        Some("state") => {
            let app_name_or_bundle_identifier = std::env::args().nth(2);
            let (max_nodes, max_depth) = atspi_tree::snapshot_limits(None, None);
            let nodes = atspi_tree::snapshot_tree(
                app_name_or_bundle_identifier.as_deref(),
                None,
                max_nodes,
                max_depth,
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&nodes)
                    .context("failed to serialize accessibility tree")?
            );
            Ok(())
        }
        Some("screenshot") => {
            let capture = screenshot::capture_screenshot().await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "mime_type": capture.mime_type,
                    "source": capture.source,
                    "width": capture.width,
                    "height": capture.height,
                    "coordinate_width": capture.coordinate_width,
                    "coordinate_height": capture.coordinate_height,
                    "scale": capture.scale,
                    "resized": capture.resized,
                    "bytes": capture.bytes,
                    "original_bytes": capture.original_bytes,
                    "max_bytes": capture.max_bytes,
                    "format": capture.format,
                    "quality": capture.quality,
                    "data_url_length": capture.data_url.len()
                }))
                .context("failed to serialize screenshot report")?
            );
            Ok(())
        }
        Some("windows") => {
            let report = match windows::list_windows().await {
                Ok(windows) => {
                    let backend = windows
                        .first()
                        .map(|window| window.backend.as_str())
                        .unwrap_or(windows::I3_BACKEND);
                    serde_json::json!({
                        "backend": backend,
                        "windows": windows,
                        "error": null,
                        "permissions_hint": null,
                    })
                }
                Err(error) => {
                    let error = format!("{error:#}");
                    serde_json::json!({
                        "backend": "unavailable",
                        "windows": [],
                        "error": error,
                        "permissions_hint": windows::window_permission_hint(&error),
                    })
                }
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(command) => {
            anyhow::bail!(
                "unknown command '{command}'. Expected one of: mcp, doctor, apps, state, screenshot, windows"
            );
        }
        None => {
            print_help();
            Ok(())
        }
    }
}

fn print_help() {
    println!(
        "computer-use-linux\n\nUsage:\n  computer-use-linux mcp\n  computer-use-linux doctor\n  computer-use-linux apps\n  computer-use-linux state [APP_NAME]\n  computer-use-linux screenshot\n  computer-use-linux windows"
    );
}
