import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import Ajv2020 from "ajv/dist/2020.js";

const schema = JSON.parse(fs.readFileSync(
  new URL("../../../schemas/extensions/ui.schema.json", import.meta.url),
  "utf8",
));

// The same directory publishes the package manifest, whose data-package
// category and typed resources are the other half of this contract. It is
// validated here as well, because the native tests pin the code against the
// schema while only this test runs the published schema itself.
const manifestSchema = JSON.parse(fs.readFileSync(
  new URL("../../../schemas/extensions/manifest.schema.json", import.meta.url),
  "utf8",
));

function validator() {
  return new Ajv2020({ strict: true, allErrors: true }).compile(schema);
}

function manifestValidator() {
  return new Ajv2020({ strict: true, allErrors: true }).compile(manifestSchema);
}

const base = {
  schema: "licoup.ui-contribution.v1",
  id: "example.ui.panel",
  title: "Synthetic panel",
};

// A synthetic data package: no program, six typed resources, and the host
// requirements its composition binds.
const dataPackage = {
  schema: "licoup.extension-package.v1",
  id: "org.licoland.appearance.synthetic",
  version: "1.0.0",
  displayName: "Synthetic appearance",
  hostProtocol: { major: 1, minimumMinor: 0 },
  compatibility: { clientVersions: [">=0.1.0, <1.0.0"] },
  profiles: [],
  runtime: { mode: "data" },
  activation: "on-demand",
  requires: [],
  optionalRequires: [],
  permissions: [],
  hostPrimitives: ["text", "action"],
  hostActions: ["org.licoland.action.apply-appearance"],
  resources: [
    { kind: "theme", id: "org.licoland.theme.synthetic", definition: "themes/synthetic.json", format: "licoup.data.theme.v1", tokens: ["org.licoland.token.surface"] },
    { kind: "layout", id: "org.licoland.layout.synthetic", definition: "layouts/synthetic.json", format: "licoup.data.layout.v1", regions: ["org.licoland.region.sidebar"] },
    { kind: "style", id: "org.licoland.style.synthetic", definition: "styles/synthetic.json", format: "licoup.data.style.v1", targets: ["org.licoland.target.button"] },
    { kind: "font", id: "org.licoland.font.synthetic", definition: "fonts/synthetic.json", format: "licoup.data.font.v1", families: ["Inter", "Noto Sans SC"] },
    { kind: "language", id: "org.licoland.language.synthetic", definition: "strings/synthetic.json", format: "licoup.data.language.v1", locales: ["zh", "zh-CN"] },
    {
      kind: "composition",
      id: "org.licoland.composition.synthetic",
      definition: "compositions/synthetic.json",
      format: "licoup.data.composition.v1",
      components: [
        { component: "org.licoland.component.status", primitive: "text" },
        { component: "org.licoland.component.apply", primitive: "action", actionRef: "org.licoland.action.apply-appearance" },
      ],
    },
  ],
};

test("the published UI schema compiles in strict mode", () => {
  assert.equal(typeof validator(), "function");
});

test("existing primitives and versioned resource views remain declarable", () => {
  const validate = validator();
  for (const contribution of [
    { ...base, kind: "settings", fields: [{ id: "auth", label: "Credential", type: "secret-ref" }] },
    { ...base, kind: "command", actionRef: "example.action.run" },
    { ...base, kind: "navigation" },
    { ...base, kind: "metric-panel", series: [{ metric: "example.metric.latency", label: "Latency", unit: "ms" }] },
    { ...base, kind: "resource-view", resourceRef: "example.resource.graph", resourceFormat: "licoup.ui.graph-resource.v1" },
    // Unknown formats remain data; actual host capability decides mounting.
    { ...base, kind: "resource-view", resourceFormat: "example.future-format.v2" },
  ]) assert.equal(validate(contribution), true, JSON.stringify(validate.errors));
});

