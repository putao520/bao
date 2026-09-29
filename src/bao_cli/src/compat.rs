// `bao compat` — machine-rebuildable compatibility inventory reports.
//
// The data source is the four INVENTORY SSOTs (compat/{node,bun,cdp,web}/
// INVENTORY.md), embedded at build time via include_str! so the shipped
// binary never depends on a checkout being present. The INVENTORY files are
// the single source of truth: this module only PRESENTS them — every count
// is computed from table rows by the explicit per-domain column contract
// below; no classification logic lives here, and the anchor (upstream ref)
// lines are carried verbatim from the files' 锚定 sections.
//
// Issue lineage: #11-E / #12-E / #13-G / #14-H (the four G1 report commands)
// aggregated under #21 Gate A; design: /tmp/w14-design.md (2026-09-29).
//
// Exit-code contract: report commands are observation surfaces — always 0 on
// success (same class as `bao doctor`). Partial/Unsupported rows are
// inventory facts, not failures; consumers that need a gate read --json.
// 1 = embedded-content parse failure (fail-closed loud, line-numbered —
// unreachable in practice because the dispatch tests parse all four files
// at every build).

use std::fmt::Write as _;

/// The four inventory domains (`bao compat <domain>`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Domain {
    Node,
    Bun,
    Cdp,
    Web,
}

impl Domain {
    pub const ALL: [Domain; 4] = [Domain::Node, Domain::Bun, Domain::Cdp, Domain::Web];

    pub fn parse(s: &str) -> Option<Domain> {
        match s {
            "node" => Some(Domain::Node),
            "bun" => Some(Domain::Bun),
            "cdp" => Some(Domain::Cdp),
            "web" => Some(Domain::Web),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Domain::Node => "node",
            Domain::Bun => "bun",
            Domain::Cdp => "cdp",
            Domain::Web => "web",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Domain::Node => "Node.js API inventory",
            Domain::Bun => "Bun API inventory",
            Domain::Cdp => "Chrome DevTools Protocol inventory",
            Domain::Web => "Web platform family inventory",
        }
    }

    fn source(self) -> &'static str {
        match self {
            Domain::Node => "compat/node/INVENTORY.md",
            Domain::Bun => "compat/bun/INVENTORY.md",
            Domain::Cdp => "compat/cdp/INVENTORY.md",
            Domain::Web => "compat/web/INVENTORY.md",
        }
    }

    fn inventory(self) -> &'static str {
        match self {
            Domain::Node => include_str!("../../../compat/node/INVENTORY.md"),
            Domain::Bun => include_str!("../../../compat/bun/INVENTORY.md"),
            Domain::Cdp => include_str!("../../../compat/cdp/INVENTORY.md"),
            Domain::Web => include_str!("../../../compat/web/INVENTORY.md"),
        }
    }
}

/// One INVENTORY status class. Derived `Ord` ranks worst-first (smaller =
/// worse) so compound cells (e.g. web's "Supported(X)/ Partial(Y)") roll up
/// conservatively.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Status {
    Unsupported,
    ExplicitlyUnsupported,
    PartialAckOnly,
    Partial,
    SupportedAck,
    Supported,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Supported => "Supported",
            Status::SupportedAck => "Supported(ack)",
            Status::Partial => "Partial",
            Status::PartialAckOnly => "Partial(ack-only)",
            Status::ExplicitlyUnsupported => "Explicitly-Unsupported",
            Status::Unsupported => "Unsupported",
        }
    }
}

/// Per-section / per-domain status tallies. `unclassified` is explicit —
/// any status cell that matches no known token is counted, warned on
/// stderr, and never silently dropped.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusCounts {
    pub supported: u32,
    pub supported_ack: u32,
    pub partial: u32,
    pub partial_ack_only: u32,
    pub explicitly_unsupported: u32,
    pub unsupported: u32,
    pub unclassified: u32,
}

