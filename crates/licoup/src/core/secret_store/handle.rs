use anyhow::{Result, anyhow};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretStoreHandle {
    namespace: String,
    key: String,
}

impl SecretStoreHandle {
    pub fn new(namespace: impl Into<String>, key: impl Into<String>) -> Result<Self> {
        let namespace = namespace.into();
        let key = key.into();
        if namespace.trim().is_empty() || key.trim().is_empty() {
            return Err(anyhow!("secure mesh secret-store handle cannot be empty"));
        }
        if key.contains(':') {
            return Err(anyhow!(
                "secure mesh secret-store handle contains an invalid key separator"
            ));
        }
        Ok(Self { namespace, key })
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub(crate) fn account(&self) -> String {
        format!("{}:{}", self.namespace, self.key)
    }
}