test("only resource views may request a resource format", () => {
  const validate = validator();
  for (const kind of ["settings", "command", "navigation", "metric-panel"]) {
    const contribution = {
      ...base, kind, resourceFormat: "licoup.ui.graph-resource.v1",
      ...(kind === "metric-panel" ? {
        series: [{ metric: "example.metric.latency", label: "Latency", unit: "ms" }],
      } : {}),
    };
    assert.equal(validate(contribution), false, `${kind} accepted a resource format`);
  }
  assert.equal(validate({ ...base, kind: "resource-view", resourceFormat: "unnamespaced" }), false);
});

test("secret values, executable fields and invalid metric shapes are refused", () => {
  const validate = validator();
  for (const contribution of [
    { ...base, kind: "settings", fields: [{ id: "auth", label: "Credential", type: "secret-ref", value: "synthetic-value" }] },
    { ...base, kind: "resource-view", handler: "synthetic-handler" },
    { ...base, kind: "metric-panel" },
    { ...base, kind: "metric-panel", series: [] },
    { ...base, kind: "metric-panel", series: {} },
    { ...base, kind: "command", series: [{ metric: "example.metric.latency", label: "Latency", unit: "ms" }] },
  ]) assert.equal(validate(contribution), false, JSON.stringify(contribution));
});

test("the published package manifest compiles in strict mode", () => {
  assert.equal(typeof manifestValidator(), "function");
});

test("a data package with every typed resource kind is admitted", () => {
  const validate = manifestValidator();
  assert.equal(validate(dataPackage), true, JSON.stringify(validate.errors));
  // A data package that declares no resource is still a package: it is the
  // aggregate case, and nothing about it is executable.
  assert.equal(validate({ ...dataPackage, resources: undefined, hostPrimitives: undefined }), true,
    JSON.stringify(validate.errors));
});

test("an executable declaration or a profile on a data package is refused", () => {
  const validate = manifestValidator();
  for (const runtime of [
    { mode: "data", entry: "agent.py" },
    { mode: "data", runtimeRef: "user:python3" },
    { mode: "data", descriptor: "adapter.json" },
    { mode: "data", endpointRef: "endpoint.example" },
  ]) {
    assert.equal(validate({ ...dataPackage, runtime }), false, JSON.stringify(runtime));
  }
  // A program may not carry typed appearance resources either.
  assert.equal(validate({
    ...dataPackage,
    runtime: { mode: "process", entry: "agent.py" },
    profiles: [{ id: "declarative-ui", major: 1 }],
  }), false);
  // A data package serves no method profile.
  assert.equal(validate({
    ...dataPackage,
    profiles: [{ id: "declarative-ui", major: 1 }],
    runtime: { mode: "data" },
  }), false);
});

test("unknown resource kinds, shapes and host primitives are refused by the schema", () => {
  const validate = manifestValidator();
  const theme = dataPackage.resources[0];
  for (const resources of [
    [{ kind: "widget", id: "org.licoland.widget.card", definition: "widgets/card.json", format: "licoup.data.widget.v1", components: [] }],
    [{ ...theme, format: "licoup.data.theme.v2" }],
    [{ ...theme, tokens: [] }],
    [{ ...theme, tokens: ["surface"] }],
    [{ ...theme, definition: "/etc/passwd" }],
    [{ ...theme, definition: "../outside.json" }],
    [theme, theme],
  ]) {
    assert.equal(validate({ ...dataPackage, resources }), false, JSON.stringify(resources));
  }
  // Two declarations of one identity are one resource declared twice. The
  // schema decides the identical case and publishes the identity rule; the
  // native contract decides identity, whatever the declarations say.
  assert.match(
    manifestSchema.properties.resources.description,
    /one identity are one resource declared twice/u,
  );
  assert.equal(validate({ ...dataPackage, hostPrimitives: ["canvas"] }), false);
  assert.equal(validate({ ...dataPackage, hostPrimitives: ["text", "text"] }), false);
  assert.equal(validate({ ...dataPackage, hostActions: ["eval"] }), false);
  // A resource kind keeps exactly its published fields.
  assert.equal(validate({
    ...dataPackage,
    resources: [{ ...theme, regions: ["org.licoland.region.sidebar"] }],
  }), false);
  // A composition may bind only a declared primitive, and the declaration is
  // the set the host publishes.
  assert.equal(validate({
    ...dataPackage,
    resources: [{
      ...dataPackage.resources[5],
      components: [{ component: "org.licoland.component.chart", primitive: "chart" }],
    }],
  }), true, JSON.stringify(validate.errors));
  assert.equal(validate({
    ...dataPackage,
    resources: [{
      ...dataPackage.resources[5],
      components: [{ component: "org.licoland.component.chart", primitive: "canvas" }],
    }],
  }), false);
});