impl StatusCounts {
    fn add(&mut self, s: Status) {
        match s {
            Status::Supported => self.supported += 1,
            Status::SupportedAck => self.supported_ack += 1,
            Status::Partial => self.partial += 1,
            Status::PartialAckOnly => self.partial_ack_only += 1,
            Status::ExplicitlyUnsupported => self.explicitly_unsupported += 1,
            Status::Unsupported => self.unsupported += 1,
        }
    }
}

/// One named table family within a domain report (cdp: one per dispatched
/// domain + the non-dispatch table; bun: static members / bun:* modules /
/// CLI subcommands; node and web: single tables).
#[derive(Clone, Debug)]
pub struct Section {
    pub name: String,
    pub rows: u32,
    pub status: StatusCounts,
}

/// Parsed report for one domain: verbatim anchor lines + per-section counts.
#[derive(Clone, Debug)]
pub struct DomainReport {
    pub domain: Domain,
    pub source: &'static str,
    pub anchors: Vec<String>,
    pub sections: Vec<Section>,
}

impl DomainReport {
    /// (total rows, summed status counts) across all sections.
    pub fn totals(&self) -> (u32, StatusCounts) {
        let mut rows = 0;
        let mut counts = StatusCounts::default();
        for s in &self.sections {
            rows += s.rows;
            counts.supported += s.status.supported;
            counts.supported_ack += s.status.supported_ack;
            counts.partial += s.status.partial;
            counts.partial_ack_only += s.status.partial_ack_only;
            counts.explicitly_unsupported += s.status.explicitly_unsupported;
            counts.unsupported += s.status.unsupported;
            counts.unclassified += s.status.unclassified;
        }
        (rows, counts)
    }

    /// Human-readable report (see the design doc §1 for the layout).
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        let (rows, counts) = self.totals();
        let _ = writeln!(
            out,
            "bao compat {} — {} (v{})",
            self.domain.as_str(),
            self.domain.title(),
            env!("CARGO_PKG_VERSION")
        );
        if self.anchors.is_empty() {
            let _ = writeln!(
                out,
                "  ref: (anchor section not found — see {})",
                self.source
            );
        } else {
            for a in self.anchors.iter().take(6) {
                let _ = writeln!(out, "  ref: {a}");
            }
        }
        let _ = writeln!(out, "  source: {} (embedded at build)", self.source);
        let _ = writeln!(out);
        let _ = writeln!(out, "  {}", format_counts(rows, &counts, true));
        for s in &self.sections {
            let _ = writeln!(out, "    {}", format_counts(s.rows, &s.status, false));
            let _ = writeln!(out, "      [{}]", s.name);
        }
        let _ = writeln!(
            out,
            "\n  (note) counts computed from table rows; status classes are inventory\n  facts, not pass/fail. SSOT: {}",
            self.source
        );
        out
    }

    /// Machine-readable JSON (serde_json; schema in the design doc §1).
    pub fn render_json(&self) -> String {
        let (rows, counts) = self.totals();
        let sections: Vec<serde_json::Value> = self
            .sections
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "rows": s.rows,
                    "status": counts_json(&s.status),
                })
            })
            .collect();
        serde_json::json!({
            "domain": self.domain.as_str(),
            "source": self.source,
            "anchors": self.anchors,
            "sections": sections,
            "totals": { "rows": rows, "status": counts_json(&counts) },
        })
        .to_string()
    }
}

/// Embedded-content contract violation (fail-closed, line-numbered).
#[derive(Clone, Debug)]
pub struct ParseError {
    pub file: &'static str,
    pub line: usize,
    pub reason: String,
}

impl ::std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.line, self.reason)
    }
}

/// Build the report for one domain from its embedded INVENTORY.
pub fn report(domain: Domain) -> Result<DomainReport, ParseError> {
    parse_inventory(domain.inventory(), domain.source(), domain)
}

