// Security reviewer. Static analysis over a Rojo source tree: it parses each
// TypeScript file with swc and flags a server RemoteEvent/RemoteFunction handler that
// acts on a value the client sent without validating it. The client controls every
// argument after the player, so trusting one is how exploits get in.
//
// Syntactic, not type-aware: it does not chase values across functions, so it favors
// clear, low-noise findings over completeness (the same boundary the TS version had).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, SourceFile, SourceMap, Span, Spanned};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BlockStmtOrExpr, Callee, Expr, IfStmt,
    Lit, MemberProp, Pat, SimpleAssignTarget, TsAsExpr, UnaryOp,
};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::visit::{Visit, VisitWith};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Handler {
    pub file: String,
    pub line: usize,
    pub remote: String,
    pub hook: String,
    pub client_params: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub remote: String,
    pub param: String,
    pub severity: String,
    pub issue: String,
    pub fix: String,
}

#[derive(Default)]
pub struct Report {
    pub handlers: Vec<Handler>,
    pub findings: Vec<Finding>,
}

const SKIP_DIRS: [&str; 5] = ["node_modules", "out", "include", "dist", ".git"];

pub fn review(root: &str) -> Result<Report, String> {
    if !Path::new(root).exists() {
        return Err(format!(
            "source path not found: {root} (run the server from the repo root, or pass an absolute path)"
        ));
    }
    let mut report = Report::default();
    for file in collect_ts_files(Path::new(root)) {
        analyze_file(&file, &mut report);
    }
    Ok(report)
}

fn collect_ts_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return files,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !SKIP_DIRS.contains(&name) {
                files.extend(collect_ts_files(&path));
            }
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".ts") && !name.ends_with(".d.ts") {
                files.push(path);
            }
        }
    }
    files
}

fn analyze_file(file: &Path, report: &mut Report) {
    let source = match std::fs::read_to_string(file) {
        Ok(source) => source,
        Err(_) => return,
    };
    analyze_source(&file.to_string_lossy(), &source, report);
}

fn analyze_source(name: &str, source: &str, report: &mut Report) {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(FileName::Custom(name.to_owned()).into(), source.to_owned());
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax::default()),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let module = match parser.parse_typescript_module() {
        Ok(module) => module,
        Err(_) => return,
    };
    let mut analyzer = Analyzer {
        file: name.to_owned(),
        fm,
        report,
    };
    module.visit_with(&mut analyzer);
}

struct Analyzer<'a> {
    file: String,
    fm: Lrc<SourceFile>,
    report: &'a mut Report,
}

impl<'a> Visit for Analyzer<'a> {
    fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
        // remote.OnServerEvent.Connect(handler)
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Member(outer) = &**callee {
                if member_is(&outer.prop, "Connect") {
                    if let Expr::Member(inner) = &*outer.obj {
                        if member_is(&inner.prop, "OnServerEvent") {
                            if let Some(cb) = call.args.first().map(|a| &*a.expr) {
                                let remote = self.snippet(inner.obj.span());
                                self.handle(remote, "OnServerEvent", cb);
                            }
                        }
                    }
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        // remote.OnServerInvoke = handler
        if assign.op == AssignOp::Assign {
            if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left {
                if member_is(&member.prop, "OnServerInvoke") {
                    let remote = self.snippet(member.obj.span());
                    self.handle(remote, "OnServerInvoke", &assign.right);
                }
            }
        }
        assign.visit_children_with(self);
    }
}

impl<'a> Analyzer<'a> {
    // Snippet and line are computed from the file's own source and byte offsets, so
    // no SourceMapper trait import is needed.
    fn snippet(&self, span: Span) -> String {
        let start = self.fm.start_pos.0;
        let lo = span.lo().0.saturating_sub(start) as usize;
        let hi = span.hi().0.saturating_sub(start) as usize;
        self.fm.src.get(lo..hi).unwrap_or("<remote>").to_string()
    }

    fn line(&self, span: Span) -> usize {
        let start = self.fm.start_pos.0;
        let lo = span.lo().0.saturating_sub(start) as usize;
        self.fm
            .src
            .get(..lo)
            .map(|s| s.bytes().filter(|&b| b == b'\n').count() + 1)
            .unwrap_or(0)
    }

