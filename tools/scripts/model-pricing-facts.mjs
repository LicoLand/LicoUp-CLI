#!/usr/bin/env node

/**
 * Read-only validator for the single maintained pricing catalog.
 *
 * The catalog is the checked-in authority. This module validates its exact
 * shape, route evidence, tier integrity, and release freshness without
 * creating another representation or mutating the repository.
 */

import {
  lstatSync,
  readFileSync,
} from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";

const REPO_ROOT = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const CATALOG_RELATIVE =
  "crates/licoup-model-catalog/src/pricing/pricing_catalog.json";
const MAX_JSON_BYTES = 16 * 1024 * 1024;
const EXPECTED_TABLE_COUNT = 10;
const MAX_ROUTES_PER_TABLE = 2_048;
const MAX_TIERS_PER_ROUTE = 64;
const MAX_CACHE_WRITE_RATES = 16;

const CATALOG_KEYS = Object.freeze(["last_updated", "providers", "agents"]);
const TABLE_KEYS = Object.freeze(["id", "unit", "routes"]);
const ROUTE_KEYS = Object.freeze([
  "model_id",
  "lifecycle",
  "verified_on",
  "source_urls",
  "billing_mode",
  "included_by_harness",
  "tiers",
]);
const LIFECYCLE_KEYS = Object.freeze(["status", "service_end"]);
const TIER_KEYS = Object.freeze([
  "default",
  "input",
  "cache_read",
  "cache_write",
  "output",
  "context_min",
  "context_max",
]);
const CACHE_WRITE_RATE_KEYS = Object.freeze(["ttl_seconds", "price"]);

export const PRICING_FACT_PATHS = Object.freeze({
  repoRoot: REPO_ROOT,
  catalog: path.join(REPO_ROOT, CATALOG_RELATIVE),
});

export class PricingFactsError extends Error {
  constructor(code, detail = "") {
    super(detail ? `${code}: ${detail}` : code);
    this.name = "PricingFactsError";
    this.code = code;
  }
}
function fail(code, detail = "") {
  throw new PricingFactsError(code, detail);
}

function exactKeys(value, expected, code) {
  if (!value || typeof value !== "object" || Array.isArray(value)) fail(code);
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length ||
      actual.some((key, index) => key !== wanted[index])) {
    fail(code);
  }
}

function nonEmptyString(value, code) {
  if (typeof value !== "string" || value.trim() !== value || value.length === 0) {
    fail(code);
  }
  return value;
}

function finiteNonNegative(value, code, { nullable = false } = {}) {
  if (nullable && value === null) return value;
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    fail(code);
  }
  return value;
}

function dateParts(value, code) {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}$/u.test(value)) fail(code);
  const [year, month, day] = value.split("-").map(Number);
  const date = new Date(Date.UTC(year, month - 1, day));
  if (date.getUTCFullYear() !== year ||
      date.getUTCMonth() !== month - 1 ||
      date.getUTCDate() !== day) {
    fail(code);
  }
  return { year, month, day };
}

function dayNumber(value) {
  const { year, month, day } = dateParts(value, "pricing_date_invalid");
  return Date.UTC(year, month - 1, day) / 86_400_000;
}

function readJson(filePath) {
  let info;
  try {
    info = lstatSync(filePath);
  } catch {
    fail("pricing_catalog_missing");
  }
  if (!info.isFile() || info.isSymbolicLink() || info.size > MAX_JSON_BYTES) {
    fail("pricing_catalog_invalid");
  }
  let text;
  try {
    text = readFileSync(filePath, "utf8");
  } catch {
    fail("pricing_catalog_unreadable");
  }
  try {
    return JSON.parse(text);
  } catch {
    fail("pricing_catalog_json_invalid");
  }
}

function validateLifecycle(lifecycle, lastUpdated) {
  exactKeys(lifecycle, LIFECYCLE_KEYS, "pricing_lifecycle_shape_invalid");
  if (lifecycle.status !== "active") fail("pricing_route_not_active");
  if (lifecycle.service_end !== null) {
    dateParts(lifecycle.service_end, "pricing_service_end_invalid");
    if (dayNumber(lifecycle.service_end) <= dayNumber(lastUpdated)) {
      fail("pricing_service_end_elapsed");
    }
  }
}

function tierBounds(tier) {
  const min = tier.context_min === null ? 0 : tier.context_min;
  const max = tier.context_max === null ? Number.POSITIVE_INFINITY : tier.context_max;
  if (min > max) fail("pricing_context_bounds_invalid");
  return { min, max };
}

function rangesOverlap(left, right) {
  return left.min <= right.max && right.min <= left.max;
}

