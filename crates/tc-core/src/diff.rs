//! Side-by-side text comparison (Tools › "Diff tool"): lines of two texts
//! paired as equal, changed, only on the left or only on the right.

use similar::{ChangeTag, TextDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Same,
    Changed,
    /// Only in the left text.
    Removed,
    /// Only in the right text.
    Added,
}

/// One row: line numbers (1-based) and texts of both sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    pub left: Option<(usize, String)>,
    pub right: Option<(usize, String)>,
}

/// Counts of the rows that differ.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub changed: usize,
    pub removed: usize,
    pub added: usize,
}

pub fn summary(rows: &[Row]) -> Summary {
    let mut s = Summary::default();
    for r in rows {
        match r.kind {
            RowKind::Changed => s.changed += 1,
            RowKind::Removed => s.removed += 1,
            RowKind::Added => s.added += 1,
            RowKind::Same => {}
        }
    }
    s
}

fn line(s: &str) -> String {
    s.trim_end_matches(['\n', '\r']).to_string()
}

/// Compare two texts line by line. Deleted lines followed by inserted
/// ones are paired as "changed".
pub fn side_by_side(left: &str, right: &str) -> Vec<Row> {
    let diff = TextDiff::from_lines(left, right);
    let mut rows = Vec::new();
    let mut removed: Vec<(usize, String)> = Vec::new();
    let mut added: Vec<(usize, String)> = Vec::new();
    let flush = |rows: &mut Vec<Row>,
                 removed: &mut Vec<(usize, String)>,
                 added: &mut Vec<(usize, String)>| {
        let n = removed.len().max(added.len());
        let mut r = removed.drain(..);
        let mut a = added.drain(..);
        for _ in 0..n {
            let (l, rt) = (r.next(), a.next());
            let kind = match (&l, &rt) {
                (Some(_), Some(_)) => RowKind::Changed,
                (Some(_), None) => RowKind::Removed,
                _ => RowKind::Added,
            };
            rows.push(Row {
                kind,
                left: l,
                right: rt,
            });
        }
    };
    for change in diff.iter_all_changes() {
        let text = line(change.value());
        match change.tag() {
            ChangeTag::Delete => {
                if !added.is_empty() {
                    flush(&mut rows, &mut removed, &mut added);
                }
                removed.push((change.old_index().unwrap_or(0) + 1, text));
            }
            ChangeTag::Insert => added.push((change.new_index().unwrap_or(0) + 1, text)),
            ChangeTag::Equal => {
                flush(&mut rows, &mut removed, &mut added);
                rows.push(Row {
                    kind: RowKind::Same,
                    left: Some((change.old_index().unwrap_or(0) + 1, text.clone())),
                    right: Some((change.new_index().unwrap_or(0) + 1, text)),
                });
            }
        }
    }
    flush(&mut rows, &mut removed, &mut added);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_changes() {
        let rows = side_by_side("a\nb\nc\nd\n", "a\nB\nc\nd\ne\n");
        let kinds: Vec<RowKind> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            [
                RowKind::Same,
                RowKind::Changed,
                RowKind::Same,
                RowKind::Same,
                RowKind::Added
            ]
        );
        assert_eq!(rows[1].left, Some((2, "b".into())));
        assert_eq!(rows[1].right, Some((2, "B".into())));
        assert_eq!(rows[4].right, Some((5, "e".into())));
        assert_eq!(
            summary(&rows),
            Summary {
                changed: 1,
                removed: 0,
                added: 1
            }
        );
        let rows = side_by_side("x\ny\n", "y\n");
        assert_eq!(rows[0].kind, RowKind::Removed);
        assert!(
            side_by_side("same\r\n", "same\r\n")
                .iter()
                .all(|r| r.kind == RowKind::Same)
        );
    }
}
