// Security reviewer (Phase 5).
//
// Static analysis over a Rojo source tree. It parses each TypeScript file with
// the compiler's syntax tree and looks for the failure that breaks most Roblox
// games: a server RemoteEvent or RemoteFunction handler that acts on a value the
// client sent without validating it. The client controls every argument after
// the player, so trusting one is how exploits get in.
//
// This is a syntactic pass, not a type-aware one. It does not chase values
// across functions, so it favors clear, low-noise findings over completeness.

import { readdirSync, statSync, readFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import ts from "typescript";

export interface RemoteHandler {
  file: string;
  line: number;
  remote: string;
  hook: "OnServerEvent" | "OnServerInvoke";
  clientParams: string[];
}

export interface Finding {
  file: string;
  line: number;
  remote: string;
  param: string;
  severity: "high" | "medium";
  issue: string;
  fix: string;
}

export interface SecurityReport {
  handlers: RemoteHandler[];
  findings: Finding[];
}

const SKIP_DIRS = new Set(["node_modules", "out", "include", "dist", ".git"]);

function collectTsFiles(dir: string): string[] {
  const files: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      if (!SKIP_DIRS.has(entry)) files.push(...collectTsFiles(full));
    } else if (entry.endsWith(".ts") && !entry.endsWith(".d.ts")) {
      files.push(full);
    }
  }
  return files;
}

function lineOf(sf: ts.SourceFile, node: ts.Node): number {
  return sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1;
}

function asHandlerFunction(node: ts.Node | undefined): ts.ArrowFunction | ts.FunctionExpression | undefined {
  if (node && (ts.isArrowFunction(node) || ts.isFunctionExpression(node))) return node;
  return undefined;
}

// What a client param contributes inside a handler body: whether it is read,
// validated (passed to typeIs or named in an if condition), and whether it is
// used through an unchecked `as` cast.
interface ParamUsage {
  used: Set<string>;
  validated: Set<string>;
  casted: Set<string>;
}

function scanBody(body: ts.Node): ParamUsage {
  const usage: ParamUsage = { used: new Set(), validated: new Set(), casted: new Set() };

  const noteCondition = (node: ts.Node): void => {
    if (ts.isIdentifier(node)) usage.validated.add(node.text);
    ts.forEachChild(node, noteCondition);
  };

  const walk = (node: ts.Node): void => {
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === "typeIs") {
      const first = node.arguments[0];
      if (first && ts.isIdentifier(first)) usage.validated.add(first.text);
    }
    if (ts.isIfStatement(node)) noteCondition(node.expression);
    if (ts.isAsExpression(node) && ts.isIdentifier(node.expression)) usage.casted.add(node.expression.text);
    if (ts.isIdentifier(node)) usage.used.add(node.text);
    ts.forEachChild(node, walk);
  };
  walk(body);
  return usage;
}

function reviewHandler(
  file: string,
  sf: ts.SourceFile,
  remote: string,
  hook: RemoteHandler["hook"],
  fn: ts.ArrowFunction | ts.FunctionExpression,
  report: SecurityReport,
): void {
  const line = lineOf(sf, fn);
  // The first parameter is the player the engine supplies; the rest are client controlled.
  const clientParams = fn.parameters.slice(1).map((p) => p.name.getText(sf));
  report.handlers.push({ file, line, remote, hook, clientParams });
  if (clientParams.length === 0) return;

  const usage = scanBody(fn.body);
  for (const param of clientParams) {
    if (!usage.used.has(param) || usage.validated.has(param)) continue;
    const casted = usage.casted.has(param);
    report.findings.push({
      file,
      line,
      remote,
      param,
      severity: casted ? "high" : "medium",
      issue:
        `Server handler for ${remote} (${hook}) uses client value '${param}' without validating it` +
        (casted ? " and forces its type with an unchecked cast." : "."),
      fix: `Validate '${param}' on the server before acting on it: check its type and range, reject anything unexpected, and derive the outcome from server-side state rather than the client's value.`,
    });
  }
}

function analyzeFile(file: string, report: SecurityReport): void {
  const sf = ts.createSourceFile(file, readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true);

  const visit = (node: ts.Node): void => {
    // remote.OnServerEvent.Connect(handler)
    if (
      ts.isCallExpression(node) &&
      ts.isPropertyAccessExpression(node.expression) &&
      node.expression.name.text === "Connect" &&
      ts.isPropertyAccessExpression(node.expression.expression) &&
      node.expression.expression.name.text === "OnServerEvent"
    ) {
      const fn = asHandlerFunction(node.arguments[0]);
      if (fn) reviewHandler(file, sf, node.expression.expression.expression.getText(sf), "OnServerEvent", fn, report);
    }

    // remote.OnServerInvoke = handler
    if (
      ts.isBinaryExpression(node) &&
      node.operatorToken.kind === ts.SyntaxKind.EqualsToken &&
      ts.isPropertyAccessExpression(node.left) &&
      node.left.name.text === "OnServerInvoke"
    ) {
      const fn = asHandlerFunction(node.right);
      if (fn) reviewHandler(file, sf, node.left.expression.getText(sf), "OnServerInvoke", fn, report);
    }

    ts.forEachChild(node, visit);
  };
  visit(sf);
}

export function reviewSecurity(rootDir: string): SecurityReport {
  if (!existsSync(rootDir)) {
    throw new Error(`source path not found: ${rootDir} (run the server from the repo root, or pass an absolute path)`);
  }
  const report: SecurityReport = { handlers: [], findings: [] };
  for (const file of collectTsFiles(rootDir)) analyzeFile(file, report);
  return report;
}

export function formatReport(report: SecurityReport): string {
  const lines: string[] = [
    `Security review: ${report.handlers.length} server remote handler(s), ${report.findings.length} finding(s).`,
    "",
  ];
  if (report.findings.length === 0) {
    lines.push("No client-trust issues found.");
  } else {
    for (const f of report.findings) {
      lines.push(`[${f.severity.toUpperCase()}] ${f.remote} at ${f.file}:${f.line}`);
      lines.push(`  issue: ${f.issue}`);
      lines.push(`  fix:   ${f.fix}`);
      lines.push("");
    }
  }
  lines.push("Handlers scanned:");
  for (const h of report.handlers) {
    const params = h.clientParams.length > 0 ? h.clientParams.join(", ") : "(none)";
    lines.push(`  ${h.remote} ${h.hook} (client params: ${params}) at ${h.file}:${h.line}`);
  }
  return lines.join("\n");
}
