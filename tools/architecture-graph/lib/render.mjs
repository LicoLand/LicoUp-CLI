import fs from "node:fs";
import path from "node:path";

import { RELATION_CLASSES, RELATION_CLASS_NOTES, topologicalLayers } from "./graph-model.mjs";
import { legacyMapping, relationChecks } from "./work-items.mjs";

/**
 * Generated views: Mermaid text, deterministic SVG and one offline HTML page.
 *
 * These files are outputs. Editing a generated file by hand is drift, not a
 * change: the graph documents are the source of truth, and `render` is
 * byte-reproducible for the same inputs so drift is detectable.
 */

function escapeXml(text) {
  return String(text)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function mermaidId(id) {
  return String(id).replaceAll(/[^A-Za-z0-9_]/gu, "_");
}

export function architectureMermaid(graph) {
  const lines = ["flowchart LR"];
  for (const module of graph.modules.values()) lines.push(`  ${module.id}["${module.id} · ${module.title}"]`);
  for (const edge of graph.architecture.edges) {
    lines.push(edge.type === "depends_on"
      ? `  ${edge.source} -->|depends_on| ${edge.target}`
      : `  ${edge.source} -.->|runtime_calls ${edge.contract}| ${edge.target}`);
  }
  return `${lines.join("\n")}\n`;
}

export function developmentMermaid(graph) {
  const lines = ["flowchart LR"];
  for (const task of graph.tasks.values()) lines.push(`  ${mermaidId(task.id)}["${task.id} · ${task.title}"]`);
  for (const [predecessor, dependent] of graph.developmentDag().edges) {
    lines.push(`  ${mermaidId(predecessor)} -->|precedes| ${mermaidId(dependent)}`);
  }
  return `${lines.join("\n")}\n`;
}

export function distributionMermaid(graph) {
  const lines = ["flowchart LR"];
  const ids = new Map([...graph.packages.keys()].map((id, index) => [id, `PKG${index}`]));
  for (const pkg of graph.packages.values()) lines.push(`  ${ids.get(pkg.id)}["${pkg.id} · ${pkg.title}"]`);
  for (const pkg of graph.packages.values()) {
    for (const dependency of pkg.requires_package) {
      lines.push(`  ${ids.get(pkg.id)} -->|requires_package| ${ids.get(dependency)}`);
    }
  }
  return `${lines.join("\n")}\n`;
}

export function relationsMermaid(graph, relationClass) {
  const labels = {
    [RELATION_CLASSES.GOAL_REFERENCE]: "depends_on",
    [RELATION_CLASSES.PORT_CALL]: "runtime_calls",
    [RELATION_CLASSES.DEVELOPMENT_ORDER]: "precedes",
    [RELATION_CLASSES.DEPLOYMENT_DELIVERY]: "requires_package",
    [RELATION_CLASSES.IMPACT_TRACEABILITY]: "impacts",
  };
  const selected = graph.relations().filter((edge) => edge.class === relationClass && edge.relation === labels[relationClass]);
  const lines = ["flowchart LR"];
  const nodes = [...new Set(selected.flatMap((edge) => [edge.source, edge.target]))].sort();
  for (const node of nodes) lines.push(`  ${mermaidId(node)}["${node}"]`);
  for (const edge of selected) lines.push(`  ${mermaidId(edge.source)} -->|${edge.relation}| ${mermaidId(edge.target)}`);
  return `${lines.join("\n")}\n`;
}

/**
 * Deterministic layered SVG. Text is real text, never a screenshot, and every
 * coordinate is derived from sorted node ids so the same graph always renders
 * the same bytes.
 */
export function layeredSvg({ nodes, nodeLayers, edges, label, note }) {
  const columnWidth = 310;
  const rowHeight = 98;
  const boxWidth = 276;
  const boxHeight = 72;
  const margin = 35;
  const positions = new Map();
  nodeLayers.forEach((layer, column) => {
    layer.forEach((id, row) => positions.set(id, [margin + column * columnWidth, margin + row * rowHeight]));
  });
  const width = margin * 2 + Math.max(nodeLayers.length, 1) * columnWidth;
  const height = margin * 2 + Math.max(nodeLayers.reduce((max, layer) => Math.max(max, layer.length), 1), 1) * rowHeight;
  const output = [
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}" width="${width}" height="${height}" role="img" aria-label="${escapeXml(label)}">`,
    `<title>${escapeXml(label)}</title>`,
    note ? `<desc>${escapeXml(note)}</desc>` : "",
    '<defs><marker id="arrow" markerWidth="9" markerHeight="8" refX="8" refY="4" orient="auto"><path d="M0,0 L8,4 L0,8" fill="#8196aa"/></marker></defs>',
  ].filter(Boolean);
  for (const [from, to] of edges) {
    const start = positions.get(from);
    const end = positions.get(to);
    if (!start || !end) continue;
    const [x, y] = start;
    const [xx, yy] = end;
    output.push(
      `<path d="M${x + boxWidth},${y + boxHeight / 2} C${x + boxWidth + 25},${y + boxHeight / 2} ${xx - 25},${yy + boxHeight / 2} ${xx},${yy + boxHeight / 2}" fill="none" stroke="#8aa0b5" stroke-width="1.3" marker-end="url(#arrow)"/>`,
    );
  }
  for (const id of [...positions.keys()].sort()) {
    const [x, y] = positions.get(id);
    const title = nodes.get(id)?.title ?? "";
    const label2 = `${id} ${title}`;
    output.push(
      `<g><title>${escapeXml(label2)}</title>`,
      `<rect x="${x}" y="${y}" width="${boxWidth}" height="${boxHeight}" rx="8" fill="#f2f7fb" stroke="#476c89"/>`,
      `<text x="${x + 12}" y="${y + 28}" font-family="system-ui,sans-serif" font-size="12" font-weight="700" fill="#284b66">${escapeXml(id)}</text>`,
      `<text x="${x + 12}" y="${y + 48}" font-family="system-ui,sans-serif" font-size="12" fill="#172f42">${escapeXml(title.slice(0, 34))}</text>`,
      "</g>",
    );
  }
  output.push("</svg>");
  return `${output.join("\n")}\n`;
}

