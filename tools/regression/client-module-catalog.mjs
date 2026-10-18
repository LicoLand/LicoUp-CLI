import { assembleClientModuleCatalog } from "./client-module-catalog/helpers.mjs";
import { CLIENT_MODULE_ID_ORDER } from "./client-module-catalog/order.mjs";
import { BRIDGE_PACKAGING_RELEASE_MODULES } from "./client-module-catalog/groups/bridge-packaging-release.mjs";
import { FLUTTER_MODULES } from "./client-module-catalog/groups/flutter.mjs";
import { REGRESSION_MODULES } from "./client-module-catalog/groups/regression.mjs";
import { RUST_CORE_MODULES } from "./client-module-catalog/groups/rust-core.mjs";
import { RUST_CATALOG_CONVERGENCE_MODULES } from "./client-module-catalog/groups/rust-catalog-convergence.mjs";
import { RUST_COMPONENT_MODULES } from "./client-module-catalog/groups/rust-components.mjs";
import { RUST_DOMAIN_MODULES } from "./client-module-catalog/groups/rust-domain.mjs";
import { RUST_PLATFORM_MODULES } from "./client-module-catalog/groups/rust-platform.mjs";

const CLIENT_MODULE_GROUPS = Object.freeze([
  REGRESSION_MODULES,
  FLUTTER_MODULES,
  RUST_CATALOG_CONVERGENCE_MODULES,
  RUST_COMPONENT_MODULES,
  RUST_DOMAIN_MODULES,
  RUST_CORE_MODULES,
  RUST_PLATFORM_MODULES,
  BRIDGE_PACKAGING_RELEASE_MODULES,
]);

export const CLIENT_MODULE_CATALOG = assembleClientModuleCatalog(
  CLIENT_MODULE_ID_ORDER,
  CLIENT_MODULE_GROUPS,
);

const moduleById = new Map(CLIENT_MODULE_CATALOG.map((module) => [module.id, module]));

export function clientModuleById(id) {
  return moduleById.get(id) ?? null;
}
