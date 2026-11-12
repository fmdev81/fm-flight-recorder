//! The shared table kit of the list pages (Layouts, Scripts, Freezes, and
//! later Users and Clients).
//!
//! A page builds a [`Table`]: columns, rows of [`Cell`]s, a default sort.
//! [`Table::apply`] then reads the page state from the URL (`sort=KEY`,
//! `dir=asc|desc`, `q=TEXT`), filters and sorts the rows, and builds the
//! header links, so the page reads well without JavaScript. The template
//! `table.html` (macro `tk::table`) draws it. With JavaScript, `static/app.js`
//! sorts and filters the rows in the page and keeps the same three
//! parameters in the URL, so a link, a reload and a live update keep them.
//!
//! The row order is the same in Rust and in JS: a number column sorts by
//! its number, a text column sorts by its text (case does not matter), and
//! equal rows sort by the text of the first column.

use crate::views::{Params, Split};
use std::cmp::Ordering;

/// The value that a column sorts by.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Num(f64),
    Text(String),
}

impl Default for Val {
    fn default() -> Val {
        Val::Text(String::new())
    }
}

impl Val {
    /// The text for `data-v`.
    pub fn attr(&self) -> String {
        match self {
            Val::Num(n) => format!("{n}"),
            Val::Text(t) => t.clone(),
        }
    }

