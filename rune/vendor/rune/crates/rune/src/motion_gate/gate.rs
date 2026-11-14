//! Running an app's cells and reporting their findings against its allowlist.

use std::time::Instant;

use super::check::{Finding, RULES, Recording, check, dump, sheet};

/// A finding that is the design: `(kind, element contains, cell contains, why)`, the first three
/// matched as substrings ("" for any). Every row needs its reason.
pub type Allow = (&'static str, &'static str, &'static str, &'static str);

/// Builds and records one cell.
pub type Job = Box<dyn FnOnce() -> Recording + Send>;

/// One cell's findings.
pub struct Cell {
    pub name: String,
    pub findings: Vec<Finding>,
    pub frames: usize,
    /// Fewer than 10 frames were recorded: its setup didn't apply.
    pub skipped: bool,
}

/// An allowlist row: a finding that is the design. `kind` matches exactly ("" for any),
/// `element` as a substring, and `cell` as a substring or any of several joined by `|`
/// (`"island-auto|island-click"`). `max` caps how many findings it covers in a run, so a
/// row written for one finding doesn't quietly cover the next ten. Prefer an
/// `El::motion_intent` mark where the motion is written to a row here.
#[derive(Clone, Copy, Debug)]
pub struct AllowRow {
    pub kind: &'static str,
    pub element: &'static str,
    pub cell: &'static str,
    pub why: &'static str,
    pub max: Option<usize>,
}

impl AllowRow {
    pub const fn new(kind: &'static str, element: &'static str, cell: &'static str, why: &'static str) -> Self {
        AllowRow { kind, element, cell, why, max: None }
    }

    /// Covers at most `n` findings in a run; the rest fail.
    pub const fn max(mut self, n: usize) -> Self {
        self.max = Some(n);
        self
    }

    pub fn matches(&self, f: &Finding, cell: &str) -> bool {
        (self.kind.is_empty() || f.kind == self.kind) && f.element.contains(self.element) && self.in_cell(cell)
    }

    fn in_cell(&self, cell: &str) -> bool {
        self.cell.split('|').any(|c| cell.contains(c))
    }
}

impl From<Allow> for AllowRow {
    fn from((kind, element, cell, why): Allow) -> Self {
        AllowRow::new(kind, element, cell, why)
    }
}

/// Why `f` in `cell` is allowed, if a row of `allow` matches it.
pub fn allowed(allow: &[Allow], f: &Finding, cell: &str) -> Option<&'static str> {
    allow.iter().map(|&a| AllowRow::from(a)).find(|r| r.matches(f, cell)).map(|r| r.why)
}

fn rows(allow: &[Allow]) -> Vec<AllowRow> {
    allow.iter().map(|&a| a.into()).collect()
}

/// Runs every cell whose name contains `filter`, in parallel on large stacks, and checks each
/// recording, in the order given. `MOTION_GATE_DUMP` and `MOTION_GATE_SHEET` write evidence for
/// the cell they name.
pub fn run(cells: Vec<(String, Job)>, filter: &str) -> Vec<Cell> {
    let jobs: Vec<(usize, (String, Job))> = cells.into_iter().filter(|c| c.0.contains(filter)).enumerate().collect();
    let jobs = std::sync::Mutex::new(jobs);
    let out = std::sync::Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(12);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            std::thread::Builder::new()
                .stack_size(64 << 20)
                .spawn_scoped(scope, || loop {
                    let Some((i, (name, job))) = jobs.lock().unwrap().pop() else { break };
                    let rec = job();
                    evidence(&name, &rec, std::env::var("MOTION_GATE_OUT").ok());
                    let cell = Cell { skipped: rec.frames < 10, frames: rec.frames, findings: check(&rec), name };
                    out.lock().unwrap().push((i, cell));
                })
                .unwrap();
        }
    });
    let mut cells = out.into_inner().unwrap();
    cells.sort_by_key(|c| c.0);
    cells.into_iter().map(|c| c.1).collect()
}

/// Writes the evidence `MOTION_GATE_DUMP` and `MOTION_GATE_SHEET` ask for when their cell part
/// is `cell` or empty, beside `out`.
pub(crate) fn evidence(cell: &str, rec: &Recording, out: Option<String>) {
    if let Ok(spec) = std::env::var("MOTION_GATE_DUMP") {
        let mut p = spec.split('|');
        if let (Some(c), Some(path)) = (p.next(), p.next())
            && (c.is_empty() || c == cell)
        {
            let from = p.next().and_then(|x| x.parse().ok()).unwrap_or(0);
            let to = p.next().and_then(|x| x.parse().ok()).unwrap_or(usize::MAX);
            let text = dump(rec, path, from, to);
            match &out {
                Some(out) => std::fs::write(format!("{out}.dump"), text).unwrap(),
                None => eprint!("{text}"),
            }
        }
    }
    if let Ok(spec) = std::env::var("MOTION_GATE_SHEET") {
        let mut p = spec.split('|');
        if let (Some(c), Some(Ok(at))) = (p.next(), p.next().map(str::parse))
            && (c.is_empty() || c == cell)
        {
            let out = out.unwrap_or_else(|| std::env::temp_dir().join("motion-gate").display().to_string());
            sheet(rec, at, p.next().unwrap_or("")).save(format!("{out}.sheet.png")).unwrap();
        }
    }
}

