use super::super::test_support::*;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

const ACCEPTANCE_ORIGIN_ENV: &str = "LICOUP_ACCEPTANCE_BADTOWER_ORIGIN";
const ACCEPTANCE_RUNTIME_ROOT_ENV: &str = "LICOUP_ACCEPTANCE_RUNTIME_ROOT";
const ACCEPTANCE_RECEIPT_PATH_ENV: &str = "LICOUP_ACCEPTANCE_RUNTIME_RECEIPT_PATH";
const ACCEPTANCE_CANARY_ENV: &str = "LICOUP_ACCEPTANCE_PRIVATE_CANARY";

pub(super) struct AcceptanceRuntime {
    pub(super) origin: String,
    pub(super) root: PathBuf,
    pub(super) receipt_path: PathBuf,
    pub(super) private_canary: String,
}

impl AcceptanceRuntime {
    pub(super) fn from_environment() -> Result<Self> {
        let origin = required_environment_value(ACCEPTANCE_ORIGIN_ENV)?;
        ensure!(
            validated_station_base_url(&origin)? == origin
                && origin.starts_with("http://127.0.0.1:"),
            "BadTower acceptance origin is invalid"
        );
        let root = PathBuf::from(required_environment_value(ACCEPTANCE_RUNTIME_ROOT_ENV)?);
        ensure!(
            root.is_absolute() && root.is_dir(),
            "BadTower acceptance root is invalid"
        );
        let root = fs::canonicalize(root)?;
        let requested_receipt =
            PathBuf::from(required_environment_value(ACCEPTANCE_RECEIPT_PATH_ENV)?);
        ensure!(
            requested_receipt.is_absolute() && !requested_receipt.exists(),
            "BadTower acceptance receipt path is invalid"
        );
        let receipt_parent = requested_receipt
            .parent()
            .ok_or_else(|| anyhow!("BadTower acceptance receipt path is invalid"))?;
        ensure!(
            fs::canonicalize(receipt_parent)? == root
                && requested_receipt.file_name().and_then(|name| name.to_str())
                    == Some("runtime-receipt.json"),
            "BadTower acceptance receipt path is invalid"
        );
        let receipt_path = root.join("runtime-receipt.json");
        let private_canary = required_environment_value(ACCEPTANCE_CANARY_ENV)?;
        ensure!(
            private_canary.len() == 64
                && private_canary.as_bytes().iter().all(u8::is_ascii_hexdigit),
            "BadTower acceptance canary is invalid"
        );
        Ok(Self {
            origin,
            root,
            receipt_path,
            private_canary,
        })
    }

    pub(super) fn record_stage(&self, stage: &'static str) -> Result<()> {
        ensure!(
            stage
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || *byte == b'-'),
            "BadTower acceptance stage is invalid"
        );
        fs::write(self.root.join("acceptance-stage"), stage)?;
        Ok(())
    }
}

fn required_environment_value(name: &str) -> Result<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("BadTower acceptance input is missing"))
}

pub(super) struct FreshEndpoint {
    root: PathBuf,
    mobile_secret_store: Arc<EphemeralSecretStore>,
    pairwise_secret_store: Arc<EphemeralSecretStore>,
    config: Value,
    material: RuntimeSecretMaterial,
}