export function traceabilityMarkdown(graph) {
  const mapping = legacyMapping(graph);
  const taskById = graph.tasks;
  const lines = [
    "# 工作项与旧任务追踪（由 resolved graph 生成，请勿手工编辑）",
    "",
    `图摘要：\`${graph.graphDigest}\``,
    "",
    "| 工作项 | 泳道 | 直接前置 | 原任务 | 旧缺口 | 验收 |",
    "|---|---|---|---|---|---|",
  ];
  for (const task of taskById.values()) {
    lines.push(`| ${task.id} ${task.title} | ${task.lane} | ${task.depends_on.join(", ") || "—"} | ${task.legacy_tasks.join(", ")} | ${task.audit_findings.join(", ") || "新增/补强"} | ${task.acceptance.map((caseId) => `${caseId}@${graph.effectiveLevel(task, caseId)}`).join(", ")} |`);
  }
  lines.push("", "## 旧任务映射", "");
  for (const pair of mapping.pairs) lines.push(`- ${pair.legacy_task} → ${pair.tasks.join(", ")}`);
  lines.push("", "映射只说明来源，不证明旧行为已退役。", "");
  return lines.join("\n");
}

export function taskSpecsMarkdown(graph) {
  const lines = [
    "# 工作项规格（由 resolved graph 生成，请勿手工编辑）",
    "",
    "路径为当前起点或拟落点。扩大范围先改图并重新协调，不写完再补说明。",
    "",
  ];
  for (const task of graph.tasks.values()) {
    lines.push(
      `## ${task.id} ${task.title}`,
      `泳道：${task.lane}。前置：${task.depends_on.join(", ") || "无"}。`,
      `模块：${task.modules.join(", ")}；合同：${task.contracts.map((id) => `${id}@${graph.contracts.get(id).revision}`).join(", ") || "无"}。`,
      "",
      `**交付物**：${task.deliverables.join("；")}。`,
      "",
      ...task.implementation.map((line) => `- ${line}`),
      "",
      "**独占写范围**：",
      ...task.write_scopes.map((scope) => `- \`${scope}\``),
      "",
      `**验收**：${task.acceptance.map((caseId) => `${caseId}@${graph.effectiveLevel(task, caseId)}`).join(", ")}。`,
      `高风险独立复核：${task.independent_review ? "是。" : "无需额外强制角色；沿原有代码评审。"}`,
      "",
    );
  }
  return lines.join("\n");
}