/// Entry point from the CLI: `None` = the four-domain summary.
pub fn run(domain: Option<Domain>, json: bool) -> ::std::result::Result<(), i32> {
    let domains: Vec<Domain> = match domain {
        Some(d) => vec![d],
        None => Domain::ALL.to_vec(),
    };
    let mut reports = Vec::with_capacity(domains.len());
    for &d in &domains {
        match report(d) {
            Ok(r) => reports.push(r),
            Err(e) => {
                eprintln!("bao compat: {e}");
                return Err(1);
            }
        }
    }
    // Unclassified rows are surfaced loudly, never silently dropped.
    for r in &reports {
        for s in &r.sections {
            if s.status.unclassified > 0 {
                eprintln!(
                    "bao compat: warning: {} unclassified status row(s) in section '{}' of {}",
                    s.status.unclassified, s.name, r.source
                );
            }
        }
    }
    if json {
        // Single domain = the domain object itself; summary = the root wrap.
        if reports.len() == 1 {
            println!("{}", reports[0].render_json());
        } else {
            println!("{}", render_json_root(&reports));
        }
    } else if reports.len() == 1 {
        print!("{}", reports[0].render_text());
    } else {
        print!("{}", render_text_root(&reports));
    }
    Ok(())
}

/// Four-domain summary text (`bao compat` with no domain).
pub fn render_text_root(reports: &[DomainReport]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "bao compat — compatibility inventory summary (v{})",
        env!("CARGO_PKG_VERSION")
    );
    for r in reports {
        let (rows, counts) = r.totals();
        let _ = writeln!(
            out,
            "  {:<4} sections {:>2}   rows {:>4}   Supported {} / Partial {} / Unsupported {}{}",
            r.domain.as_str(),
            r.sections.len(),
            rows,
            counts.supported,
            counts.partial,
            counts.unsupported,
            if counts.unclassified > 0 {
                format!(" / Unclassified {}", counts.unclassified)
            } else {
                ::std::string::String::new()
            }
        );
    }
    let _ = writeln!(
        out,
        "\n  (run `bao compat <node|bun|cdp|web>` for the full report; --json for\n  machine-readable output)"
    );
    out
}

/// Four-domain summary JSON (`bao compat --json`).
pub fn render_json_root(reports: &[DomainReport]) -> String {
    let domains: Vec<serde_json::Value> = reports
        .iter()
        .map(|r| serde_json::from_str(&r.render_json()).expect("DomainReport JSON is valid"))
        .collect();
    serde_json::json!({ "domains": domains }).to_string()
}

// ─── parsing ────────────────────────────────────────────────────────────────

/// Explicit per-domain column contract: `Some(status_column_index)` when a
/// table header matches one of the domain's data tables, `None` otherwise
/// (anchor/narrative tables are excluded BY CONTRACT, never heuristically —
/// free-text status prose cannot pollute the counts).
fn status_column(domain: Domain, header: &[String]) -> Option<usize> {
    fn has(header: &[String], needle: &str) -> bool {
        header.iter().any(|c| c == needle)
    }
    match domain {
        Domain::Node => has(header, "module").then_some(4),
        Domain::Bun => {
            if has(header, "api") {
                Some(4)
            } else if has(header, "模块") {
                Some(3)
            } else if has(header, "子命令") {
                Some(2)
            } else {
                None
            }
        }
        Domain::Cdp => match header {
            [m, b, t, s]
                if m == "method" && b == "bao_impl" && t == "tests" && s == "status" =>
            {
                Some(3)
            }
            [d, m, s] if d == "domain" && m == "methods" && s == "status" => Some(2),
            _ => None,
        },
        Domain::Web => has(header, "家族").then_some(3),
    }
}

/// A status cell's claim: a classified token, an unknown token
/// (Unclassified — counted + warned, never dropped), or no claim at all
/// (empty / "—" placeholder — summary rows like cdp's 合计 carry no status
/// and are not inventory rows; the dispatch snapshot tests would flag any
/// real row that silently lost its status).
enum Claim {
    Some(Status),
    Unclassified,
    None,
}

