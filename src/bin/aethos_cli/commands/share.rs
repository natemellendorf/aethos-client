use std::fs;
use std::path::Path;

use clap::{Args, Subcommand};
use image::{ImageBuffer, Luma, Rgba, RgbaImage};
use qrcode::QrCode;
use serde_json::json;

#[derive(Debug, Args)]
pub struct ShareArgs {
    #[command(subcommand)]
    pub cmd: ShareCmd,
}

#[derive(Debug, Subcommand)]
pub enum ShareCmd {
    Id,
    Qr {
        #[arg(long)]
        output: String,
    },
}

pub fn run(args: &ShareArgs, state: &crate::state::CliState) -> Result<(), String> {
    let (event_type, data) = execute(args, state)?;
    crate::output::emit_success(&event_type, data);
    Ok(())
}

fn generate_qr_png(wayfarer_id: &str, output_path: &Path) -> Result<(), String> {
    let code = QrCode::new(wayfarer_id.as_bytes())
        .map_err(|err| format!("failed generating QR payload: {err}"))?;

    let scale: u32 = 8;
    let border: u32 = 4;
    let luma: ImageBuffer<Luma<u8>, Vec<u8>> = code
        .render::<Luma<u8>>()
        .quiet_zone(false)
        .module_dimensions(scale, scale)
        .build();

    let inner_w = luma.width();
    let inner_h = luma.height();
    let width = inner_w + border * scale * 2;
    let height = inner_h + border * scale * 2;
    let mut rgba = RgbaImage::from_pixel(width, height, Rgba([255, 255, 255, 255]));

    for y in 0..inner_h {
        for x in 0..inner_w {
            let px = luma.get_pixel(x, y).0[0];
            let color = if px < 128 {
                Rgba([16, 18, 28, 255])
            } else {
                Rgba([255, 255, 255, 255])
            };
            rgba.put_pixel(x + border * scale, y + border * scale, color);
        }
    }

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("failed creating output dir {}: {err}", parent.display()))?;
        }
    }

    rgba.save(output_path)
        .map_err(|err| format!("failed saving QR image at {}: {err}", output_path.display()))?;

    Ok(())
}

fn execute(
    args: &ShareArgs,
    _state: &crate::state::CliState,
) -> Result<(String, serde_json::Value), String> {
    match &args.cmd {
        ShareCmd::Id => {
            let identity = crate::aethos_core::identity_store::ensure_local_identity()?;
            Ok((
                "share_id".to_string(),
                json!({ "wayfarer_id": identity.wayfarer_id }),
            ))
        }
        ShareCmd::Qr { output } => {
            let identity = crate::aethos_core::identity_store::ensure_local_identity()?;
            let wayfarer_id = identity.wayfarer_id;
            let output_path = std::path::PathBuf::from(output);
            generate_qr_png(&wayfarer_id, &output_path)?;
            Ok((
                "share_qr".to_string(),
                json!({
                    "wayfarer_id": wayfarer_id,
                    "output_path": output_path.display().to_string(),
                }),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{execute, ShareArgs, ShareCmd};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "aethos-cli-share-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn test_state(base_dir: &std::path::Path) -> crate::state::CliState {
        crate::state::CliState::from_cli_args(base_dir.to_str(), None, false)
    }

    #[test]
    fn share_id_returns_wayfarer_id() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("id");
        let state = test_state(&base_dir);
        state.setup_env();

        let args = ShareArgs { cmd: ShareCmd::Id };
        let (event_type, data) = execute(&args, &state).expect("share id");

        assert_eq!(event_type, "share_id");
        assert!(data["wayfarer_id"].as_str().is_some_and(|v| !v.is_empty()));

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn share_qr_generates_png_file() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("qr");
        let state = test_state(&base_dir);
        state.setup_env();

        let output_path = base_dir.join("share.png");
        let args = ShareArgs {
            cmd: ShareCmd::Qr {
                output: output_path.to_string_lossy().to_string(),
            },
        };
        let (event_type, data) = execute(&args, &state).expect("share qr");

        assert_eq!(event_type, "share_qr");
        assert!(data["wayfarer_id"].as_str().is_some_and(|v| !v.is_empty()));
        assert!(output_path.exists(), "PNG file should exist");

        let _ = std::fs::remove_dir_all(&base_dir);
    }
}