    fn handle(&mut self, remote: String, hook: &str, callback: &Expr) {
        let params = match callback_params(callback) {
            Some(params) => params,
            None => return,
        };
        let line = self.line(callback.span());
        self.report.handlers.push(Handler {
            file: self.file.clone(),
            line,
            remote: remote.clone(),
            hook: hook.to_string(),
            client_params: params.clone(),
        });
        if params.is_empty() {
            return;
        }

        let mut scan = BodyScan::default();
        scan_body(callback, &mut scan);
        for param in &params {
            if !scan.used.contains(param) || scan.validated.contains(param) {
                continue;
            }
            let casted = scan.casted.contains(param);
            let issue = format!(
                "Server handler for {remote} ({hook}) uses client value '{param}' without validating it{}",
                if casted { " and forces its type with an unchecked cast." } else { "." }
            );
            self.report.findings.push(Finding {
                file: self.file.clone(),
                line,
                remote: remote.clone(),
                param: param.clone(),
                severity: if casted { "high" } else { "medium" }.to_string(),
                issue,
                fix: format!(
                    "Validate '{param}' on the server before acting on it: check its type and range, reject anything unexpected, and derive the outcome from server-side state rather than the client's value."
                ),
            });
        }
    }
}

fn member_is(prop: &MemberProp, name: &str) -> bool {
    matches!(prop, MemberProp::Ident(ident) if ident.sym.as_str() == name)
}

// The first parameter is the player the engine supplies; the rest are client controlled.
fn callback_params(callback: &Expr) -> Option<Vec<String>> {
    let names: Vec<String> = match callback {
        Expr::Arrow(arrow) => arrow.params.iter().filter_map(pat_name).collect(),
        Expr::Fn(func) => func
            .function
            .params
            .iter()
            .filter_map(|p| pat_name(&p.pat))
            .collect(),
        _ => return None,
    };
    Some(names.into_iter().skip(1).collect())
}

fn pat_name(pat: &Pat) -> Option<String> {
    match pat {
        Pat::Ident(binding) => Some(binding.id.sym.to_string()),
        _ => Some("_".to_string()),
    }
}

fn scan_body(callback: &Expr, scan: &mut BodyScan) {
    match callback {
        Expr::Arrow(arrow) => match &*arrow.body {
            BlockStmtOrExpr::BlockStmt(block) => block.visit_with(scan),
            BlockStmtOrExpr::Expr(expr) => expr.visit_with(scan),
        },
        Expr::Fn(func) => {
            if let Some(block) = &func.function.body {
                block.visit_with(scan);
            }
        }
        _ => {}
    }
}

#[derive(Default)]
struct BodyScan {
    used: HashSet<String>,
    validated: HashSet<String>,
    casted: HashSet<String>,
}

impl Visit for BodyScan {
    fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Ident(ident) = &**callee {
                if ident.sym.as_str() == "typeIs" {
                    if let Some(first) = call.args.first() {
                        if let Expr::Ident(arg) = &*first.expr {
                            self.validated.insert(arg.sym.to_string());
                        }
                    }
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_if_stmt(&mut self, stmt: &IfStmt) {
        // Only a real check counts as validation. A bare truthy guard like
        // `if (amount)` or an existence check `if (amount !== undefined)` does not stop
        // a client from forging a huge number, so the value stays tainted. We mark a
        // param validated only when the test actually compares or type-checks it.
        let mut checked = ValidationCollector::default();
        stmt.test.visit_with(&mut checked);
        self.validated.extend(checked.0);
        stmt.visit_children_with(self);
    }

    fn visit_ts_as_expr(&mut self, as_expr: &TsAsExpr) {
        if let Expr::Ident(ident) = &*as_expr.expr {
            self.casted.insert(ident.sym.to_string());
        }
        as_expr.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
        self.used.insert(ident.sym.to_string());
    }
}

// Collects the params a test genuinely checks: an operand of a comparison
// (`amount > 0`, `kind === "buy"`) or the subject of a `typeof`. An equality against
// null/undefined is an existence check, not validation, so it is excluded.
#[derive(Default)]
struct ValidationCollector(HashSet<String>);

impl Visit for ValidationCollector {
    fn visit_bin_expr(&mut self, bin: &BinExpr) {
        if is_relational(bin.op) || (is_equality(bin.op) && !compares_nullish(bin)) {
            self.note(&bin.left);
            self.note(&bin.right);
        }
        bin.visit_children_with(self);
    }
}

impl ValidationCollector {
    fn note(&mut self, expr: &Expr) {
        match expr {
            Expr::Ident(ident) => {
                self.0.insert(ident.sym.to_string());
            }
            // `typeof amount === "number"`: the param is the typeof argument.
            Expr::Unary(unary) if unary.op == UnaryOp::TypeOf => {
                if let Expr::Ident(ident) = &*unary.arg {
                    self.0.insert(ident.sym.to_string());
                }
            }
            _ => {}
        }
    }
}

fn is_relational(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq
    )
}

fn is_equality(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::EqEq | BinaryOp::NotEq | BinaryOp::EqEqEq | BinaryOp::NotEqEq
    )
}

fn compares_nullish(bin: &BinExpr) -> bool {
    is_nullish(&bin.left) || is_nullish(&bin.right)
}

fn is_nullish(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(Lit::Null(_)) => true,
        Expr::Ident(ident) => ident.sym.as_str() == "undefined",
        _ => false,
    }
}

