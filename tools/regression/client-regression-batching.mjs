import os from "node:os";

function stableUnique(values) {
  return [...new Set(values)];
}

function commandKey(command) {
  return JSON.stringify([command.program, command.cwd, command.args]);
}

function rustBatchShape(module) {
  const { args } = module.command;
  if (module.command.program !== "cargo" || args[0] !== "test") return null;
  const separator = args.indexOf("--");
  if (separator >= 0) return null;
  let filterIndex = -1;
  const lib = args.indexOf("--lib");
  const integration = args.indexOf("--test");
  const binary = args.indexOf("--bin");
  if (lib >= 0 && args.length > lib + 1) filterIndex = lib + 1;
  else if (integration >= 0 && args.length > integration + 2) filterIndex = integration + 2;
  else if (binary >= 0 && args.length > binary + 2) filterIndex = binary + 2;
  if (filterIndex < 0 || filterIndex !== args.length - 1) return null;
  return Object.freeze({
    key: JSON.stringify([module.command.program, module.command.cwd, args.slice(0, filterIndex)]),
    broadArgs: Object.freeze(args.slice(0, filterIndex)),
    filter: args[filterIndex],
  });
}

function nodeTestShape(module) {
  const { args } = module.command;
  if (module.command.program !== "node" || args[0] !== "--test") return null;
  if (args.slice(1).some((value) => value.startsWith("-"))) return null;
  return Object.freeze({
    key: JSON.stringify([module.command.cwd, "node-test"]),
    paths: Object.freeze(args.slice(1)),
  });
}

function flutterTestShape(module) {
  const { args } = module.command;
  if (module.command.program !== "node" || args[0] !== "tools/scripts/client-toolchain-runner.mjs") return null;
  const separator = args.indexOf("--");
  if (separator < 0 || args[separator + 1] !== "flutter" || args[separator + 2] !== "test") return null;
  const tail = args.slice(separator + 3);
  const nameIndex = tail.indexOf("--name");
  const optionStart = nameIndex >= 0 ? nameIndex : tail.length;
  const paths = tail.slice(0, optionStart).filter((value) => !value.startsWith("--"));
  if (paths.length === 0) return null;
  const options = [
    ...tail.slice(0, optionStart).filter((value) => value.startsWith("--")),
    ...tail.slice(optionStart),
  ];
  return Object.freeze({
    key: JSON.stringify([module.command.cwd, args.slice(0, separator + 3), options]),
    prefix: Object.freeze(args.slice(0, separator + 3)),
    options: Object.freeze(options),
    paths: Object.freeze(paths),
  });
}

function gradleTestShape(module) {
  const { args } = module.command;
  if (module.command.program !== "node" || args[0] !== "tools/scripts/client-toolchain-runner.mjs") return null;
  const separator = args.indexOf("--");
  if (separator < 0 || !["./gradlew", "gradlew.bat"].includes(args[separator + 1])) return null;
  const inner = args.slice(separator + 2);
  if (inner[0] !== ":app:testDebugUnitTest") return null;
  const filters = [];
  const options = [];
  for (let index = 1; index < inner.length; index += 1) {
    if (inner[index] === "--tests" && inner[index + 1]) {
      filters.push(inner[index + 1]);
      index += 1;
    } else {
      options.push(inner[index]);
    }
  }
  if (filters.length === 0) return null;
  return Object.freeze({
    key: JSON.stringify([module.command.cwd, args.slice(0, separator + 2), options]),
    prefix: Object.freeze(args.slice(0, separator + 2)),
    options: Object.freeze(options),
    filters: Object.freeze(filters),
  });
}

