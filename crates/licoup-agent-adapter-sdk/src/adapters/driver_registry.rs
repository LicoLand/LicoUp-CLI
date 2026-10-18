//! Process-local registry shared by movable platform drivers.
//!
//! Driver values stay type-erased behind this module so the platform tree can
//! later move as one unit without introducing a crate-external singleton.

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

type RegistryValue = Box<dyn Any + Send + Sync>;
type RegistryKey = (&'static str, String);

static DRIVER_REGISTRY: OnceLock<Mutex<HashMap<RegistryKey, RegistryValue>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<RegistryKey, RegistryValue>> {
    DRIVER_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn registry_insert<T>(
    namespace: &'static str,
    key: &str,
    value: T,
    namespace_capacity: usize,
) -> Result<(), ()>
where
    T: Any + Send + Sync,
{
    let mut registry = registry().lock().map_err(|_| ())?;
    let namespaced_key = (namespace, key.to_owned());
    let namespace_len = registry
        .keys()
        .filter(|(registered_namespace, _)| *registered_namespace == namespace)
        .count();
    if namespace_len >= namespace_capacity && !registry.contains_key(&namespaced_key) {
        return Err(());
    }
    registry.insert(namespaced_key, Box::new(value));
    Ok(())
}

pub fn registry_get<T>(namespace: &'static str, key: &str) -> Option<T>
where
    T: Any + Clone + Send + Sync,
{
    registry()
        .lock()
        .ok()?
        .get(&(namespace, key.to_owned()))?
        .downcast_ref::<T>()
        .cloned()
}

/// Insert without replacing a concurrently registered value. The existing
/// typed value is returned to the caller so it can discard duplicate work.
pub fn registry_insert_if_absent<T>(
    namespace: &'static str,
    key: &str,
    value: T,
    namespace_capacity: usize,
) -> Result<Result<(), T>, ()>
where
    T: Any + Clone + Send + Sync,
{
    let mut registry = registry().lock().map_err(|_| ())?;
    let namespaced_key = (namespace, key.to_owned());
    if let Some(existing) = registry.get(&namespaced_key) {
        return existing.downcast_ref::<T>().cloned().map(Err).ok_or(());
    }
    let namespace_len = registry
        .keys()
        .filter(|(registered_namespace, _)| *registered_namespace == namespace)
        .count();
    if namespace_len >= namespace_capacity {
        return Err(());
    }
    registry.insert(namespaced_key, Box::new(value));
    Ok(Ok(()))
}

pub fn registry_remove<T>(namespace: &'static str, key: &str) -> Option<T>
where
    T: Any + Send + Sync,
{
    registry()
        .lock()
        .ok()?
        .remove(&(namespace, key.to_owned()))?
        .downcast::<T>()
        .ok()
        .map(|value| *value)
}

pub fn registry_remove_if<T>(
    namespace: &'static str,
    key: &str,
    predicate: impl FnOnce(&T) -> bool,
) -> bool
where
    T: Any + Send + Sync,
{
    let Ok(mut registry) = registry().lock() else {
        return false;
    };
    let namespaced_key = (namespace, key.to_owned());
    let should_remove = registry
        .get(&namespaced_key)
        .and_then(|value| value.downcast_ref::<T>())
        .is_some_and(predicate);
    if should_remove {
        registry.remove(&namespaced_key);
    }
    should_remove
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_keep_driver_values_isolated_and_bounded() {
        let namespace = "driver-registry-test";
        assert!(registry_insert(namespace, "one", 1_u32, 1).is_ok());
        assert_eq!(registry_get::<u32>(namespace, "one"), Some(1));
        assert_eq!(
            registry_insert_if_absent(namespace, "one", 9_u32, 1),
            Ok(Err(1))
        );
        assert!(registry_insert(namespace, "two", 2_u32, 1).is_err());
        assert!(registry_remove_if::<u32>(namespace, "one", |value| *value == 1));
        assert_eq!(registry_get::<u32>(namespace, "one"), None);
    }
}