pub fn format_report(report: &Report) -> String {
    let mut lines = vec![
        format!(
            "Security review: {} server remote handler(s), {} finding(s).",
            report.handlers.len(),
            report.findings.len()
        ),
        String::new(),
    ];
    if report.findings.is_empty() {
        lines.push("No client-trust issues found.".into());
    } else {
        for f in &report.findings {
            lines.push(format!(
                "[{}] {} at {}:{}",
                f.severity.to_uppercase(),
                f.remote,
                f.file,
                f.line
            ));
            lines.push(format!("  issue: {}", f.issue));
            lines.push(format!("  fix:   {}", f.fix));
            lines.push(String::new());
        }
    }
    lines.push("Handlers scanned:".into());
    for h in &report.handlers {
        let params = if h.client_params.is_empty() {
            "(none)".to_string()
        } else {
            h.client_params.join(", ")
        };
        lines.push(format!(
            "  {} {} (client params: {params}) at {}:{}",
            h.remote, h.hook, h.file, h.line
        ));
    }
    lines.join("\n")
}

/// Machine-readable report for tooling that wants to act on findings (CI, dashboards).
pub fn format_report_json(report: &Report) -> String {
    let value = serde_json::json!({
        "handlerCount": report.handlers.len(),
        "findingCount": report.findings.len(),
        "findings": report.findings,
        "handlers": report.handlers,
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flagged_params(src: &str) -> Vec<String> {
        let mut report = Report::default();
        analyze_source("test.ts", src, &mut report);
        report.findings.iter().map(|f| f.param.clone()).collect()
    }

    #[test]
    fn flags_unvalidated_client_value() {
        let src = "remote.OnServerEvent.Connect((player, amount) => { grant(player, amount as number); });";
        assert_eq!(flagged_params(src), ["amount"]);
    }

    #[test]
    fn type_check_clears_it() {
        let src = "remote.OnServerEvent.Connect((player, itemId) => { if (!typeIs(itemId, \"string\")) return; buy(itemId); });";
        assert!(flagged_params(src).is_empty());
    }

    #[test]
    fn bare_truthy_guard_is_not_validation() {
        // The regression this fix targets: a truthy check does not make a forged number safe.
        let src = "remote.OnServerEvent.Connect((player, amount) => { if (amount) grant(player, amount as number); });";
        assert_eq!(flagged_params(src), ["amount"]);
    }

    #[test]
    fn existence_check_is_not_validation() {
        let src = "remote.OnServerEvent.Connect((player, amount) => { if (amount !== undefined) grant(player, amount as number); });";
        assert_eq!(flagged_params(src), ["amount"]);
    }

    #[test]
    fn range_check_is_validation() {
        let src = "remote.OnServerEvent.Connect((player, amount) => { if (amount > 0 && amount < 100) grant(player, amount); });";
        assert!(flagged_params(src).is_empty());
    }

    #[test]
    fn equality_whitelist_is_validation() {
        let src = "remote.OnServerEvent.Connect((player, kind) => { if (kind === \"buy\" || kind === \"sell\") act(kind as string); });";
        assert!(flagged_params(src).is_empty());
    }
}
