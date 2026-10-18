//! Single-process Private Gateway Runtime: the loopback LLM Gateway layer.

use crate::control::credentials::serve_credentials_control;
use crate::control::inventory::{
    control_socket_path as inventory_control_socket_path, load_inventory_overlay_if_present,
    overlay_path as inventory_overlay_path, serve_inventory_control,
};
use crate::http::server::serve_loopback;
use crate::model::llm_gateway::CompiledGateway;
use crate::usage::GatewayUsageRecorder;
use anyhow::{Result, anyhow};
use licoup_foundation::core::secret_bytes::SecretBytes;
use licoup_vault::api_key::GatewayCredentialSlot;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

pub struct GatewayServeArgs {
    pub listener: TcpListener,
    pub gateway: Arc<CompiledGateway>,
    pub credentials: Arc<GatewayCredentialSlot>,
    pub client_token: Arc<SecretBytes>,
    pub usage: Arc<GatewayUsageRecorder>,
    pub credentials_control: Option<PathBuf>,
}

/// Serve the LLM loopback layer until `stop` is set.
pub fn serve_gateway_runtime(args: GatewayServeArgs, stop: Arc<AtomicBool>) -> Result<()> {
    // Prefer the last hot-applied readiness overlay so soft-restart and boot
    // see the same verified set as an in-process inventory reload.
    if let Some(state_root) = args
        .credentials_control
        .as_ref()
        .and_then(|path| path.parent())
    {
        let _ = load_inventory_overlay_if_present(&inventory_overlay_path(state_root));
    }
    let http_stop = Arc::clone(&stop);
    let listener = args.listener;
    let gateway = Arc::clone(&args.gateway);
    let credentials = Arc::clone(&args.credentials);
    let client_token = Arc::clone(&args.client_token);
    let usage = Arc::clone(&args.usage);
    let http = thread::Builder::new()
        .name("gateway-llm".into())
        .spawn(move || {
            serve_loopback(
                listener,
                gateway,
                http_stop,
                credentials,
                client_token,
                usage,
            )
            .map_err(|error| anyhow!("llm_layer_failed:{error:?}"))
        })
        .map_err(|_| anyhow!("gateway_runtime_spawn_failed"))?;

    let inventory_socket = args
        .credentials_control
        .as_ref()
        .and_then(|path| path.parent().map(inventory_control_socket_path));

    let control = match args.credentials_control {
        Some(path) => {
            let control_stop = Arc::clone(&stop);
            let control_credentials = Arc::clone(&args.credentials);
            Some(
                thread::Builder::new()
                    .name("gateway-credentials-control".into())
                    .spawn(move || {
                        let _ = serve_credentials_control(path, control_credentials, control_stop);
                    })
                    .map_err(|_| anyhow!("gateway_runtime_spawn_failed"))?,
            )
        }
        None => None,
    };

    let inventory_control = match inventory_socket {
        Some(path) => {
            let control_stop = Arc::clone(&stop);
            Some(
                thread::Builder::new()
                    .name("gateway-inventory-control".into())
                    .spawn(move || {
                        let _ = serve_inventory_control(path, control_stop);
                    })
                    .map_err(|_| anyhow!("gateway_runtime_spawn_failed"))?,
            )
        }
        None => None,
    };

    // Block on the LLM layer (authoritative process lifetime).
    let http_result = http
        .join()
        .map_err(|_| anyhow!("gateway_runtime_join_failed"))?;
    stop.store(true, Ordering::SeqCst);
    if let Some(handle) = control {
        let _ = handle.join();
    }
    if let Some(handle) = inventory_control {
        let _ = handle.join();
    }
    http_result
}