function makeBatch(id, members, command, attribution = "group", {
  inputOwners = null,
  internalConcurrency = null,
  weight = Math.max(...members.map((member) => member.regression.weight)),
} = {}) {
  return Object.freeze({
    id,
    stage: members[0].regression.stage,
    lane: members[0].regression.lane,
    toolchain: members[0].regression.toolchain,
    weight,
    internalConcurrency,
    inputOwners: inputOwners
      ? Object.freeze(inputOwners.map((owner) => Object.freeze({
        member: owner.member,
        indexes: Object.freeze([...owner.indexes]),
      })))
      : null,
    resources: Object.freeze(stableUnique(members.flatMap((member) => member.regression.resources))),
    members: Object.freeze(members.map((member) => member.id)),
    command: Object.freeze({ ...command, args: Object.freeze([...command.args]) }),
    attribution,
  });
}

export function planClientRegressionBatches(selected, {
  catalog = selected,
  excludedCatalog = [],
  availableParallelism = os.availableParallelism(),
  narrow = false,
} = {}) {
  if (narrow) {
    return Object.freeze(selected.map((module, index) => makeBatch(
      `retry-exact-${index + 1}`,
      [module],
      module.command,
      "exact",
      {
        internalConcurrency: module.regression.internalParallelism
          ? module.regression.weight
          : null,
      },
    )));
  }
  const selectedIds = new Set(selected.map((module) => module.id));
  const catalogRustGroups = new Map();
  for (const module of catalog) {
    const shape = rustBatchShape(module);
    if (!shape) continue;
    const key = `${module.regression.stage}:${shape.key}`;
    const group = catalogRustGroups.get(key) || [];
    group.push(module);
    catalogRustGroups.set(key, group);
  }
  const excludedRustFilters = new Map();
  for (const module of excludedCatalog) {
    const shape = rustBatchShape(module);
    if (!shape) continue;
    const key = `${module.regression.stage}:${shape.key}`;
    const filters = excludedRustFilters.get(key) || new Set();
    filters.add(shape.filter);
    excludedRustFilters.set(key, filters);
  }

  const consumed = new Set();
  const batches = [];
  const append = (batch) => {
    batches.push(batch);
    for (const id of batch.members) consumed.add(id);
  };

  // A broad Rust target invocation is allowed only when the complete
  // host-applicable target group is selected. Exclude foreign-platform filters;
  // a foreign scope covering a required narrow owner cannot be subtracted.
  for (const [key, group] of catalogRustGroups) {
    if (group.length < 2 || !group.every((module) => selectedIds.has(module.id))) continue;
    const selectedGroup = selected.filter((module) => group.some((candidate) => candidate.id === module.id));
    const shape = rustBatchShape(selectedGroup[0]);
    const selectedFilters = selectedGroup.map((module) => rustBatchShape(module).filter);
    const skipFilters = [...(excludedRustFilters.get(key) || [])]
      // The same case may have distinct owners for two supported platforms.
      .filter((filter) => !selectedFilters.includes(filter));
    // General owners cover all applicable descendants. A narrower foreign
    // owner can be excluded, but never erase a complete selected narrow owner.
    if (skipFilters.some((excluded) => selectedFilters.some((included) =>
      included.includes(excluded)))) continue;
    const args = skipFilters.length === 0 ? shape.broadArgs
      : [...shape.broadArgs, "--", ...skipFilters.flatMap((filter) => ["--skip", filter])];
    append(makeBatch(
      `rust-target-${batches.length + 1}`,
      selectedGroup,
      { ...selectedGroup[0].command, args, timeoutMs: Math.max(...selectedGroup.map((m) => m.command.timeoutMs)) },
      "target",
      { internalConcurrency: selectedGroup[0].regression.weight },
    ));
  }

  const nodeGroups = new Map();
  const flutterGroups = new Map();
  const gradleGroups = new Map();
  for (const module of selected) {
    if (consumed.has(module.id)) continue;
    const nodeShape = nodeTestShape(module);
    if (nodeShape && module.regression.toolchain === "node-test") {
      const key = `${module.regression.stage}:${nodeShape.key}`;
      const group = nodeGroups.get(key) || [];
      group.push(module);
      nodeGroups.set(key, group);
      continue;
    }
    const flutterShape = flutterTestShape(module);
    if (flutterShape) {
      const key = `${module.regression.stage}:${flutterShape.key}`;
      const group = flutterGroups.get(key) || [];
      group.push(module);
      flutterGroups.set(key, group);
      continue;
    }
    const gradleShape = gradleTestShape(module);
    if (gradleShape) {
      const key = `${module.regression.stage}:${gradleShape.key}`;
      const group = gradleGroups.get(key) || [];
      group.push(module);
      gradleGroups.set(key, group);
    }
  }

  for (const group of nodeGroups.values()) {
    const files = stableUnique(group.flatMap((module) => nodeTestShape(module).paths));
    const inputIndex = new Map(files.map((file, index) => [file, index]));
    const inputOwners = group.map((module) => ({
      member: module.id,
      indexes: stableUnique(nodeTestShape(module).paths).map((file) => inputIndex.get(file)),
    }));
    const concurrency = Math.max(1, Math.floor(availableParallelism / 2));
    append(makeBatch(`node-test-${batches.length + 1}`, group, {
      ...group[0].command,
      args: ["--test", `--test-concurrency=${concurrency}`, ...files],
      timeoutMs: group.reduce((total, module) => total + module.command.timeoutMs, 0),
    }, "files", {
      inputOwners,
      internalConcurrency: concurrency,
      weight: Math.max(group[0].regression.weight, concurrency),
    }));
  }

  for (const group of flutterGroups.values()) {
    const shape = flutterTestShape(group[0]);
    const allPaths = stableUnique(group.flatMap((module) => flutterTestShape(module).paths));
    const moduleChunks = [];
    for (let offset = 0; offset < allPaths.length; offset += 64) {
      const paths = allPaths.slice(offset, offset + 64);
      const pathSet = new Set(paths);
      const members = group.filter((module) =>
        flutterTestShape(module).paths.some((candidate) => pathSet.has(candidate)));
      moduleChunks.push({ members, paths });
    }
    for (const [index, chunk] of moduleChunks.entries()) {
      const members = chunk.members;
      const inputIndex = new Map(chunk.paths.map((file, pathIndex) => [file, pathIndex]));
      const inputOwners = members.map((module) => ({
        member: module.id,
        indexes: stableUnique(flutterTestShape(module).paths)
          .filter((file) => inputIndex.has(file))
          .map((file) => inputIndex.get(file)),
      }));
      append(makeBatch(`flutter-test-${batches.length + 1}-${index + 1}`, members, {
        ...members[0].command,
        args: [...shape.prefix, ...chunk.paths, ...shape.options],
        timeoutMs: members.reduce((total, module) => total + module.command.timeoutMs, 0),
      }, "files", {
        inputOwners,
        internalConcurrency: members[0].regression.weight,
      }));
    }
  }

  for (const group of gradleGroups.values()) {
    const shape = gradleTestShape(group[0]);
    const filters = stableUnique(group.flatMap((module) => gradleTestShape(module).filters));
    append(makeBatch(`gradle-test-${batches.length + 1}`, group, {
      ...group[0].command,
      args: [
        ...shape.prefix,
        ":app:testDebugUnitTest",
        ...filters.flatMap((filter) => ["--tests", filter]),
        ...shape.options,
      ],
      timeoutMs: group.reduce((total, module) => total + module.command.timeoutMs, 0),
    }, "filters"));
  }

  const exact = new Map();
  for (const module of selected) {
    if (consumed.has(module.id)) continue;
    const key = commandKey(module.command);
    const group = exact.get(key) || [];
    group.push(module);
    exact.set(key, group);
  }
  for (const group of exact.values()) {
    append(makeBatch(`exact-${batches.length + 1}`, group, group[0].command,
      group.length === 1 ? "exact" : "duplicate", {
        internalConcurrency: group[0].regression.internalParallelism
          ? group[0].regression.weight
          : null,
      }));
  }

  const order = new Map(selected.map((module, index) => [module.id, index]));
  return Object.freeze(batches.sort((left, right) =>
    order.get(left.members[0]) - order.get(right.members[0])));
}
