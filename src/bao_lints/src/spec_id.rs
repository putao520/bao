//! SPEC API element id-format detector — REQ-SPEC-001 防复发实现。
//!
// @trace REQ-SPEC-001
//
//! 扫描 `.spec/*.html`,定位带 `data-api=` 属性的 `<section>`/`<div>` 元素,
//! 校验其 `id` 属性是否符合 `API-{DOMAIN}-{N}` 格式。命中以下「method-path」
//! 退化形态即报错违规:
//!   - `id="post-/..."` / `id="get-/..."` / `id="put-/..."` / `id="delete-/..."`
//!     / `id="patch-/..."`  (HTTP 方法 + 路径开头)
//!   - `id="/vm/sandbox"`                                 (纯路径开头)
//!   - 缺失 `id` 属性的 `data-api=` 元素
//!
//! 例外(REQ-SPEC-001 明确豁免,不算违规):
//!   - `id="bao-cdp-client::..."`  —— 子系统 section(`data-module=`),非 API 元素
//!   - `id="bao-cdp-client-..."`   —— criterion div,非 API 元素
//!   - 复用既有"非 API-{DOMAIN}-{N}"命名但本就不是 API 元素的 artifact/section
//!     (例如 `id="artifact-interface-protocol"`、`id="artifact-module"`):
//!     这些是历史遗留的 artifact section 复用,REQ-SPEC-001 的约束范围仅限真正的
//!     API 元素。本检测器通过「同时具备 `data-api=` 且 id 非 API- 形态」识别,
//!     artifact 类 id 即便复用了 data-api 也会被报出(它们正是要逐步迁移的目标)。
//!
//! Baseline 机制:
//!   CI 门禁用 `--baseline` 传入一个已知违规清单文件(每行一个 id),
//!   清单内的违规被抑制(只报新增违规),让历史技术债务可追踪而不阻断 CI。
//!   本检测器本体无 baseline 时报全部违规——这是 SSOT,让单元测试可精确验证。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// 一个 SPEC id-format 违规。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SpecIdFinding {
    pub file: String,
    pub line: usize,
    pub id: String,
    pub reason: Reason,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Reason {
    /// id 形如 `post-/...` / `get-/...` 等 HTTP 方法 + 路径开头。
    MethodPath,
    /// id 形如 `/vm/sandbox` —— 纯路径开头,无 HTTP 方法前缀。
    PathOnly,
    /// 元素带 `data-api=` 但完全没有 `id` 属性。
    MissingId,
    /// id 既不是 `API-{DOMAIN}-{N}` 也不属于豁免类别(其他畸形)。
    Other,
}

impl SpecIdFinding {
    pub fn render(&self) -> String {
        let kind = match self.reason {
            Reason::MethodPath => "method-path id",
            Reason::PathOnly => "path-only id",
            Reason::MissingId => "missing id",
            Reason::Other => "non-API-{DOMAIN}-{N} id",
        };
        format!(
            "{}:{}: REQ-SPEC-001 violation: {} (id={:?})",
            self.file, self.line, kind, self.id
        )
    }
}

/// 扫描单个 HTML 文本,返回所有 REQ-SPEC-001 违规(不去重、不应用 baseline)。
///
/// `file_path` 仅用于报告;`src` 是 UTF-8 HTML 文本。
pub fn scan_html(file_path: &Path, src: &str) -> Vec<SpecIdFinding> {
    let mut out = Vec::new();
    let file_str = file_path.display().to_string();
    for element in iter_api_elements(src) {
        let id_opt = element.id.as_deref();
        let reason = match id_opt {
            None => Some(Reason::MissingId),
            Some(id) => classify_id(id),
        };
        if let Some(reason) = reason {
            out.push(SpecIdFinding {
                file: file_str.clone(),
                line: element.line,
                id: id_opt.unwrap_or("<missing>").to_string(),
                reason,
            });
        }
    }
    out
}

/// 判定一个 id 是否违规;返回 `Some(reason)` 表示违规,`None` 表示合规或豁免。
pub fn classify_id(id: &str) -> Option<Reason> {
    // 合规:API-{DOMAIN}-{N},DOMAIN ∈ 项目认可的 10 个子域。
    if is_valid_api_id(id) {
        return None;
    }
    // 豁免:bao-cdp-client::... 子系统 section(非 API 元素)。
    if id.starts_with("bao-cdp-client::") {
        return None;
    }
    // 豁免:bao-cdp-client-... criterion div(非 API 元素)。
    if id.starts_with("bao-cdp-client-") {
        return None;
    }
    // 违规:HTTP 方法 + 路径(method-path 退化形态)。
    let lower = id.to_ascii_lowercase();
    for method in ["post-", "get-", "put-", "delete-", "patch-"] {
        if lower.starts_with(method) {
            return Some(Reason::MethodPath);
        }
    }
    // 违规:纯路径开头(无 HTTP 方法前缀)。
    if id.starts_with('/') {
        return Some(Reason::PathOnly);
    }
    // 其他非 API-{DOMAIN}-{N} 形态(例如 artifact-*、req-* 等误用 data-api=)。
    Some(Reason::Other)
}