// The committed converter sample under `samples/state-converter/` is the
// developer example this contract is written for. Its manifest is a document on
// disk rather than an object built here, so the schema is run against what a
// package author would copy — and each shape the schema must refuse is produced
// by mutating that same document.
const sampleDirectory = new URL(
  "../../../crates/licoup-extension-contracts/samples/state-converter/",
  import.meta.url,
);

function sampleManifest() {
  return JSON.parse(fs.readFileSync(new URL("manifest.json", sampleDirectory), "utf8"));
}

test("the committed converter sample is admitted by the published manifest schema", () => {
  const validate = manifestValidator();
  const manifest = sampleManifest();
  assert.equal(validate(manifest), true, JSON.stringify(validate.errors));

  // The conversion block the sample ships is the block the schema publishes:
  // same fields, same required set, one kind. A field added on either side
  // without the other fails here.
  const conversion = manifestSchema.properties.conversion;
  assert.deepEqual(
    Object.keys(manifest.conversion).sort(),
    Object.keys(conversion.properties).sort(),
  );
  assert.deepEqual(
    [...conversion.required].sort(),
    Object.keys(conversion.properties).sort(),
  );
  assert.equal(conversion.properties.kind.const, manifest.conversion.kind);

  // The program the host starts and the program the conversion runs are one
  // program at one path, declared inside the payload. A converter that borrowed
  // an interpreter is not the kind this contract publishes, so the sample
  // declares no runtime reference at all.
  assert.equal(manifest.runtime.entry, manifest.conversion.entry);
  assert.equal(manifest.conversion.kind, "native-executable");
  assert.equal(Object.hasOwn(manifest.runtime, "runtimeRef"), false);
  assert.match(manifest.conversion.entry, /\//u);
});

test("the published schema refuses every shape a conversion declaration must not take", () => {
  const validate = manifestValidator();
  const manifest = sampleManifest();
  const { conversion } = manifest;
  assert.equal(validate(manifest), true, "the unmutated document is the control");

  for (const mutated of [
    { ...manifest, conversion: { ...conversion, kind: "interpreter-carried" } },
    { ...manifest, conversion: { ...conversion, entry: "converter" } },
    { ...manifest, conversion: { ...conversion, entry: "/bin/converter" } },
    { ...manifest, conversion: { ...conversion, entry: "../bin/converter" } },
    { ...manifest, conversion: { ...conversion, entry: "bin//converter" } },
    { ...manifest, conversion: { ...conversion, entry: "bin\\converter" } },
    { ...manifest, conversion: { ...conversion, sourceFormats: [] } },
    {
      ...manifest,
      conversion: {
        ...conversion,
        sourceFormats: [...conversion.sourceFormats, conversion.sourceFormats[0]],
      },
    },
    {
      ...manifest,
      conversion: {
        ...conversion,
        sourceFormats: Array.from({ length: 9 }, (unused, index) => `licoup-state-0.0.${index}`),
      },
    },
    { ...manifest, conversion: { ...conversion, targetFormat: "Licoup-State-0.3.0" } },
    { ...manifest, conversion: { ...conversion, sources: conversion.sourceFormats } },
    { ...manifest, conversionRequired: conversion },
    { ...manifest, compatibility: { clientVersions: [] } },
    { ...manifest, permissions: [{ scope: "self" }] },
    { ...manifest, runtime: { mode: "process" } },
  ]) {
    assert.equal(validate(mutated), false, `the schema admitted ${JSON.stringify(mutated)}`);
  }

  // Restoring the document restores admission: what the previous assertions
  // observed is the mutation, not a validator that refuses everything.
  assert.equal(validate(sampleManifest()), true, JSON.stringify(validate.errors));
});