function offlineHtml({ graph, views, summary, checks, mapping }) {
  const panel = (id, title, svg, note) =>
    `<section><h2 id="${id}">${escapeXml(title)}</h2><p class="note">${escapeXml(note)}</p><div class="canvas">${svg}</div></section>`;
  const legend = Object.entries(RELATION_CLASS_NOTES)
    .map(([name, note]) => `<li><b>${escapeXml(name)}</b>：${escapeXml(note)}${name === RELATION_CLASSES.DEVELOPMENT_ORDER ? "  <b>（唯一进入开发DAG的关系类）</b>" : ""}</li>`)
    .join("");
  const checkRows = checks.checks
    .map((check) => `<tr><td>${escapeXml(check.id)}</td><td>${escapeXml(check.severity)}</td><td>${check.passed ? "通过" : "未通过"}</td><td>${escapeXml(check.failures.slice(0, 6).join("; "))}</td></tr>`)
    .join("");
  const mappingRows = mapping.pairs
    .map((pair) => `<tr><td>${escapeXml(pair.legacy_task)}</td><td>${escapeXml(pair.tasks.join(", "))}</td></tr>`)
    .join("");
  return `<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>LicoUp 架构与开发图（离线）</title>
<style>
body{font:15px/1.65 system-ui,sans-serif;margin:0;background:#f6f8fa;color:#193244}
header{background:#102e43;color:#fff;padding:22px 32px}main{padding:16px 32px;max-width:1500px;margin:auto}
h1{margin:0 0 6px;font-size:26px}h2{margin-top:28px}table{border-collapse:collapse;width:100%;background:#fff}
th,td{border:1px solid #c9d6e0;padding:6px 8px;text-align:left;font-size:13px}
.canvas{overflow:auto;background:#fff;border:1px solid #c6d3dd;border-radius:8px;padding:10px;max-width:100%}
.note{color:#4a6377;font-size:13px}code{background:#eaf1f7;padding:1px 4px;border-radius:3px}
ul{background:#fff;border:1px solid #d5e0e8;border-radius:6px;padding:12px 34px}
</style></head>
<body><header><h1>LicoUp 架构与开发图</h1>
<p>目标图，不是当前实现证明。离线文件：无 CDN、无遥测、无网络请求。</p>
<p><code>graph_digest ${escapeXml(summary.graph_digest)}</code> · 架构版本 ${escapeXml(graph.architecture.revision)}</p></header>
<main>
<section><h2>关系类</h2><ul>${legend}</ul></section>
${panel("architecture", "目标依赖图（消费方 → 被依赖模块）", views.architectureSvg, "虚线与端口调用见对应的 Mermaid 文件；此图不是任务等待。")}
${panel("distribution", "按需安装与裁剪图", views.distributionSvg, "包 → 必需依赖，只决定安装闭包，不参与开发DAG等待。")}
${panel("development", "开发执行 DAG（仅 precedes）", views.developmentSvg, "只有 task.depends_on 降低成的 precedes 进入本图。")}
<section><h2>有向关系检查</h2><table><tr><th>检查</th><th>级别</th><th>结果</th><th>失败项</th></tr>${checkRows}</table></section>
<section><h2>旧任务映射</h2><table><tr><th>原任务</th><th>本轮工作项</th></tr>${mappingRows}</table></section>
<section><h2>规模</h2><pre>${escapeXml(JSON.stringify(summary, null, 2))}</pre></section>
</main></body></html>
`;
}