function validateCacheWrite(value) {
  if (value === null || typeof value === "number") {
    finiteNonNegative(value, "pricing_price_invalid", { nullable: true });
    return;
  }
  if (!Array.isArray(value) || value.length === 0 || value.length > MAX_CACHE_WRITE_RATES) {
    fail("pricing_cache_write_invalid");
  }
  let previousTtl = 0;
  for (const rate of value) {
    exactKeys(rate, CACHE_WRITE_RATE_KEYS, "pricing_cache_write_rate_shape_invalid");
    if (!Number.isSafeInteger(rate.ttl_seconds) || rate.ttl_seconds <= previousTtl) {
      fail("pricing_cache_write_ttl_invalid");
    }
    finiteNonNegative(rate.price, "pricing_price_invalid");
    previousTtl = rate.ttl_seconds;
  }
}

function validateTier(tier) {
  exactKeys(tier, TIER_KEYS, "pricing_tier_shape_invalid");
  if (typeof tier.default !== "boolean") fail("pricing_tier_default_invalid");
  finiteNonNegative(tier.input, "pricing_price_invalid");
  finiteNonNegative(tier.cache_read, "pricing_price_invalid", { nullable: true });
  validateCacheWrite(tier.cache_write);
  finiteNonNegative(tier.output, "pricing_price_invalid");
  for (const bound of [tier.context_min, tier.context_max]) {
    if (bound !== null &&
        (!Number.isSafeInteger(bound) || bound < 0)) {
      fail("pricing_context_bounds_invalid");
    }
  }
  return tierBounds(tier);
}

function validateRoute(route, lastUpdated) {
  exactKeys(route, ROUTE_KEYS, "pricing_route_shape_invalid");
  nonEmptyString(route.model_id, "pricing_model_id_invalid");
  validateLifecycle(route.lifecycle, lastUpdated);
  dateParts(route.verified_on, "pricing_verification_date_invalid");
  if (dayNumber(route.verified_on) > dayNumber(lastUpdated)) {
    fail("pricing_verification_future");
  }
  if (!Array.isArray(route.source_urls) || route.source_urls.length === 0) {
    fail("pricing_sources_missing");
  }
  const sources = new Set();
  for (const source of route.source_urls) {
    nonEmptyString(source, "pricing_source_invalid");
    if (sources.has(source)) fail("pricing_source_duplicate");
    sources.add(source);
    let parsed;
    try {
      parsed = new URL(source);
    } catch {
      fail("pricing_source_invalid");
    }
    if (parsed.protocol !== "https:" || parsed.username || parsed.password) {
      fail("pricing_source_not_official_https");
    }
  }
  nonEmptyString(route.billing_mode, "pricing_billing_mode_invalid");
  if (typeof route.included_by_harness !== "boolean") {
    fail("pricing_harness_flag_invalid");
  }
  if (!Array.isArray(route.tiers) || route.tiers.length === 0) {
    fail("pricing_tiers_missing");
  }
  if (route.tiers.length > MAX_TIERS_PER_ROUTE) fail("pricing_tiers_too_many");
  const defaults = route.tiers.filter((tier) => tier.default === true);
  if (defaults.length !== 1) fail("pricing_default_tier_ambiguous");
  const bounds = route.tiers.map(validateTier);
  for (let left = 0; left < bounds.length; left += 1) {
    for (let right = left + 1; right < bounds.length; right += 1) {
      if (rangesOverlap(bounds[left], bounds[right])) {
        fail("pricing_context_bounds_overlap");
      }
    }
  }
  return route;
}

function validateTable(table, role, lastUpdated, tableIds, providerModels) {
  exactKeys(table, TABLE_KEYS, "pricing_table_shape_invalid");
  nonEmptyString(table.id, "pricing_table_id_invalid");
  nonEmptyString(table.unit, "pricing_unit_invalid");
  if (tableIds.has(table.id)) fail("pricing_table_duplicate");
  tableIds.add(table.id);
  if (!Array.isArray(table.routes) || table.routes.length === 0 ||
      table.routes.length > MAX_ROUTES_PER_TABLE) {
    fail("pricing_routes_incomplete");
  }
  const routeIds = new Set();
  const routes = [];
  for (const route of table.routes) {
    const valid = validateRoute(route, lastUpdated);
    if (routeIds.has(valid.model_id)) fail("pricing_route_duplicate");
    routeIds.add(valid.model_id);
    if (role === "provider") {
      if (providerModels.has(valid.model_id)) {
        fail("pricing_raw_provider_duplicate");
      }
      providerModels.add(valid.model_id);
    }
    routes.push(valid);
  }
  return Object.freeze({
    role,
    id: table.id,
    unit: table.unit,
    routes: Object.freeze(routes),
  });
}