/// 校验 `API-{DOMAIN}-{N}` 格式;DOMAIN 必须是项目认可的 10 个子域之一。
///
/// 10 个子域:ENG / CDP / STL / CLI / BRW / LIB / BAO-API / PERF / IMPL / CDP-UWS
/// (与 REQ-SPEC-001 定义一致)。注意 `BAO-API` 和 `CDP-UWS` 含连字符。
pub fn is_valid_api_id(id: &str) -> bool {
    let valid_domains: &[&str] = &[
        "ENG", "CDP", "STL", "CLI", "BRW", "LIB", "BAO-API", "PERF", "IMPL", "CDP-UWS",
    ];
    // API-<DOMAIN>-<N>,其中 DOMAIN 可能含连字符(BAO-API),N 为整数。
    let rest = match id.strip_prefix("API-") {
        Some(r) => r,
        None => return false,
    };
    // 从尾部切下最后的 -N(N 必须是纯数字)。
    let last_dash = match rest.rfind('-') {
        Some(i) => i,
        None => return false,
    };
    let (domain_part, num_part) = (&rest[..last_dash], &rest[last_dash + 1..]);
    if num_part.is_empty() || !num_part.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    valid_domains.iter().any(|d| *d == domain_part)
}

/// 解析出的一个 `data-api=` 元素的开标签快照。
struct ApiElement {
    line: usize,
    id: Option<String>,
}

/// 遍历 HTML 文本,产出所有带 `data-api=` 属性的 `<section>`/`<div>` 开标签。
///
/// 不依赖外部 HTML 解析器——用简单的字符级扫描定位每一个 `<section ...>` /
/// `<div ...>` 开标签(SPEC HTML 经常把多个 section 压在一行,所以必须扫整行
/// 而非只看行首)。同时记录每个开标签所在行号。本扫描器是 SSOT,故意不处理
/// 畸形 HTML;若 SPEC 文件结构异常,应先修 SPEC。
fn iter_api_elements(src: &str) -> Vec<ApiElement> {
    let mut out = Vec::new();
    for (line_idx, raw_line) in src.lines().enumerate() {
        let line_no = line_idx + 1;
        // 快速过滤:这一行必须含 `data-api=`,否则不可能命中。
        if !raw_line.contains("data-api=") {
            continue;
        }
        // 在该行内找所有 `<section` / `<div` 起始位置,逐一截取开标签。
        let mut search_from = 0usize;
        while let Some(rel_idx) = find_tag_start(raw_line, search_from) {
            let abs_idx = rel_idx;
            let tag_end = match raw_line[abs_idx..].find('>') {
                Some(i) => abs_idx + i,
                None => break,
            };
            let tag = &raw_line[abs_idx..tag_end];
            if tag.contains("data-api=") {
                let id = extract_attr(tag, "id");
                out.push(ApiElement { line: line_no, id });
            }
            search_from = tag_end + 1;
        }
    }
    out
}

/// 在 `line[search_from..]` 中查找下一个 `<section` 或 `<div` 起始位置,
/// 返回相对于整行的字节偏移。未找到返回 `None`。
fn find_tag_start(line: &str, search_from: usize) -> Option<usize> {
    let tail = line.get(search_from..)?;
    let rel = tail
        .find("<section")
        .into_iter()
        .chain(tail.find("<div"))
        .min()?;
    Some(search_from + rel)
}