impl FreshEndpoint {
    pub(super) fn create(parent: &Path, directory_name: &str, origin: &str) -> Result<Self> {
        let root = parent.join(directory_name);
        fs::create_dir(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        let mut config = default_config();
        config["relayEnabled"] = json!(true);
        config["stationBaseUrl"] = json!(origin);
        Ok(Self {
            root,
            mobile_secret_store: Arc::new(EphemeralSecretStore::new()),
            pairwise_secret_store: Arc::new(EphemeralSecretStore::new()),
            config,
            material: RuntimeSecretMaterial::new(),
        })
    }

    pub(super) fn with_state<T>(
        &mut self,
        operation: impl FnOnce(&mut Value, &mut RuntimeSecretMaterial) -> Result<T>,
    ) -> Result<T> {
        let root = self.root.clone();
        let mobile_store: Arc<dyn SecureMeshSecretStore> = self.mobile_secret_store.clone();
        let pairwise_store: Arc<dyn SecureMeshSecretStore> = self.pairwise_secret_store.clone();
        let previous = set_portable_data_dir_override(Some(root.clone()));
        let result = with_mobile_relay_secret_store_override(mobile_store, || {
            with_pairwise_secret_store_override(pairwise_store, || {
                crate::domain::secure_mesh_command_runtime::with_secure_command_test_history_home(
                    &root,
                    || operation(&mut self.config, &mut self.material),
                )
            })
        });
        set_portable_data_dir_override(previous);
        result
    }

    pub(super) fn invoke<T>(&mut self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        self.with_state(|_, _| operation())
    }

    pub(super) fn persist(&mut self) -> Result<()> {
        self.with_state(|config, material| {
            let mut context = RuntimeSecretContext::default();
            std::mem::swap(&mut context.material, material);
            let result = save_config_with_runtime_secret_context(config, &mut context);
            std::mem::swap(&mut context.material, material);
            result
        })
    }

    pub(super) fn public_identity(&mut self) -> Result<(String, String)> {
        self.with_state(|config, material| {
            let endpoint = local_endpoint_state(config, material)?;
            Ok((endpoint.endpoint_id, endpoint.fingerprint))
        })
    }

    pub(super) fn is_isolated_from(&self, other: &Self) -> bool {
        let stores = [
            &self.mobile_secret_store,
            &self.pairwise_secret_store,
            &other.mobile_secret_store,
            &other.pairwise_secret_store,
        ];
        self.root != other.root
            && stores.iter().enumerate().all(|(index, store)| {
                stores
                    .iter()
                    .skip(index + 1)
                    .all(|other_store| !Arc::ptr_eq(store, other_store))
            })
    }

    pub(super) fn shares_kt_pin_with(&self, other: &Self) -> bool {
        let self_pin = self
            .config
            .get("secureMeshKeyTransparency")
            .and_then(|settings| settings.get("pin"));
        let other_pin = other
            .config
            .get("secureMeshKeyTransparency")
            .and_then(|settings| settings.get("pin"));
        self_pin.is_some() && self_pin == other_pin
    }

    pub(super) fn install_codex_history_fixture(&self, canary: &str) -> Result<()> {
        let codex_root = self.root.join(".codex");
        fs::create_dir(&codex_root)?;
        fs::write(
            codex_root.join("history.jsonl"),
            [
                json!({
                    "role": "user",
                    "content": canary,
                    "createdAt": "2026-07-29T00:00:00Z"
                })
                .to_string(),
                json!({
                    "role": "assistant",
                    "content": "isolated Lico Arc runtime fixture",
                    "createdAt": "2026-07-29T00:00:01Z"
                })
                .to_string(),
            ]
            .join("\n"),
        )?;
        Ok(())
    }
}

pub(super) fn pair_fresh_endpoints(
    pc: &mut FreshEndpoint,
    mobile: &mut FreshEndpoint,
    runtime: &AcceptanceRuntime,
) -> Result<()> {
    ensure!(
        pc.is_isolated_from(mobile),
        "BadTower acceptance endpoints do not have isolated state custody"
    );
    runtime.record_stage("pairing-secret")?;
    let shared_delivery_secret = random_base64url(MOBILE_RELAY_KEY_BYTES);
    pc.with_state(|_, material| {
        material.replace_e2ee_secret(
            MobileRelayE2eeSecretField::PairingSecret,
            SecretBytes::try_from_string(shared_delivery_secret.clone())?,
        )?;
        Ok(())
    })?;
    mobile.with_state(|_, material| {
        material.replace_e2ee_secret(
            MobileRelayE2eeSecretField::PairingSecret,
            SecretBytes::try_from_string(shared_delivery_secret)?,
        )?;
        Ok(())
    })?;

    runtime.record_stage("pairing-descriptors")?;
    let pc_descriptor = pc.with_state(|config, material| {
        ensure_mobile_relay_endpoint_descriptor(config, material, "desktop_sidecar")
    })?;
    let mobile_initial_descriptor = mobile.with_state(|config, material| {
        ensure_mobile_relay_endpoint_descriptor(config, material, "mobile")
    })?;
    ensure!(
        pc_descriptor["endpointId"] != mobile_initial_descriptor["endpointId"]
            && pc_descriptor["publicKeyBase64url"]
                != mobile_initial_descriptor["publicKeyBase64url"],
        "BadTower acceptance endpoint identities are not fresh"
    );
    ensure!(
        pc.shares_kt_pin_with(mobile),
        "BadTower acceptance endpoints do not share the test directory authority"
    );

    runtime.record_stage("pairing-mobile-intro")?;
    let mobile_intro_result = mobile.with_state(|config, material| {
        apply_peer_secure_mesh_descriptor(config, material, &pc_descriptor, true)
    });
    if let Err(error) = &mobile_intro_result {
        record_pairing_error(runtime, error)?;
    }
    mobile_intro_result?;
    let mobile_intro = mobile.with_state(|config, material| {
        ensure_mobile_relay_endpoint_descriptor(config, material, "mobile")
    })?;
    runtime.record_stage("pairing-pc-accept")?;
    pc.with_state(|config, material| {
        apply_peer_secure_mesh_descriptor(config, material, &mobile_intro, true)
    })?;
    let pc_accepted = pc.with_state(|config, material| {
        ensure_mobile_relay_endpoint_descriptor(config, material, "desktop_sidecar")
    })?;
    runtime.record_stage("pairing-mobile-finish")?;
    mobile.with_state(|config, material| {
        apply_peer_secure_mesh_descriptor(config, material, &pc_accepted, true)
    })?;
    let mobile_finished = mobile.with_state(|config, material| {
        ensure_mobile_relay_endpoint_descriptor(config, material, "mobile")
    })?;
    ensure!(
        mobile_finished["pairwiseFinished"].is_object(),
        "BadTower acceptance pairwise handshake did not finish"
    );
    runtime.record_stage("pairing-pc-finish")?;
    pc.with_state(|config, material| {
        apply_peer_secure_mesh_descriptor(config, material, &mobile_finished, true)
    })?;

    runtime.record_stage("pairing-key-confirm")?;
    let protected_confirmation = mobile.with_state(|config, material| {
        seal_mobile_relay_payload(
            config,
            material,
            crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
            &json!({"action": "acceptance_pairwise_key_confirmation"}),
        )
    })?;
    let opened_confirmation = pc.with_state(|config, material| {
        open_mobile_relay_payload(
            config,
            material,
            &protected_confirmation,
            crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
        )
    })?;
    ensure!(
        serde_json::from_slice::<Value>(&opened_confirmation)?["action"]
            == "acceptance_pairwise_key_confirmation",
        "BadTower acceptance pairwise key confirmation failed"
    );

    runtime.record_stage("pairing-persist")?;
    pc.persist()?;
    mobile.persist()?;
    Ok(())
}

fn record_pairing_error(runtime: &AcceptanceRuntime, error: &anyhow::Error) -> Result<()> {
    let detail = error.to_string().to_ascii_lowercase();
    let category = if detail.contains("capability") {
        "pairing-error-capability"
    } else if detail.contains("transparency") || detail.contains("directory") {
        "pairing-error-directory"
    } else if detail.contains("prekey") {
        "pairing-error-prekey"
    } else if detail.contains("pairwise") || detail.contains("handshake") {
        "pairing-error-pairwise"
    } else if detail.contains("secret") || detail.contains("custody") {
        "pairing-error-custody"
    } else if detail.contains("trust") || detail.contains("verified") {
        "pairing-error-trust"
    } else if detail.contains("missing field") {
        "pairing-error-missing-field"
    } else if detail.contains("does not contain") {
        "pairing-error-incomplete-descriptor"
    } else if detail.contains("mismatch") {
        "pairing-error-mismatch"
    } else if detail.contains("unsupported") {
        "pairing-error-unsupported"
    } else if detail.contains("unavailable") {
        "pairing-error-unavailable"
    } else if detail.contains("failed") {
        "pairing-error-failed"
    } else if detail.contains("database") || detail.contains("sqlite") {
        "pairing-error-database"
    } else if detail.contains("os error 2") {
        "pairing-error-not-found"
    } else if detail.contains("os error 13") {
        "pairing-error-permission"
    } else if detail.starts_with("mobile relay") {
        "pairing-error-mobile-relay"
    } else if detail.starts_with("secure mesh") {
        "pairing-error-secure-mesh"
    } else if detail.starts_with("private ") || detail.starts_with("path ") {
        "pairing-error-private-state"
    } else {
        "pairing-error-other"
    };
    runtime.record_stage(category)
}

pub(super) fn write_runtime_receipt(path: &Path, scenario: Value) -> Result<()> {
    let payload = json!({
        "schemaVersion": "licoup.licoarc-badtower.runtime-receipt.v1",
        "scenario": scenario
    });
    let encoded = serde_json::to_vec(&payload)?;
    ensure!(
        encoded.len() <= 4 * 1024,
        "BadTower acceptance receipt is oversized"
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    Ok(())
}