/// Classify one status cell. Most-specific forms first; bare-token scan
/// collects the full set ("Unsupported" is removed before the "Supported"
/// probe — it contains it as a substring) and the WORST wins so compound
/// cells (web rows 7/9) roll up conservatively.
fn classify_cell(cell: &str) -> Claim {
    let c = cell.replace('*', "").trim().to_string();
    if c.is_empty() || c == "—" || c == "–" || c == "-" {
        return Claim::None;
    }
    if c.contains("Explicitly-Unsupported") || c.contains("Explicitly Unsupported") {
        return Claim::Some(Status::ExplicitlyUnsupported);
    }
    let mut found: Vec<Status> = Vec::new();
    let mut rest = c;
    for (needle, s) in [
        ("Supported(ack)", Status::SupportedAck),
        ("Partial(ack-only)", Status::PartialAckOnly),
    ] {
        if rest.contains(needle) {
            found.push(s);
            rest = rest.replace(needle, "");
        }
    }
    if rest.contains("Unsupported") {
        found.push(Status::Unsupported);
        rest = rest.replace("Unsupported", "");
    }
    if rest.contains("Supported") {
        found.push(Status::Supported);
    }
    if rest.contains("Partial") {
        found.push(Status::Partial);
    }
    found.sort();
    match found.into_iter().next() {
        Some(s) => Claim::Some(s),
        None => Claim::Unclassified,
    }
}

fn split_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let body = t
        .strip_prefix('|')
        .unwrap_or(t)
        .strip_suffix('|')
        .unwrap_or(t.strip_prefix('|').unwrap_or(t));
    body.split('|').map(|c| c.trim().to_string()).collect()
}

fn is_separator_row(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells
            .iter()
            .all(|c| {
                let t: String = c.chars().filter(|&ch| ch != ':').collect();
                let t = t.trim();
                t.is_empty() || (t.len() >= 2 && t.chars().all(|ch| ch == '-'))
            })
}

fn heading_of(line: &str) -> Option<String> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &t[hashes..];
    rest.starts_with(' ')
        .then(|| rest.trim_start().trim_end().to_string())
}