/// 从一个开标签字符串中提取指定属性的值(双引号包覆)。未找到返回 `None`。
fn extract_attr(tag: &str, name: &str) -> Option<String> {
    // 匹配 ` name="..."`(前导空白),捕获引号内的值。
    let needle = format!(" {}=\"", name);
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 递归收集一个目录(或单个文件)下所有 `.html` 文件路径。
fn collect_html_files(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if path.is_file() {
        if path.extension().and_then(|e| e.to_str()) == Some("html") {
            out.push(path.to_path_buf());
        }
        return out;
    }
    if !path.is_dir() {
        return out;
    }
    for entry in WalkDir::new(path).into_iter().filter_map(Result::ok) {
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("html") {
            out.push(p.to_path_buf());
        }
    }
    out.sort();
    out
}

/// 扫描结果:包含所有违规与命中文件数。
pub struct ScanResult {
    pub findings: Vec<SpecIdFinding>,
    pub files_scanned: usize,
    /// baseline 文件中的总条目数（含注释和空行以外的有效行）。
    pub baseline_total: usize,
    /// baseline 中实际命中（抑制了 findings）的条目数。
    pub baseline_matched: usize,
    /// baseline 中未命中任何 finding 的条目数（幻影/死条目）。
    pub baseline_unmatched: usize,
}

/// 扫描一个路径下所有 `.html` 文件,返回违规清单。
///
/// `baseline_path` 若提供,则其中列出的 id 会被抑制(每行一个 id,`#` 起始为注释)。
/// 这是 CI 门禁用于追踪历史技术债务的运维旋钮——检测器本体(`scan_html`)仍 SSOT。
pub fn scan_path(path: &Path, baseline_path: Option<&Path>) -> std::io::Result<ScanResult> {
    let baseline = match baseline_path {
        Some(p) => load_baseline(p)?,
        None => HashSet::new(),
    };
    let baseline_total = baseline.len();
    let files = collect_html_files(path);
    let mut findings = Vec::new();
    let mut matched_ids: HashSet<String> = HashSet::new();
    for file in &files {
        let src = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(_e) => continue,
        };
        for f in scan_html(file, &src) {
            if baseline.contains(&f.id) {
                matched_ids.insert(f.id.clone());
                continue;
            }
            findings.push(f);
        }
    }
    Ok(ScanResult {
        findings,
        files_scanned: files.len(),
        baseline_total,
        baseline_matched: matched_ids.len(),
        baseline_unmatched: baseline_total - matched_ids.len(),
    })
}