/// What a run found.
pub struct Report {
    /// The summary line, then a line per finding that isn't allowed (and per allowed one, when
    /// asked), then what each kind found means.
    pub text: String,
    pub summary: String,
    /// Findings that aren't allowed, plus skipped cells.
    pub failing: usize,
    pub allowed: usize,
    /// Allow rows that matched nothing in the cells they scope.
    pub unused: usize,
}

/// Reads `cells` against `allow`; `list_allowed` lists allowed findings too.
pub fn report(cells: &[Cell], allow: &[Allow], list_allowed: bool, secs: f32) -> Report {
    report_rows(cells, &rows(allow), list_allowed, secs)
}

/// [`report`] with [`AllowRow`]s. A row covers findings up to its `max`; past it they fail.
/// Rows that matched nothing in the cells they scope are listed as unused.
pub fn report_rows(cells: &[Cell], rows: &[AllowRow], list_allowed: bool, secs: f32) -> Report {
    report_with(cells, rows, list_allowed, true, secs)
}

/// `whole`: every cell ran, so a row that matched nothing is worth listing.
fn report_with(cells: &[Cell], rows: &[AllowRow], list_allowed: bool, whole: bool, secs: f32) -> Report {
    let mut used = vec![0usize; rows.len()];
    let mut text = String::new();
    let (mut failing, mut allowed_n) = (0, 0);
    let mut failed_kinds: Vec<&str> = Vec::new();
    for c in cells {
        if c.skipped {
            failing += 1;
            text += &format!("SKIP {} (its setup didn't apply)\n", c.name);
        }
        for f in &c.findings {
            let row = rows.iter().position(|r| r.matches(f, &c.name));
            let over = row.and_then(|i| {
                used[i] += 1;
                rows[i].max.filter(|&m| used[i] > m)
            });
            match (row, over) {
                (Some(i), None) => {
                    allowed_n += 1;
                    if list_allowed {
                        text += &format!("allowed  {}  in {}  ({})\n", f, c.name, rows[i].why);
                    }
                }
                _ => {
                    failing += 1;
                    failed_kinds.push(f.kind);
                    let past = over.map_or(String::new(), |m| format!("  (past its allow row's max of {m})"));
                    text += &format!("{}  in {}{past}\n", f, c.name);
                }
            }
        }
    }
    let ran = |r: &AllowRow| cells.iter().any(|c| !c.skipped && r.in_cell(&c.name));
    let unused: Vec<&AllowRow> = rows.iter().zip(&used).filter(|(r, n)| whole && **n == 0 && ran(r)).map(|(r, _)| r).collect();
    for r in &unused {
        text += &format!("unused allow row (\"{}\", \"{}\", \"{}\"): {}\n", r.kind, r.element, r.cell, r.why);
    }
    let frames: usize = cells.iter().map(|c| c.frames).sum();
    let summary = format!("motion gate: {} cells, {frames} frames, {failing} findings ({allowed_n} allowed, {} unused allow rows) in {secs:.1} s\n", cells.len(), unused.len());
    failed_kinds.sort();
    failed_kinds.dedup();
    for r in RULES.iter().filter(|r| failed_kinds.contains(&r.kind)) {
        text += &format!("  {}: {} ({})\n", r.kind, r.means, r.threshold);
    }
    Report { text: summary.clone() + &text, summary, failing, allowed: allowed_n, unused: unused.len() }
}

/// The gate as a test: runs the cells `MOTION_GATE` keeps, writes the report to
/// `MOTION_GATE_OUT`, and panics with it when a finding isn't allowed. Unused allow rows are
/// listed only when `MOTION_GATE` is unset, since a filtered run leaves most rows idle.
pub fn gate(cells: Vec<(String, Job)>, allow: &[Allow]) {
    gate_rows(cells, &rows(allow));
}

/// [`gate`] with [`AllowRow`]s (cell scopes and match caps).
pub fn gate_rows(cells: Vec<(String, Job)>, allow: &[AllowRow]) {
    let filter = std::env::var("MOTION_GATE").unwrap_or_default();
    let started = Instant::now();
    let cells = run(cells, &filter);
    let r = report_with(&cells, allow, std::env::var_os("MOTION_GATE_ALLOWED").is_some(), filter.is_empty(), started.elapsed().as_secs_f32());
    if let Ok(path) = std::env::var("MOTION_GATE_OUT") {
        std::fs::write(path, &r.text).unwrap();
    }
    assert!(r.failing == 0, "{}", r.text);
    eprint!("{}", r.summary);
}