    fn cmp(&self, o: &Val) -> Ordering {
        match (self, o) {
            (Val::Num(a), Val::Num(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (a, b) => a.attr().cmp(&b.attr()),
        }
    }
}

/// One cell. Build it with [`Cell::text`] or [`Cell::num`] and the
/// builder methods.
#[derive(Debug, Clone, Default)]
pub struct Cell {
    pub text: String,
    pub val: Val,
    /// A link inside the cell.
    pub url: String,
    /// `"text"`, `"num"`, `"split"` or `"tags"`.
    pub kind: &'static str,
    pub muted: bool,
    pub mono: bool,
    pub split: Split,
    pub tags: Vec<String>,
    /// A flag chip that replaces the text (for example `p95 12 s`).
    pub flag: String,
    pub title: String,
    /// A status dot before the text: `"ok"` or `"off"`.
    pub dot: &'static str,
    /// A flag chip after the text (for example `silent 20 min`).
    pub chip: String,
}

impl Cell {
    pub fn text(s: impl Into<String>) -> Cell {
        let text: String = s.into();
        Cell {
            val: Val::Text(text.to_lowercase()),
            text,
            kind: "text",
            ..Default::default()
        }
    }

    /// A number cell: right aligned, tabular figures, sorts by `v`.
    pub fn num(text: impl Into<String>, v: f64) -> Cell {
        Cell {
            text: text.into(),
            val: Val::Num(v),
            kind: "num",
            mono: true,
            ..Default::default()
        }
    }

    /// A number cell with no value ("—"). It sorts below every number.
    pub fn none() -> Cell {
        Cell::num("—", -1.0)
    }

    pub fn split(sp: Split, v: f64) -> Cell {
        Cell {
            split: sp,
            val: Val::Num(v),
            kind: "split",
            ..Default::default()
        }
    }

    pub fn tags(t: Vec<String>) -> Cell {
        Cell {
            val: Val::Text(t.join(" ")),
            tags: t,
            kind: "tags",
            ..Default::default()
        }
    }

    /// Sort by a number instead of the text.
    pub fn sort(mut self, v: f64) -> Cell {
        self.val = Val::Num(v);
        self
    }
    pub fn link(mut self, url: impl Into<String>) -> Cell {
        self.url = url.into();
        self
    }
    pub fn muted(mut self) -> Cell {
        self.muted = true;
        self
    }
    pub fn mono(mut self) -> Cell {
        self.mono = true;
        self
    }
    pub fn flag(mut self, s: impl Into<String>) -> Cell {
        self.flag = s.into();
        self
    }
    pub fn dot(mut self, d: &'static str) -> Cell {
        self.dot = d;
        self
    }
    pub fn chip(mut self, s: impl Into<String>) -> Cell {
        self.chip = s.into();
        self
    }
    pub fn title(mut self, s: impl Into<String>) -> Cell {
        self.title = s.into();
        self
    }
}

/// A column. `num` columns are right aligned and sort high to low first.
#[derive(Debug, Clone, Default)]
pub struct Col {
    pub key: String,
    pub label: String,
    pub num: bool,
    pub tip: String,
    /// The header link (set by [`Table::apply`]).
    pub href: String,
    /// `"ascending"`, `"descending"` or `"none"`.
    pub aria: &'static str,
    /// `"asc"`, `"desc"` or empty: the arrow.
    pub arrow: &'static str,
}

impl Col {
    pub fn text(key: &str, label: &str) -> Col {
        Col {
            key: key.into(),
            label: label.into(),
            ..Default::default()
        }
    }
    pub fn num(key: &str, label: &str) -> Col {
        Col {
            num: true,
            ..Col::text(key, label)
        }
    }
    pub fn tip(mut self, t: &str) -> Col {
        self.tip = t.into();
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// The detail page (a click on the row opens it).
    pub url: String,
    /// The text that the filter box searches (lower case).
    pub search: String,
    pub flagged: bool,
    /// For `data-id` (live updates mark new rows by it).
    pub id: String,
    /// A freeze that still lasts: the row pulses.
    pub ongoing: bool,
    /// The filter hides the row. It stays in the page so that JS can show
    /// it again when the filter gets shorter or is cleared.
    pub hidden: bool,
}

impl Row {
    pub fn new(cells: Vec<Cell>, url: impl Into<String>) -> Row {
        let search = cells
            .iter()
            .map(|c| {
                if c.kind == "tags" {
                    c.tags.join(" ")
                } else {
                    c.text.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        Row {
            cells,
            url: url.into(),
            search,
            ..Default::default()
        }
    }
    pub fn flagged(mut self, f: bool) -> Row {
        self.flagged = f;
        self
    }
    /// Mark the row as a freeze that still lasts.
    pub fn ongoing(mut self, on: bool) -> Row {
        self.ongoing = on;
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Row {
        self.id = id.into();
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct Table {
    /// The id of the live part (`data-live`).
    pub id: String,
    pub one: String,
    pub many: String,
    pub cols: Vec<Col>,
    pub rows: Vec<Row>,
    /// The totals row (below the rows; the filter does not change it).
    pub foot: Vec<Cell>,
    pub sort: String,
    /// `"asc"` or `"desc"`.
    pub dir: String,
    pub q: String,
    /// All rows before the filter.
    pub total: usize,
    /// The path and query without sort, dir and q.
    pub base: String,
    /// Hidden form fields for the filter box (all other parameters).
    pub keep: Vec<(String, String)>,
    /// Sort and filter run on the server (the page loads more rows):
    /// JS reloads the page part instead of sorting rows in the page.
    pub server: bool,
    /// A short note below the table, and a link at its end.
    pub note: String,
    pub note_link: (String, String),
    /// A segmented control in the toolbar: (url, label, pressed).
    pub seg: Vec<(String, String, bool)>,
    pub path: String,
    /// A table in the middle of a detail page: its box has a height limit.
    pub inline: bool,
    /// The URL of the next rows (Load more), when the page loads in parts.
    pub more: String,
    /// Rows shown and rows in all, when the page loads the rows in parts.
    pub shown: Option<usize>,
    pub grand: Option<usize>,
    pub empty: String,
}

/// `desc` or `asc`; anything else is `None`.
fn dir_of(s: &str) -> Option<&'static str> {
    match s {
        "asc" => Some("asc"),
        "desc" => Some("desc"),
        _ => None,
    }
}

impl Table {
    /// A table whose default order is `sort` / `dir`.
    pub fn new(id: &str, one: &str, many: &str, cols: Vec<Col>, sort: &str, dir: &str) -> Table {
        Table {
            id: id.into(),
            one: one.into(),
            many: many.into(),
            cols,
            sort: sort.into(),
            dir: dir.into(),
            empty: format!("No {many}"),
            ..Default::default()
        }
    }

    /// Read `sort`, `dir` and `q` from the page parameters, set the base
    /// of the links, and (unless `server`) filter and sort the rows.
    pub fn apply(mut self, path: &str, p: &Params) -> Table {
        self.read(p);
        self.path = path.into();
        self.base = base_of(path, p);
        self.keep = p
            .raw
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "sort" | "dir" | "q" | "before" | "shown"))
            .cloned()
            .collect();
        self.total = self.rows.len();
        if !self.server {
            self.hide();
            self.order();
        }
        self.links();
        self
    }

    /// The state of the URL (a column that does not exist falls back to
    /// the default order).
    pub fn read(&mut self, p: &Params) {
        if !p.sort.is_empty() && self.cols.iter().any(|c| c.key == p.sort) {
            self.sort = p.sort.clone();
            self.dir = dir_of(&p.dir)
                .map(String::from)
                .unwrap_or_else(|| self.first_dir(&p.sort));
        }
        self.q = p.q.trim().to_string();
    }

    fn first_dir(&self, key: &str) -> String {
        if self.cols.iter().any(|c| c.key == key && c.num) {
            "desc".into()
        } else {
            "asc".into()
        }
    }

    fn terms(&self) -> Vec<String> {
        self.q
            .to_lowercase()
            .split_whitespace()
            .map(String::from)
            .collect()
    }

    /// Remove the rows that do not match the filter (a `server` table: the
    /// server answers again when the filter changes).
    pub fn filter(&mut self) {
        let terms = self.terms();
        if !terms.is_empty() {
            self.rows
                .retain(|r| terms.iter().all(|t| r.search.contains(t.as_str())));
        }
    }

    /// Mark the rows that do not match the filter as hidden. They stay in
    /// the page: JS filters the rows in the page, so a row the server left
    /// out could not come back when the filter is cleared.
    pub fn hide(&mut self) {
        let terms = self.terms();
        for r in &mut self.rows {
            r.hidden = !terms.iter().all(|t| r.search.contains(t.as_str()));
        }
    }

    /// The rows that the filter does not hide.
    pub fn visible(&self) -> usize {
        self.rows.iter().filter(|r| !r.hidden).count()
    }

    pub fn order(&mut self) {
        let Some(i) = self.cols.iter().position(|c| c.key == self.sort) else {
            return;
        };
        let desc = self.dir == "desc";
        self.rows.sort_by(|a, b| {
            let o = a.cells[i].val.cmp(&b.cells[i].val);
            let o = if desc { o.reverse() } else { o };
            o.then_with(|| a.cells[0].val.cmp(&b.cells[0].val))
        });
    }

    /// The header links and the arrow.
    pub fn links(&mut self) {
        let (sort, dir) = (self.sort.clone(), self.dir.clone());
        let first: Vec<String> = self.cols.iter().map(|c| self.first_dir(&c.key)).collect();
        let sep = if self.base.contains('?') { '&' } else { '?' };
        let q = if self.q.is_empty() {
            String::new()
        } else {
            format!("&q={}", crate::fmt::enc(&self.q))
        };
        for (c, f) in self.cols.iter_mut().zip(first) {
            let next = if c.key == sort {
                if dir == "asc" {
                    "desc"
                } else {
                    "asc"
                }
            } else {
                f.as_str()
            };
            c.href = format!("{}{sep}sort={}&dir={next}{q}", self.base, c.key);
            (c.aria, c.arrow) = if c.key == sort {
                if dir == "asc" {
                    ("ascending", "asc")
                } else {
                    ("descending", "desc")
                }
            } else {
                ("none", "")
            };
        }
    }

    /// "42 scripts", or "3 of 42 scripts" while the filter hides rows.
    pub fn count_text(&self) -> String {
        let n = self.shown.unwrap_or_else(|| self.visible());
        let all = self.grand.unwrap_or(self.total);
        let noun = |n: usize| if n == 1 { &self.one } else { &self.many };
        if all == n {
            format!("{} {}", crate::fmt::num(n as u64), noun(n))
        } else {
            format!(
                "{} of {} {}",
                crate::fmt::num(n as u64),
                crate::fmt::num(all as u64),
                noun(all)
            )
        }
    }

    /// The text for an empty table.
    pub fn empty_text(&self) -> String {
        if self.q.is_empty() {
            self.empty.clone()
        } else {
            format!("No {} match “{}”", self.many, self.q)
        }
    }

    /// The page URL with the table state (for tests and links).
    pub fn url(&self) -> String {
        let sep = if self.base.contains('?') { '&' } else { '?' };
        let mut s = format!("{}{sep}sort={}&dir={}", self.base, self.sort, self.dir);
        if !self.q.is_empty() {
            s.push_str(&format!("&q={}", crate::fmt::enc(&self.q)));
        }
        s
    }
}

/// The path with every parameter except the table state and the zoom.
fn base_of(path: &str, p: &Params) -> String {
    let qs: Vec<String> = p
        .raw
        .iter()
        .filter(|(k, _)| {
            !matches!(
                k.as_str(),
                "sort" | "dir" | "q" | "before" | "shown" | "zoom" | "span"
            )
        })
        .map(|(k, v)| format!("{}={}", crate::fmt::enc(k), crate::fmt::enc(v)))
        .collect();
    if qs.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", qs.join("&"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn params(kv: &[(&str, &str)]) -> Params {
        let m: HashMap<String, String> = kv
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Params::from_map(&m)
    }

    fn table() -> Table {
        let mut t = Table::new(
            "t",
            "script",
            "scripts",
            vec![Col::text("name", "Script"), Col::num("n", "Freezes")],
            "n",
            "desc",
        );
        for (name, n) in [
            ("Beta", 5.0),
            ("alpha", 9.0),
            ("Gamma", 5.0),
            ("delta", 1.0),
        ] {
            t.rows.push(Row::new(
                vec![Cell::text(name), Cell::num(format!("{n}"), n)],
                format!("/script?name={name}"),
            ));
        }
        t
    }

    /// The rows in view (not hidden by the filter), in order.
    fn names(t: &Table) -> Vec<String> {
        t.rows
            .iter()
            .filter(|r| !r.hidden)
            .map(|r| r.cells[0].text.clone())
            .collect()
    }

    #[test]
    fn default_order_is_the_page_default() {
        let t = table().apply("/scripts", &params(&[("range", "7d")]));
        // 9, then the two 5s by name (case does not matter), then 1.
        assert_eq!(names(&t), ["alpha", "Beta", "Gamma", "delta"]);
        assert_eq!((t.sort.as_str(), t.dir.as_str()), ("n", "desc"));
    }

    #[test]
    fn sort_and_dir_come_from_the_url() {
        let t = table().apply("/scripts", &params(&[("sort", "name"), ("dir", "desc")]));
        assert_eq!(names(&t), ["Gamma", "delta", "Beta", "alpha"]);
        // A text column starts ascending, a number column descending.
        let t = table().apply("/scripts", &params(&[("sort", "name")]));
        assert_eq!(names(&t), ["alpha", "Beta", "delta", "Gamma"]);
        let t = table().apply("/scripts", &params(&[("sort", "n"), ("dir", "asc")]));
        assert_eq!(names(&t), ["delta", "Beta", "Gamma", "alpha"]);
    }

    #[test]
    fn a_column_without_a_key_never_sorts() {
        let mut t = table();
        t.cols.push(Col::text("", "Bar"));
        let t = t.apply("/scripts", &params(&[]));
        assert_eq!(t.sort, "n");
        let t = table().apply("/scripts", &params(&[("sort", "")]));
        assert_eq!(t.sort, "n");
    }

    #[test]
    fn an_unknown_column_falls_back_to_the_default() {
        let t = table().apply("/scripts", &params(&[("sort", "nope"), ("dir", "asc")]));
        assert_eq!((t.sort.as_str(), t.dir.as_str()), ("n", "desc"));
    }

    #[test]
    fn filter_matches_every_word_in_any_cell() {
        let t = table().apply("/scripts", &params(&[("q", "A")]));
        assert_eq!(names(&t), ["alpha", "Beta", "Gamma", "delta"]);
        let t = table().apply("/scripts", &params(&[("q", "  al ")]));
        assert_eq!(names(&t), ["alpha"]);
        let t = table().apply("/scripts", &params(&[("q", "a 9")]));
        assert_eq!(names(&t), ["alpha"]);
        let t = table().apply("/scripts", &params(&[("q", "zzz")]));
        assert_eq!(t.visible(), 0);
        assert_eq!(t.empty_text(), "No scripts match “zzz”");
    }

    #[test]
    fn the_filter_hides_rows_but_keeps_them_in_the_page() {
        // JS filters the rows in the page; a row the server dropped could
        // not come back when the filter is cleared.
        let t = table().apply("/scripts", &params(&[("q", "eta")]));
        assert_eq!(t.rows.len(), 4);
        assert_eq!(names(&t), ["Beta"]);
        assert_eq!(t.count_text(), "1 of 4 scripts");
        // Still sorted as a whole, hidden rows too.
        let all: Vec<_> = t.rows.iter().map(|r| r.cells[0].text.as_str()).collect();
        assert_eq!(all, ["alpha", "Beta", "Gamma", "delta"]);
    }

    #[test]
    fn count_text_names_the_filter() {
        let t = table().apply("/scripts", &params(&[]));
        assert_eq!(t.count_text(), "4 scripts");
        let t = table().apply("/scripts", &params(&[("q", "eta")]));
        assert_eq!(t.count_text(), "1 of 4 scripts");
        let mut one = table();
        one.rows.truncate(1);
        assert_eq!(one.apply("/scripts", &params(&[])).count_text(), "1 script");
    }

    #[test]
    fn header_links_keep_the_other_parameters_and_toggle() {
        let t = table().apply(
            "/scripts",
            &params(&[("range", "7d"), ("file", "TM"), ("sort", "n"), ("q", "a b")]),
        );
        // The current column flips; the other starts in its first direction.
        assert_eq!(
            t.cols[1].href,
            "/scripts?file=TM&range=7d&sort=n&dir=asc&q=a+b"
        );
        assert_eq!(
            t.cols[0].href,
            "/scripts?file=TM&range=7d&sort=name&dir=asc&q=a+b"
        );
        assert_eq!((t.cols[1].aria, t.cols[1].arrow), ("descending", "desc"));
        assert_eq!(t.cols[0].aria, "none");
        // The filter form keeps the other parameters, not the table state.
        assert!(t.keep.contains(&("range".into(), "7d".into())));
        assert!(!t.keep.iter().any(|(k, _)| k == "q" || k == "sort"));
    }

    #[test]
    fn server_mode_leaves_the_rows_to_the_page() {
        let mut t = table();
        t.server = true;
        let t = t.apply("/freezes", &params(&[("sort", "name"), ("q", "zzz")]));
        assert_eq!(t.rows.len(), 4);
        assert_eq!(t.sort, "name");
    }
}