/// Anchor sections (`上游锚` / `锚定` / `协议版本锁定`): bullets and table
/// rows carried verbatim, capped at 8 lines. A table's header row (the row
/// a separator follows) is cruft, not an anchor, and is dropped.
fn anchors_of(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_anchor = false;
    let lines: Vec<&str> = content.lines().collect();
    for (idx, raw) in lines.iter().enumerate() {
        let line = raw.trim_end();
        if let Some(h) = heading_of(line) {
            in_anchor = h.contains("锚") || h.contains("协议版本");
            continue;
        }
        if !in_anchor {
            continue;
        }
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(b) = t.strip_prefix("- ") {
            out.push(b.trim().to_string());
        } else if t.starts_with('|') {
            let cells = split_cells(t);
            if is_separator_row(&cells) {
                continue;
            }
            // Header-row skip: the next table line is a separator.
            if lines
                .get(idx + 1)
                .map(|next| {
                    next.trim_start().starts_with('|')
                        && is_separator_row(&split_cells(next))
                })
                .unwrap_or(false)
            {
                continue;
            }
            out.push(cells.join(" — "));
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// Parse one embedded INVENTORY into a report (also the test seam via
/// `parse_content_for_test`).
fn parse_inventory(
    content: &'static str,
    file: &'static str,
    domain: Domain,
) -> Result<DomainReport, ParseError> {
    let mut report = DomainReport {
        domain,
        source: file,
        anchors: anchors_of(content),
        sections: Vec::new(),
    };
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0usize;
    let mut heading = String::from("(untitled)");
    while i < lines.len() {
        let line = lines[i].trim_end();
        if let Some(h) = heading_of(line) {
            heading = h;
            i += 1;
            continue;
        }
        if !line.trim_start().starts_with('|') {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && lines[i].trim_start().starts_with('|') {
            i += 1;
        }
        process_table(
            &lines[start..i],
            start + 1,
            file,
            domain,
            &heading,
            &mut report,
        )?;
    }
    Ok(report)
}

/// Contract-check + count one table block. Blocks whose header does not
/// match the domain's column contract are skipped (narrative/anchor); any
/// DATA row of a matched table that violates the header's cell count is a
/// loud ParseError.
fn process_table(
    block: &[&str],
    start_line: usize,
    file: &'static str,
    domain: Domain,
    heading: &str,
    report: &mut DomainReport,
) -> Result<(), ParseError> {
    let mut header: Option<Vec<String>> = None;
    let mut rows: Vec<(usize, Vec<String>)> = Vec::new();
    for (off, raw) in block.iter().enumerate() {
        let cells = split_cells(raw);
        if is_separator_row(&cells) {
            continue;
        }
        if header.is_none() {
            header = Some(cells);
        } else {
            rows.push((start_line + off, cells));
        }
    }
    let hcells = match header {
        Some(h) => h,
        None => return Ok(()), // separator-only block
    };
    let Some(status_col) = status_column(domain, &hcells) else {
        return Ok(()); // not one of this domain's data tables
    };
    if rows.is_empty() {
        return Ok(()); // header-only table — nothing to count
    }
    let mut section = Section {
        name: heading.to_string(),
        rows: 0,
        status: StatusCounts::default(),
    };
    for (line_no, cells) in rows {
        if cells.len() != hcells.len() {
            return Err(ParseError {
                file,
                line: line_no,
                reason: format!(
                    "table row has {} cells, header has {} ({:?})",
                    cells.len(),
                    hcells.len(),
                    hcells
                ),
            });
        }
        match classify_cell(&cells[status_col]) {
            Claim::Some(s) => {
                section.rows += 1;
                section.status.add(s);
            }
            Claim::Unclassified => {
                section.rows += 1;
                section.status.unclassified += 1;
            }
            Claim::None => {} // no status claim — not an inventory row
        }
    }
    if let Some(existing) = report
        .sections
        .iter_mut()
        .find(|s| s.name == section.name)
    {
        existing.rows += section.rows;
        existing.status.supported += section.status.supported;
        existing.status.supported_ack += section.status.supported_ack;
        existing.status.partial += section.status.partial;
        existing.status.partial_ack_only += section.status.partial_ack_only;
        existing.status.explicitly_unsupported += section.status.explicitly_unsupported;
        existing.status.unsupported += section.status.unsupported;
        existing.status.unclassified += section.status.unclassified;
    } else {
        report.sections.push(section);
    }
    Ok(())
}

/// Test seam: parse synthetic content under a domain's column contract
/// (the embedded files themselves are fixed at compile time).
#[doc(hidden)]
pub fn parse_content_for_test(
    content: &'static str,
    domain: Domain,
) -> Result<DomainReport, ParseError> {
    parse_inventory(content, "<test>", domain)
}

// ─── formatting helpers ─────────────────────────────────────────────────────

fn counts_json(c: &StatusCounts) -> serde_json::Value {
    serde_json::json!({
        "supported": c.supported,
        "supported_ack": c.supported_ack,
        "partial": c.partial,
        "partial_ack_only": c.partial_ack_only,
        "explicitly_unsupported": c.explicitly_unsupported,
        "unsupported": c.unsupported,
        "unclassified": c.unclassified,
    })
}

/// Total line: full class labels. Section line: compact S/ack + P/ackonly /
/// EU / U form (mirrors the cdp file's own `### Domain(...)` header style).
fn format_counts(rows: u32, c: &StatusCounts, full_labels: bool) -> String {
    let unclassified = if c.unclassified > 0 {
        format!(
            " / {} {}",
            if full_labels { "Unclassified" } else { "?" },
            c.unclassified
        )
    } else {
        ::std::string::String::new()
    };
    if full_labels {
        format!(
            "Total: {rows} rows — Supported {} / Supported(ack) {} / Partial {} / Partial(ack-only) {} / Explicitly-Unsupported {} / Unsupported {}{unclassified}",
            c.supported,
            c.supported_ack,
            c.partial,
            c.partial_ack_only,
            c.explicitly_unsupported,
            c.unsupported
        )
    } else {
        format!(
            "{} rows — S {} + ack {} / P {} + ackonly {} / EU {} / U {}{unclassified}",
            rows,
            c.supported,
            c.supported_ack,
            c.partial,
            c.partial_ack_only,
            c.explicitly_unsupported,
            c.unsupported
        )
    }
}
