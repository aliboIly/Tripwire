#!/usr/bin/env node
// Command-line entry for the security reviewer, used by the security-review CI
// workflow to post a PR comment. It prints a Markdown report to stdout and always
// exits 0, so the workflow can comment even when the scan path is missing.

import { reviewSecurity, SecurityReport } from "./security.js";

const target = process.argv[2] ?? "sample-game/src";

function plural(n: number): string {
  return n === 1 ? "" : "s";
}

function toMarkdown(report: SecurityReport, path: string): string {
  const lines = [
    "### Tripwire security review",
    "",
    `Scanned \`${path}\`: ${report.handlers.length} remote handler${plural(report.handlers.length)}, ` +
      `${report.findings.length} finding${plural(report.findings.length)}.`,
    "",
  ];
  if (report.findings.length === 0) {
    lines.push("No client-trust issues found.");
    return lines.join("\n");
  }
  for (const f of report.findings) {
    lines.push(`#### [${f.severity.toUpperCase()}] \`${f.remote}\` at \`${f.file}:${f.line}\``);
    lines.push(`Issue: ${f.issue}`);
    lines.push(`Fix: ${f.fix}`);
    lines.push("");
  }
  return lines.join("\n");
}

try {
  process.stdout.write(toMarkdown(reviewSecurity(target), target) + "\n");
} catch (err) {
  const message = err instanceof Error ? err.message : String(err);
  process.stdout.write(`### Tripwire security review\n\nCould not run the review: ${message}\n`);
}