fn load_baseline(path: &Path) -> std::io::Result<HashSet<String>> {
    let txt = std::fs::read_to_string(path)?;
    Ok(txt
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(id: &str) -> Option<Reason> {
        classify_id(id)
    }

    // ─── 正向:合规 id 不报 ────────────────────────────────────────────────

    #[test]
    fn valid_api_ids_are_accepted() {
        assert_eq!(classify("API-ENG-001"), None);
        assert_eq!(classify("API-CDP-042"), None);
        assert_eq!(classify("API-BAO-API-007"), None, "BAO-API 含连字符");
        assert_eq!(classify("API-IMPL-12"), None);
        assert!(is_valid_api_id("API-STL-1"));
    }

    #[test]
    fn exempt_bao_cdp_client_ids_are_accepted() {
        // 子系统 section(module)与 criterion div 不属于 API 元素,REQ-SPEC-001 豁免。
        assert_eq!(classify("bao-cdp-client::transport"), None);
        assert_eq!(classify("bao-cdp-client::api"), None);
        assert_eq!(classify("bao-cdp-client-some-criterion"), None);
    }

    // ─── 负向:method-path / path-only / missing 必报 ───────────────────────

    #[test]
    fn method_path_ids_are_flagged() {
        assert_eq!(classify("post-/vm/sandbox"), Some(Reason::MethodPath));
        assert_eq!(classify("get-/json/version"), Some(Reason::MethodPath));
        assert_eq!(classify("delete-/json/close/{targetid}"), Some(Reason::MethodPath));
        assert_eq!(classify("PUT-/foo"), Some(Reason::MethodPath), "大小写不敏感");
    }

    #[test]
    fn path_only_ids_are_flagged() {
        assert_eq!(classify("/vm/sandbox"), Some(Reason::PathOnly));
        assert_eq!(classify("/json/list"), Some(Reason::PathOnly));
    }

    #[test]
    fn other_malformed_ids_are_flagged() {
        // 非 API-{DOMAIN}-{N}、非豁免类别 → Other
        assert_eq!(classify("artifact-interface-protocol"), Some(Reason::Other));
        assert_eq!(classify("artifact-module"), Some(Reason::Other));
        assert_eq!(classify("API-FOO-1"), Some(Reason::Other), "未认可的 DOMAIN");
        assert_eq!(classify("API-ENG-"), Some(Reason::Other), "缺 N");
        assert_eq!(classify("API-ENG-x"), Some(Reason::Other), "N 非数字");
    }

    // ─── HTML 扫描:定位 data-api= 元素 ─────────────────────────────────────

    #[test]
    fn scan_html_finds_method_path_violation() {
        // 注意:raw string 起始的换行算第 1 行,`<section>` 在第 3 行。
        let html = r#"
<html>
<section data-api="POST /vm/sandbox" id="post-/vm/sandbox">
  <p>bad</p>
</section>
</html>
"#;
        let path = Path::new("test.html");
        let findings = scan_html(path, html);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "post-/vm/sandbox");
        assert_eq!(findings[0].reason, Reason::MethodPath);
        assert_eq!(findings[0].line, 3);
    }

    #[test]
    fn scan_html_accepts_valid_api_section() {
        let html = r#"<section data-api="POST /foo" id="API-ENG-001"></section>"#;
        let findings = scan_html(Path::new("t.html"), html);
        assert!(findings.is_empty());
    }

    #[test]
    fn scan_html_exempts_bao_cdp_client_module_section() {
        // 这个 section 带有 data-module= 而非 data-api=,本就不该被扫到;
        // 但即便误标了 data-api=,id 也应被豁免。
        let html = r#"<section data-api="POST /cdp-client/connect" id="bao-cdp-client::transport"></section>"#;
        let findings = scan_html(Path::new("t.html"), html);
        assert!(findings.is_empty(), "bao-cdp-client:: 应豁免, got {:?}", findings);
    }

    #[test]
    fn scan_html_reports_missing_id() {
        let html = r#"<section data-api="POST /foo"></section>"#;
        let findings = scan_html(Path::new("t.html"), html);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].reason, Reason::MissingId);
    }

    #[test]
    fn scan_html_does_not_chase_closing_tags_or_non_api_sections() {
        let html = r#"
<section id="req-spec-001" data-req="REQ-SPEC-001"><h3>REQ</h3></section>
<div data-api="GET /x" id="get-/x">bad</div>
<section data-api="POST /y" id="API-CDP-9">good</section>
"#;
        let findings = scan_html(Path::new("t.html"), html);
        // 只报 get-/x;req-spec-001 没有 data-api= 不算,API-CDP-9 合规。
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "get-/x");
    }

    #[test]
    fn scan_html_finds_multiple_sections_packed_on_one_line() {
        // SPEC HTML 经常把多个 section 压在一行;扫描器必须全部命中而非只取行首。
        let html = r#"<section data-api="POST /a" id="post-/a"></section><section data-api="POST /b" id="post-/b"></section>"#;
        let findings = scan_html(Path::new("t.html"), html);
        let ids: Vec<_> = findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["post-/a", "post-/b"]);
    }

    // ─── Baseline 机制 ─────────────────────────────────────────────────────

    #[test]
    fn baseline_suppresses_listed_ids() {
        let dir = tempdir_with("test_spec", &[
            ("a.html", r#"<section data-api="POST /old" id="post-/old"></section>"#),
            ("b.html", r#"<section data-api="POST /new" id="post-/new"></section>"#),
        ]);
        let baseline = dir.join("baseline.txt");
        std::fs::write(&baseline, "# historical\npost-/old\n").unwrap();

        let result = scan_path(&dir, Some(&baseline)).unwrap();
        let ids: Vec<_> = result.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["post-/new"], "baseline 应抑制 post-/old");
    }

    fn tempdir_with(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("bao_lints_spec_id_{}_{}", name, std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        for (fname, content) in files {
            std::fs::write(p.join(fname), content).unwrap();
        }
        // baseline.txt 故意不在这里创建——由调用方写。
        let _ = std::fs::remove_file(p.join("baseline.txt"));
        p
    }

    #[test]
    fn valid_cdp_uws_domain_is_accepted() {
        assert_eq!(classify("API-CDP-UWS-001"), None, "CDP-UWS 含连字符");
        assert_eq!(classify("API-CDP-UWS-005"), None);
        assert!(is_valid_api_id("API-CDP-UWS-42"));
    }

    #[test]
    fn scan_path_baseline_stats() {
        let dir = tempdir_with("baseline_stats", &[
            ("a.html", r#"<section data-api="POST /old" id="post-/old"></section>"#),
            ("b.html", r#"<section data-api="POST /new" id="post-/new"></section>"#),
        ]);
        let baseline = dir.join("baseline.txt");
        std::fs::write(&baseline, "# historical\npost-/old\nphantom-entry\n").unwrap();

        let result = scan_path(&dir, Some(&baseline)).unwrap();
        assert_eq!(result.baseline_total, 2, "baseline has 2 effective entries");
        assert_eq!(result.baseline_matched, 1, "post-/old matched");
        assert_eq!(result.baseline_unmatched, 1, "phantom-entry did not match");
        assert_eq!(result.findings.len(), 1, "only post-/new is reported");
    }
}