export function renderViews(graph, outDirectory) {
  fs.mkdirSync(outDirectory, { recursive: true });
  const development = graph.developmentDag();
  const architectureLayers = graph.moduleLayers();
  const packageLayers = topologicalLayers(
    [...graph.packages.keys()],
    [...graph.packages.values()].flatMap((pkg) => pkg.requires_package.map((dependency) => [pkg.id, dependency])),
  );

  const views = {
    architectureSvg: layeredSvg({
      nodes: graph.modules,
      nodeLayers: architectureLayers,
      edges: graph.architecture.edges.filter((edge) => edge.type === "depends_on").map((edge) => [edge.source, edge.target]),
      label: "目标架构：消费方到被依赖模块",
      note: "Declared target only; it is not an observation of the current code tree.",
    }),
    developmentSvg: layeredSvg({
      nodes: graph.tasks,
      nodeLayers: development.layers,
      edges: development.edges,
      label: "开发前置关系：左到右",
      note: "Only precedes edges lowered from task.depends_on.",
    }),
    distributionSvg: layeredSvg({
      nodes: graph.packages,
      nodeLayers: packageLayers,
      edges: [...graph.packages.values()].flatMap((pkg) => pkg.requires_package.map((dependency) => [pkg.id, dependency])),
      label: "部署：包到必需依赖",
      note: "Install closure only; never a development prerequisite.",
    }),
  };

  const files = {
    "architecture.mmd": architectureMermaid(graph),
    "execution.mmd": developmentMermaid(graph),
    "distribution.mmd": distributionMermaid(graph),
    "relations.goal-reference.mmd": relationsMermaid(graph, RELATION_CLASSES.GOAL_REFERENCE),
    "relations.port-call.mmd": relationsMermaid(graph, RELATION_CLASSES.PORT_CALL),
    "relations.development-order.mmd": relationsMermaid(graph, RELATION_CLASSES.DEVELOPMENT_ORDER),
    "relations.deployment-delivery.mmd": relationsMermaid(graph, RELATION_CLASSES.DEPLOYMENT_DELIVERY),
    "architecture.svg": views.architectureSvg,
    "execution.svg": views.developmentSvg,
    "distribution.svg": views.distributionSvg,
    "traceability.md": traceabilityMarkdown(graph),
    "TASKS.md": taskSpecsMarkdown(graph),
  };

  const checks = relationChecks(graph);
  const summary = {
    graph_digest: graph.graphDigest,
    graph_version: graph.graphVersion,
    modules: graph.modules.size,
    contracts: graph.contracts.size,
    tasks: graph.tasks.size,
    acceptance_cases: graph.cases.size,
    packages: graph.packages.size,
    profiles: graph.profiles.size,
    development_layers: development.layers.length,
    layer_width: Math.max(0, ...development.layers.map((layer) => layer.length)),
    note: "Layer width is structural. It is not a measured speedup or the exact maximum feasible concurrency.",
  };
  files["index.html"] = offlineHtml({ graph, views, summary, checks, mapping: legacyMapping(graph) });
  files["summary.json"] = `${JSON.stringify(summary, null, 2)}\n`;
  files["relation-checks.json"] = `${JSON.stringify(checks, null, 2)}\n`;

  for (const [name, content] of Object.entries(files)) {
    fs.writeFileSync(path.join(outDirectory, name), content, "utf8");
  }
  return { outDirectory, files: Object.keys(files).sort(), summary };
}