export function validateCatalog(catalog) {
  exactKeys(catalog, CATALOG_KEYS, "pricing_catalog_shape_invalid");
  const lastUpdated = nonEmptyString(catalog.last_updated, "pricing_catalog_date_invalid");
  dateParts(lastUpdated, "pricing_catalog_date_invalid");
  if (!Array.isArray(catalog.providers) || !Array.isArray(catalog.agents) ||
      catalog.providers.length === 0 || catalog.agents.length === 0) {
    fail("pricing_tables_incomplete");
  }
  if (catalog.providers.length + catalog.agents.length !== EXPECTED_TABLE_COUNT) {
    fail("pricing_table_count_invalid");
  }
  const tableIds = new Set();
  const providerModels = new Set();
  const tables = [
    ...catalog.providers.map((table) =>
      validateTable(table, "provider", lastUpdated, tableIds, providerModels)),
    ...catalog.agents.map((table) =>
      validateTable(table, "agent", lastUpdated, tableIds, providerModels)),
  ];
  return Object.freeze({
    catalog,
    lastUpdated,
    tables: Object.freeze(tables),
    tableCount: tables.length,
    routeCount: tables.reduce((count, table) => count + table.routes.length, 0),
  });
}

export function loadCatalog(root = REPO_ROOT) {
  return readJson(path.join(root, CATALOG_RELATIVE));
}

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

function expectRejected(label, code, operation) {
  try {
    operation();
  } catch (error) {
    if (error instanceof PricingFactsError && error.code === code) return;
    throw error;
  }
  fail(`pricing_self_test_${label}`);
}

export function runSelfTest() {
  const source = loadCatalog(REPO_ROOT);
  const checked = validateCatalog(source);
  if (checked.tableCount !== EXPECTED_TABLE_COUNT || checked.routeCount !== 92) {
    fail("pricing_self_test_counts");
  }
  if (checked.lastUpdated !== "2026-08-22") fail("pricing_self_test_date");
  const cloneCatalog = () => clone(source);
  const cases = [
    ["unknown_key", "pricing_catalog_shape_invalid", () => {
      const value = cloneCatalog();
      value.unknown = true;
      validateCatalog(value);
    }],
    ["missing_source", "pricing_sources_missing", () => {
      const value = cloneCatalog();
      value.providers[0].routes[0].source_urls = [];
      validateCatalog(value);
    }],
    ["negative_price", "pricing_price_invalid", () => {
      const value = cloneCatalog();
      value.providers[0].routes[0].tiers[0].input = -1;
      validateCatalog(value);
    }],
    ["duplicate_cache_write_ttl", "pricing_cache_write_ttl_invalid", () => {
      const value = cloneCatalog();
      const route = value.providers.find((table) => table.id === "anthropic").routes[0];
      route.tiers[0].cache_write[1].ttl_seconds = 300;
      validateCatalog(value);
    }],
    ["ambiguous_default", "pricing_default_tier_ambiguous", () => {
      const value = cloneCatalog();
      const route = value.providers.find((table) => table.id === "openai").routes[0];
      route.tiers.push(clone(route.tiers[0]));
      validateCatalog(value);
    }],
    ["non_active", "pricing_route_not_active", () => {
      const value = cloneCatalog();
      value.providers[0].routes[0].lifecycle.status = "retired";
      validateCatalog(value);
    }],
    ["service_end", "pricing_service_end_elapsed", () => {
      const value = cloneCatalog();
      value.providers[0].routes[0].lifecycle.service_end = value.last_updated;
      validateCatalog(value);
    }],
    ["future_verification", "pricing_verification_future", () => {
      const value = cloneCatalog();
      value.providers[0].routes[0].verified_on = "2099-01-01";
      validateCatalog(value);
    }],
    ["duplicate_raw_owner", "pricing_raw_provider_duplicate", () => {
      const value = cloneCatalog();
      value.providers[1].routes[0].model_id = value.providers[0].routes[0].model_id;
      validateCatalog(value);
    }],
  ];
  for (const [label, code, operation] of cases) {
    expectRejected(label, code, operation);
  }
  return Object.freeze({
    ok: true,
    caseCount: cases.length,
    tableCount: checked.tableCount,
    routeCount: checked.routeCount,
    deterministic: true,
  });
}

function main(argv) {
  if (argv.length > 1) fail("pricing_cli_arguments_invalid");
  const mode = argv[0] || "check";
  if (mode === "check") {
    const checked = validateCatalog(loadCatalog());
    process.stdout.write(`${JSON.stringify({
      ok: true,
      tableCount: checked.tableCount,
      routeCount: checked.routeCount,
    })}\n`);
    return;
  }
  if (mode === "self-test") {
    process.stdout.write(`${JSON.stringify(runSelfTest())}\n`);
    return;
  }
  fail("pricing_cli_mode_invalid");
}

if (process.argv[1] &&
    import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`${JSON.stringify({
      ok: false,
      code: error instanceof PricingFactsError ? error.code : "pricing_catalog_invalid",
      privateDataIncluded: false,
    })}\n`);
    process.exitCode = 1;
  }
}
